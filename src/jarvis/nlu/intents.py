"""Las intenciones que entiende el NLU local y qué hace cada una.

Cada intención es una tool que ya existe (`tools/system.py`, con su nivel en
`policy/levels.py`), una respuesta local (la hora, un saludo) o una derivación: lo de
programación va a los agentes de código y lo que no es una orden, al modelo de lenguaje. No hay
intenciones para borrar, ejecutar comandos ni instalar: esas siempre las decide el modelo y
las confirma Roman.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

Route = Literal["tool", "local", "derivar"]


@dataclass(frozen=True)
class Intent:
    name: str
    route: Route
    #: La tool de JARVIS (solo si `route == "tool"`).
    tool: str | None = None
    #: Los datos que necesita (sin ellos, el NLU se abstiene).
    slots: tuple[str, ...] = ()
    #: Argumentos fijos que la tool necesita y la frase no dice.
    fixed: tuple[tuple[str, str], ...] = ()


INTENTS: tuple[Intent, ...] = (
    Intent("abrir_app", "tool", "abrir_app", ("app",)),
    Intent("abrir_archivo", "tool", "abrir_archivo", ("ruta",)),
    Intent("abrir_web", "tool", "abrir_web", ("url",)),
    Intent("buscar_web", "tool", "buscar_web", ("consulta",)),
    Intent("listar_archivos", "tool", "listar_archivos", ("ruta",)),
    Intent("crear_carpeta", "tool", "crear_carpeta", ("ruta",)),
    Intent("estado_sistema", "tool", "estado_sistema"),
    Intent("ventanas_abiertas", "tool", "ventanas_abiertas"),
    Intent("cerrar_ventana", "tool", "cerrar_ventana", ("app",)),
    Intent("recordar", "tool", "recordar", ("dato",)),
    Intent("buscar_memoria", "tool", "buscar_memoria", ("consulta",)),
    Intent("hora", "local"),
    Intent("fecha", "local"),
    Intent("saludo", "local"),
    Intent("gracias", "local"),
    # Programar (abrir un proyecto, arreglar, agregar): el agente de código, por el modelo.
    Intent("programar", "derivar"),
    # Lo que no es una orden de JARVIS-OS: una pregunta, una charla, redactar algo.
    Intent("fuera", "derivar"),
)

BY_NAME: dict[str, Intent] = {i.name: i for i in INTENTS}
NAMES: list[str] = [i.name for i in INTENTS]
