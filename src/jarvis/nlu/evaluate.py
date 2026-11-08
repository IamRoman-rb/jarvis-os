"""Las métricas del NLU (C3), las mismas para la red y para la línea de base.

- **exactitud** y **F1 macro** (el promedio del F1 de cada intención: una intención con pocos
  ejemplos pesa igual que una con muchos), y precisión/cobertura/F1 por intención;
- **datos**: de las frases con la intención bien, en cuántas los argumentos son exactamente
  los esperados;
- **acciones equivocadas**: cuántas veces se habría hecho algo (una tool o una respuesta local)
  que no era lo pedido. Es la métrica que más importa: abstenerse de más cuesta una llamada al
  modelo; equivocarse hace otra cosa;
- **abstención**: en frases de JARVIS (ojalá poca) y en frases fuera de alcance nunca vistas
  (ojalá casi todas: se abstiene o dice "fuera");
- **latencia** por frase (p50 y p95);
- **cobertura local**: qué parte de las órdenes se resolvería sin el modelo de lenguaje en el
  modo "local primero" (intención y datos bien, y una tool de nivel 1 o una respuesta local).
"""

from __future__ import annotations

import time
from collections.abc import Callable
from dataclasses import dataclass, field

from jarvis.nlu import slots as slot_rules
from jarvis.nlu.dataset import Example
from jarvis.nlu.intents import BY_NAME
from jarvis.policy.levels import level_of

#: Predice (intención o `None` si se abstiene, argumentos o `None`).
Predictor = Callable[[str], tuple[str | None, dict[str, str] | None]]


@dataclass
class Report:
    n: int = 0
    accuracy: float = 0.0
    macro_f1: float = 0.0
    per_intent: dict[str, tuple[float, float, float, int]] = field(default_factory=dict)
    slot_exact: float = 0.0
    wrong_action: float = 0.0
    abstain_in_scope: float = 0.0
    ood_n: int = 0
    ood_rejected: float = 0.0
    ood_wrong_action: float = 0.0
    p50_ms: float = 0.0
    p95_ms: float = 0.0
    local_coverage: float = 0.0

    def text(self, title: str) -> str:
        lines = [
            f"== {title} ==",
            f"frases: {self.n} · exactitud {self.accuracy:.3f} · F1 macro {self.macro_f1:.3f}",
            f"datos exactos (con la intención bien): {self.slot_exact:.3f}",
            f"acciones equivocadas: {self.wrong_action:.3%} · "
            f"abstención en órdenes: {self.abstain_in_scope:.3%}",
            f"fuera de alcance nunca vistas ({self.ood_n}): rechazadas {self.ood_rejected:.3%}"
            f" · acciones equivocadas {self.ood_wrong_action:.3%}",
            f"latencia p50 {self.p50_ms:.2f} ms · p95 {self.p95_ms:.2f} ms",
            f"cobertura local (sin modelo de lenguaje): {self.local_coverage:.3%}",
            "por intención (precisión, cobertura, F1, n):",
        ]
        for name, (p, r, f1, n) in sorted(self.per_intent.items()):
            lines.append(f"  {name:<18} {p:.2f}  {r:.2f}  {f1:.2f}  {n}")
        return "\n".join(lines)


def actionable(intent: str | None) -> bool:
    return intent is not None and BY_NAME[intent].route != "derivar"


def resolved_locally(intent: str | None, args: dict[str, str] | None) -> bool:
    """¿El modo "local primero" lo resolvería sin el modelo? (Nivel 1 o respuesta local.)"""
    if intent is None:
        return False
    spec = BY_NAME[intent]
    if spec.route == "local":
        return True
    if spec.route == "tool" and spec.tool is not None:
        return args is not None and level_of(spec.tool) == 1
    return False


def _slots_ok(gold: dict[str, str], got: dict[str, str] | None) -> bool:
    if not gold:
        return True
    if got is None:
        return False
    return all(k in got and slot_rules.same(got[k], v) for k, v in gold.items())


def evaluate(predict: Predictor, examples: list[Example], ood: list[Example]) -> Report:
    r = Report(n=len(examples), ood_n=len(ood))
    times: list[float] = []
    tp: dict[str, int] = {}
    fp: dict[str, int] = {}
    fn: dict[str, int] = {}
    support: dict[str, int] = {}
    correct = wrong = abstain = slot_total = slot_good = local = in_scope = 0
    for ex in examples:
        start = time.perf_counter()
        intent, args = predict(ex.text)
        times.append((time.perf_counter() - start) * 1000)
        support[ex.intent] = support.get(ex.intent, 0) + 1
        if ex.intent != "fuera":
            in_scope += 1
            if intent is None:
                abstain += 1
        if intent == ex.intent:
            correct += 1
            tp[ex.intent] = tp.get(ex.intent, 0) + 1
            if ex.slots:
                slot_total += 1
                slot_good += _slots_ok(ex.slots, args)
            if (
                ex.intent != "fuera"
                and _slots_ok(ex.slots, args)
                and resolved_locally(intent, args)
            ):
                local += 1
        else:
            fn[ex.intent] = fn.get(ex.intent, 0) + 1
            if intent is not None:
                fp[intent] = fp.get(intent, 0) + 1
            if actionable(intent):
                wrong += 1
    ood_rej = ood_wrong = 0
    for ex in ood:
        start = time.perf_counter()
        intent, _ = predict(ex.text)
        times.append((time.perf_counter() - start) * 1000)
        if intent is None or not actionable(intent):
            ood_rej += 1
        else:
            ood_wrong += 1
    f1s = []
    for name in support:
        p = tp.get(name, 0) / max(1, tp.get(name, 0) + fp.get(name, 0))
        rc = tp.get(name, 0) / max(1, support[name])
        f1 = 2 * p * rc / (p + rc) if p + rc else 0.0
        r.per_intent[name] = (p, rc, f1, support[name])
        f1s.append(f1)
    r.accuracy = correct / max(1, len(examples))
    r.macro_f1 = sum(f1s) / max(1, len(f1s))
    r.slot_exact = slot_good / max(1, slot_total)
    r.wrong_action = wrong / max(1, len(examples))
    r.abstain_in_scope = abstain / max(1, in_scope)
    r.ood_rejected = ood_rej / max(1, len(ood))
    r.ood_wrong_action = ood_wrong / max(1, len(ood))
    r.local_coverage = local / max(1, in_scope)
    times.sort()
    if times:
        r.p50_ms = times[len(times) // 2]
        r.p95_ms = times[min(len(times) - 1, int(len(times) * 0.95))]
    return r
