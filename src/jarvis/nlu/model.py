"""La red que clasifica la intención (C3), con el motor propio de `jarvis.neural`.

n-gramas (4096 casillas, ver `text.py`) → capa oculta de 64 con ReLU → una salida por
intención. Unos 270 000 pesos: entrena en segundos en la CPU y contesta en menos de un
milisegundo.

**Saber cuándo no sabe.** Una red siempre elige alguna clase, aunque la frase no se parezca a
nada de lo que vio. Para no actuar por una suposición:
1. **Temperatura** (calibración): se busca el `T` que hace que las probabilidades de
   `softmax(logits / T)` digan la verdad en validación (cuando dice 90 %, acierta ~90 %).
2. **Umbrales**: la probabilidad mínima para aceptar una intención, elegida en validación como
   la más baja que deja menos de 1 % de acciones equivocadas. Son dos: uno para cuando hay
   **evidencia** aparte de la red (la intención necesita un dato y las reglas lo encontraron:
   una dirección, un archivo, una app de la lista) y otro, más exigente, para cuando no la hay.
   Dos señales independientes que coinciden equivocan mucho menos que una sola. Debajo del
   umbral, el NLU se abstiene y la frase va al modelo de lenguaje.
El conjunto de prueba nunca se usa para elegir nada de esto.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

from jarvis.neural import AdamW, Sequential, cross_entropy, mlp, no_grad, tensor
from jarvis.nlu.intents import BY_NAME, NAMES
from jarvis.nlu.text import DIM, vectorize

HIDDEN = 64
#: ¿Hay evidencia aparte de la red para esta intención en esta frase? `True`: necesita datos y
#: las reglas los encontraron; `False`: los necesita y no están (no se puede actuar: se
#: abstiene); `None`: no necesita datos. (Ver `router.evidence`.)
Evidence = Callable[[str, str], bool | None]
#: Lo más que se acepta equivocarse con confianza (en validación) al elegir el umbral.
MAX_WRONG = 0.01


@dataclass
class Prediction:
    intent: str | None
    confidence: float
    #: La intención más probable aunque no pase el umbral (para el registro).
    best: str = ""
    probs: dict[str, float] = field(default_factory=dict)


class IntentModel:
    def __init__(self, intents: list[str] | None = None, seed: int = 0) -> None:
        self.intents = list(intents or NAMES)
        self.seed = seed
        self.net: Sequential = mlp(
            [DIM, HIDDEN, len(self.intents)], rng=np.random.default_rng(seed)
        )
        self.temperature = 1.0
        self.threshold = 0.5
        self.threshold_evidence = 0.5
        self.meta: dict[str, Any] = {}

    # --- entrenamiento ------------------------------------------------------------------------

    def fit(
        self,
        texts: list[str],
        labels: list[str],
        epochs: int = 30,
        lr: float = 3e-3,
        batch: int = 64,
        weight_decay: float = 1e-4,
    ) -> list[float]:
        """Entrena con AdamW. Devuelve la pérdida promedio de cada época."""
        x = vectorize(texts)
        y = np.array([self.intents.index(lab) for lab in labels])
        opt = AdamW(self.net.parameters(), lr=lr, weight_decay=weight_decay)
        rng = np.random.default_rng(self.seed)
        history: list[float] = []
        for _ in range(epochs):
            order = rng.permutation(len(y))
            total = 0.0
            for start in range(0, len(y), batch):
                idx = order[start : start + batch]
                loss = cross_entropy(self.net(tensor(x[idx])), y[idx])
                opt.zero_grad()
                loss.backward()
                opt.step()
                total += loss.item() * len(idx)
            history.append(total / max(1, len(y)))
        return history

    def logits(self, texts: list[str]) -> np.ndarray:
        with no_grad():
            return self.net(tensor(vectorize(texts))).numpy()

    def proba(self, texts: list[str]) -> np.ndarray:
        z = self.logits(texts) / self.temperature
        z = z - z.max(axis=1, keepdims=True)
        e = np.exp(z)
        out: np.ndarray = e / e.sum(axis=1, keepdims=True)
        return out

    def calibrate(
        self, texts: list[str], labels: list[str], evidence: Evidence | None = None
    ) -> None:
        """Temperatura (mínima pérdida en validación) y después el umbral."""
        z = self.logits(texts)
        y = np.array([self.intents.index(lab) for lab in labels])
        best_t, best_nll = 1.0, float("inf")
        for t in np.linspace(0.3, 5.0, 48):
            s = z / t
            s = s - s.max(axis=1, keepdims=True)
            logp = s - np.log(np.exp(s).sum(axis=1, keepdims=True))
            nll = float(-logp[np.arange(len(y)), y].mean())
            if nll < best_nll:
                best_t, best_nll = float(t), nll
        self.temperature = best_t
        p = self.proba(texts)
        pred = p.argmax(axis=1)
        conf = p.max(axis=1)
        actionable = np.array([BY_NAME[self.intents[i]].route != "derivar" for i in pred])
        wrong = (pred != y) & actionable
        ev = [
            evidence(self.intents[i], t) if evidence is not None else None
            for i, t in zip(pred, texts, strict=True)
        ]
        has = np.array([e is True for e in ev])
        plain = np.array([e is None for e in ev])
        # Sin los datos que necesita, igual se abstiene: no es una acción equivocada posible.
        # La mitad del margen de error para cada grupo.
        self.threshold = _lowest(conf[plain], wrong[plain], len(y), MAX_WRONG / 2)
        self.threshold_evidence = _lowest(conf[has], wrong[has], len(y), MAX_WRONG / 2)

    # --- uso ------------------------------------------------------------------------------------

    def predict(self, text: str, evidence: Evidence | None = None) -> Prediction:
        p = self.proba([text])[0]
        i = int(p.argmax())
        conf = float(p[i])
        best = self.intents[i]
        probs = {self.intents[j]: float(p[j]) for j in np.argsort(-p)[:3]}
        ev = evidence(best, text) if evidence is not None else None
        if ev is False:
            return Prediction(None, conf, best, probs)
        needed = self.threshold_evidence if ev else self.threshold
        return Prediction(best if conf >= needed else None, conf, best, probs)

    # --- guardar --------------------------------------------------------------------------------

    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        meta = {
            **self.meta,
            "intents": self.intents,
            "temperature": self.temperature,
            "threshold": self.threshold,
            "threshold_evidence": self.threshold_evidence,
            "seed": self.seed,
            "dim": DIM,
            "hidden": HIDDEN,
        }
        arrays: dict[str, Any] = dict(self.net.state_dict())
        arrays["__meta__"] = np.array(json.dumps(meta, ensure_ascii=False))
        savez: Any = np.savez
        savez(path, **arrays)

    @classmethod
    def load(cls, path: Path) -> IntentModel:
        # allow_pickle=False: un archivo de pesos no puede ejecutar código al abrirlo.
        with np.load(path, allow_pickle=False) as data:
            meta = json.loads(str(data["__meta__"]))
            if meta.get("dim") != DIM or meta.get("hidden") != HIDDEN:
                raise ValueError("el modelo guardado es de otra versión")
            m = cls(list(meta["intents"]), int(meta.get("seed", 0)))
            m.net.load_state_dict({k: data[k] for k in data.files if k != "__meta__"})
        m.temperature = float(meta["temperature"])
        m.threshold = float(meta["threshold"])
        m.threshold_evidence = float(meta.get("threshold_evidence", m.threshold))
        m.meta = {k: v for k, v in meta.items() if k not in ("intents",)}
        return m


def _lowest(conf: np.ndarray, wrong: np.ndarray, total: int, budget: float) -> float:
    """El umbral más bajo con el que las acciones equivocadas aceptadas no pasan `budget`."""
    for th in np.linspace(0.3, 0.97, 68):
        if float((wrong & (conf >= th)).sum()) / max(1, total) <= budget:
            return float(th)
    return 0.97
