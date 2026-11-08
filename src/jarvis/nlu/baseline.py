"""La línea de base: las reglas que JARVIS tiene hoy, para comparar contra la red.

Junta las dos fuentes de reglas que existen: los comandos de la Consola del kernel
(`kernel/desktop/src/apps/console.rs`: la primera palabra decide, "abrir", "buscar", "hora"…)
y las expresiones regulares del cerebro simulado (`agent/brain.py`, `ACTIONS`). Si ninguna
coincide, se abstiene. Si la red no le gana a esto, no vale la pena (C3).
"""

from __future__ import annotations

import re

from jarvis.nlu.text import plain

#: (expresión sobre el texto sin tildes, intención). La primera que coincide gana.
RULES: list[tuple[str, str]] = [
    # Consola: la primera palabra.
    (r"^(hora|fecha)\b", "hora"),
    (r"^(abrir|abri|open)\s+\S+\.\w{2,4}$", "abrir_archivo"),
    (r"^(ir|navegar|abrir-web)\b", "abrir_web"),
    (r"^(buscar|googlear)\b", "buscar_web"),
    (r"^(ls|dir)\b", "listar_archivos"),
    (r"^(estado|top)\b", "estado_sistema"),
    (r"^(abrir|abri|open)\b", "abrir_app"),
    (r"^hola\b", "saludo"),
    # Cerebro simulado.
    (r"record[a] que ", "recordar"),
    (r"te acord[a]s (de|del) ", "buscar_memoria"),
    (r"modific[a] el sistema", "programar"),
    (r"abr[i] (el )?proyecto ", "programar"),
    (r"abr[i] el navegador y busc[a] ", "buscar_web"),
    (r"^le[e] \S+", "abrir_archivo"),
    (r"^mostrame \S+\.\w{2,4}", "abrir_archivo"),
]


def predict(text: str) -> str | None:
    t = plain(text)
    for pattern, intent in RULES:
        if re.search(pattern, t):
            return intent
    return None
