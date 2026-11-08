"""El cerebro "local primero": el NLU contesta lo cotidiano y lo demás va al de siempre.

Tres modos (`cerebro_local` en `config.toml`):
- `apagado`: como antes, todo va al modelo de lenguaje;
- `sombra` (el de fábrica): el NLU predice y se anota qué habría hecho, pero **no hace nada**:
  sirve para medir con pedidos reales antes de confiarle algo;
- `primero`: si el NLU está seguro y la orden es una tool de **nivel 1** (abrir una app, buscar,
  listar) o una respuesta local (la hora, un saludo), la resuelve sin el modelo. Lo de nivel 2
  o 3, programar, lo ambiguo o lo que no entiende va al modelo.

El NLU no autoriza nada: una tool se pide por `Gate.run`, con los mismos niveles, la misma
confirmación y la misma auditoría que si la hubiera pedido Claude. Y el texto de Roman es solo
un dato: no hay intenciones para borrar, instalar ni ejecutar comandos.
"""

from __future__ import annotations

import logging
import re
import time
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Literal, Protocol

from platformdirs import user_data_path

from jarvis.nlu import dataset
from jarvis.nlu import slots as slot_rules
from jarvis.nlu.intents import BY_NAME
from jarvis.nlu.model import IntentModel
from jarvis.nlu.text import plain
from jarvis.policy.levels import level_of

log = logging.getLogger("jarvis.nlu")

Action = Literal["tool", "local", "derivar", "abstener"]
MODES = ("apagado", "sombra", "primero")

DAYS = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"]
MONTHS = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
]


@dataclass
class Decision:
    action: Action
    intent: str | None = None
    confidence: float = 0.0
    best: str = ""
    tool: str | None = None
    args: dict[str, str] = field(default_factory=dict)
    answer: str = ""
    #: Por qué no se resolvió local (para el registro).
    reason: str = ""
    ms: float = 0.0

    def summary(self) -> dict[str, Any]:
        return {
            "accion": self.action,
            "intencion": self.intent or self.best,
            "segura": self.intent is not None,
            "confianza": round(self.confidence, 3),
            "tool": self.tool,
            "motivo": self.reason,
            "ms": round(self.ms, 2),
        }


def local_answer(intent: str, now: datetime) -> str:
    if intent == "hora":
        return f"Son las {now.hour}:{now.minute:02d}."
    if intent == "fecha":
        return f"Hoy es {DAYS[now.weekday()]} {now.day} de {MONTHS[now.month - 1]} de {now.year}."
    if intent == "saludo":
        return "Hola, Roman. ¿Qué necesitás?"
    if intent == "gracias":
        return "De nada. Acá estoy."
    return ""


#: Verbos de lo que el NLU **no** hace (las tools de nivel 2 y 3 de `docs/permisos.md`: borrar,
#: ejecutar, instalar, apagar o reiniciar, mover, copiar, escribir, olvidar, mandar). Si la frase
#: tiene uno, el NLU no actúa aunque crea entender otra cosa ("ejecutá rm -rf en la terminal"
#: no es "abrí la terminal"): la frase va al modelo, que la decide con confirmación de Roman.
NOT_MINE = re.compile(
    r"\b(?:borr|elimin|ejecut|corre(?:r|me)? el|instal|desinstal|apag|reinici|format|"
    r"olvida\b|mov(?:e|eme|er)\b|copi|escribi\b|envia|manda\b|rm\b|sudo\b)"
)


def not_mine(text: str) -> bool:
    return NOT_MINE.search(plain(text)) is not None


def evidence(intent: str, text: str) -> bool | None:
    """¿Las reglas encontraron los datos que la intención necesita? `None` si no necesita."""
    spec = BY_NAME.get(intent)
    if spec is None or not spec.slots:
        return None
    return slot_rules.extract(intent, text) is not None


class Understanding:
    """El NLU entrenado: frase → decisión."""

    def __init__(self, model: IntentModel, clock: Callable[[], datetime] = datetime.now) -> None:
        self.model = model
        self._clock = clock

    def decide(self, text: str) -> Decision:
        start = time.perf_counter()
        d = self._decide(text)
        d.ms = (time.perf_counter() - start) * 1000
        return d

    def _decide(self, text: str) -> Decision:
        p = self.model.predict(text, evidence)
        c, b = p.confidence, p.best
        if not_mine(text):
            return Decision("derivar", p.intent, c, b, reason="pide algo que el NLU no hace")
        if p.intent is None:
            return Decision("abstener", confidence=c, best=b, reason="poca confianza")
        spec = BY_NAME[p.intent]
        if spec.route == "derivar":
            return Decision("derivar", p.intent, c, b, reason=p.intent)
        if spec.route == "local":
            return Decision("local", p.intent, c, b, answer=local_answer(p.intent, self._clock()))
        args = slot_rules.extract(p.intent, text)
        if args is None or spec.tool is None:
            return Decision("abstener", p.intent, c, b, reason="faltan datos")
        level = level_of(spec.tool)
        if level > 1:
            # En esta etapa el NLU solo resuelve nivel 1: lo demás lo decide el modelo.
            return Decision("derivar", p.intent, c, b, spec.tool, args, reason=f"nivel {level}")
        return Decision("tool", p.intent, c, b, spec.tool, args)

    def predictor(self) -> Callable[[str], tuple[str | None, dict[str, str] | None]]:
        """Para `evaluate`: intención y argumentos (o `None`)."""

        def predict(text: str) -> tuple[str | None, dict[str, str] | None]:
            p = self.model.predict(text, evidence)
            if p.intent is None:
                return None, None
            if not_mine(text):
                # Se deriva al modelo: para las métricas cuenta como "fuera" (no actúa).
                return "fuera", None
            spec = BY_NAME[p.intent]
            return p.intent, slot_rules.extract(p.intent, text) if spec.slots else {}

        return predict


# --- entrenar o cargar ------------------------------------------------------------------------


def model_path() -> Path:
    return user_data_path("jarvis") / "nlu" / f"modelo-{dataset.VERSION}.npz"


def train(seed: int = 0, epochs: int = 40) -> IntentModel:
    """Entrena con la partición de entrenamiento y calibra con la de validación."""
    tr = dataset.generate("train")
    va = dataset.generate("val")
    m = IntentModel(seed=seed)
    m.fit([e.text for e in tr], [e.intent for e in tr], epochs=epochs)
    m.calibrate([e.text for e in va], [e.intent for e in va], evidence)
    m.meta = {"dataset": dataset.VERSION, "train": len(tr), "val": len(va)}
    return m


def load_or_train(path: Path | None = None) -> IntentModel:
    p = path or model_path()
    if p.exists():
        try:
            return IntentModel.load(p)
        except (ValueError, KeyError, OSError) as e:
            log.warning("el modelo del NLU no sirve (%s): se entrena de nuevo", e)
    m = train()
    try:
        m.save(p)
    except OSError as e:
        log.warning("no pude guardar el modelo del NLU: %s", e)
    return m


# --- el cerebro -------------------------------------------------------------------------------


class Runner(Protocol):
    """Lo que el cerebro local necesita para actuar: el `Gate` (permiso + ejecución)."""

    async def run(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]: ...


def done_text(tool: str, args: dict[str, str], ok: bool, data: str) -> str:
    if not ok:
        return f"No lo hice: {data}"
    match tool:
        case "abrir_app":
            return f"Abro {args.get('app', 'la app')}."
        case "abrir_web":
            return f"Abro {args.get('url', 'la página')}."
        case "buscar_web":
            return f"Busco «{args.get('consulta', '')}» en el navegador."
        case "abrir_archivo":
            return f"Abro {args.get('ruta', 'el archivo')}."
        case "recordar":
            return "Anotado."
        case _:
            return data


class LocalFirstBrain:
    """Envuelve al cerebro de siempre (`inner`): en `primero`, contesta lo que sabe."""

    def __init__(
        self,
        inner: Any,
        gate: Runner,
        mode: Callable[[], str],
        understanding: Callable[[], Understanding | None],
    ) -> None:
        self._inner = inner
        self._gate = gate
        self._mode = mode
        self._understanding = understanding
        self.last_decision: Decision | None = None
        self.last_local = False

    def last_info(self) -> dict[str, Any]:
        info: dict[str, Any] = {}
        inner_info = getattr(self._inner, "last_info", None)
        if callable(inner_info) and not self.last_local:
            info.update(inner_info())
        if self.last_decision is not None:
            info["nlu"] = self.last_decision.summary()
        info["resuelto_local"] = self.last_local
        if self.last_local:
            info["agente"] = "jarvis-local"
        return info

    async def reply(self, text: str) -> AsyncIterator[str]:
        mode = self._mode()
        self.last_decision = None
        self.last_local = False
        nlu = self._understanding() if mode != "apagado" else None
        if nlu is not None:
            d = nlu.decide(text)
            self.last_decision = d
            if mode == "primero":
                if d.action == "local":
                    self.last_local = True
                    yield d.answer
                    return
                if d.action == "tool" and d.tool is not None:
                    self.last_local = True
                    ok, data = await self._gate.run(d.tool, dict(d.args))
                    yield done_text(d.tool, d.args, ok, data)
                    return
        async for delta in self._inner.reply(text):
            yield delta

    async def interrupt(self) -> None:
        await self._inner.interrupt()

    async def close(self) -> None:
        await self._inner.close()
