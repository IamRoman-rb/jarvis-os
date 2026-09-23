"""Confirmación de acciones de nivel 2 y 3.

Por ahora se pregunta por terminal. En la fase 1b se agrega una implementación con
notificaciones de escritorio; ambas cumplen el protocolo ``Confirmer``.
"""

import asyncio
import json
from collections.abc import Callable
from typing import Any, Protocol

from jarvis.policy.levels import Level, Origin


class Confirmer(Protocol):
    async def __call__(
        self, tool: str, tool_input: dict[str, Any], level: Level, origin: Origin
    ) -> bool:
        """Devuelve True solo si el usuario aprobó explícitamente."""
        ...


def describe_action(tool: str, tool_input: dict[str, Any], level: Level) -> str:
    """Texto que ve el usuario. Muestra los argumentos completos: nunca se truncan."""
    args = json.dumps(tool_input, ensure_ascii=False, indent=2)
    return f"JARVIS quiere ejecutar {tool} (nivel {int(level)}) con:\n{args}"


class TerminalConfirmer:
    """Pregunta en la terminal. Cualquier respuesta distinta de la esperada es un "no"."""

    def __init__(
        self,
        read: Callable[[str], str] = input,
        write: Callable[[str], None] = print,
    ) -> None:
        self._read = read
        self._write = write

    async def __call__(
        self, tool: str, tool_input: dict[str, Any], level: Level, origin: Origin
    ) -> bool:
        self._write(describe_action(tool, tool_input, level))
        if level >= Level.CRITICAL:
            prompt = "Acción crítica. Escribí 'si' para permitirla: "
        else:
            prompt = "¿Permitir? [s/N]: "
        try:
            answer = await asyncio.to_thread(self._read, prompt)
        except EOFError:
            return False
        answer = answer.strip().lower()
        if level >= Level.CRITICAL:
            return answer in ("si", "sí")
        return answer in ("s", "si", "sí")
