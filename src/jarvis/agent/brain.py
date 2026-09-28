"""El cerebro: recibe un pedido en texto y devuelve la respuesta de a pedazos.

- `ClaudeBrain`: Claude por el Agent SDK, con el login de Claude Code de la PC (sin API key).
  Sin herramientas propias de Claude Code (Bash, Edit…): solo puede actuar con las de JARVIS.
- `ScriptedBrain`: respuestas fijas, sin red ni API, para los tests (`jarvis serve --simulado`).
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
from typing import Any, Protocol

from jarvis.agent.prompts import JARVIS_SYSTEM_PROMPT
from jarvis.config import Config


class BrainError(RuntimeError):
    """El cerebro no pudo responder (sin Claude Code, sin sesión, error del modelo)."""


class Brain(Protocol):
    def reply(self, text: str) -> AsyncIterator[str]:
        """La respuesta a `text`, de a pedazos (se muestran a medida que llegan)."""
        ...

    async def interrupt(self) -> None:
        """Corta la respuesta en curso."""
        ...

    async def close(self) -> None: ...


class ClaudeBrain:
    def __init__(self, config: Config) -> None:
        self._config = config
        self._client: Any = None

    def _options(self) -> Any:
        from claude_agent_sdk import ClaudeAgentOptions

        return ClaudeAgentOptions(
            system_prompt=JARVIS_SYSTEM_PROMPT,
            model=self._config.modelo,
            # Sin las herramientas de Claude Code: JARVIS actúa solo con las suyas (§12.2).
            tools=[],
            allowed_tools=[],
            # Nunca bypassPermissions ni acceptEdits (ver docs/permisos.md).
            permission_mode="default",
            include_partial_messages=True,
            # Sin CLAUDE.md ni ajustes de proyectos: el cerebro no trabaja sobre un repo.
            setting_sources=[],
        )

    async def _connect(self) -> Any:
        if self._client is None:
            from claude_agent_sdk import ClaudeSDKClient, CLINotFoundError

            client = ClaudeSDKClient(options=self._options())
            try:
                await client.connect()
            except CLINotFoundError as e:
                raise BrainError(
                    "No encontré Claude Code en esta PC (el Agent SDK lo usa para iniciar sesión)."
                ) from e
            self._client = client
        return self._client

    async def reply(self, text: str) -> AsyncIterator[str]:
        from claude_agent_sdk import AssistantMessage, ResultMessage, StreamEvent, TextBlock

        client = await self._connect()
        await client.query(text)
        streamed = False
        async for msg in client.receive_response():
            if isinstance(msg, StreamEvent):
                ev = msg.event
                delta = ev.get("delta", {}) if ev.get("type") == "content_block_delta" else {}
                if delta.get("type") == "text_delta" and delta.get("text"):
                    streamed = True
                    yield delta["text"]
            elif isinstance(msg, AssistantMessage) and not streamed:
                # Sin eventos parciales (versiones viejas del CLI): el mensaje entero.
                for block in msg.content:
                    if isinstance(block, TextBlock):
                        yield block.text
            elif isinstance(msg, ResultMessage) and msg.is_error:
                raise BrainError(str(msg.result or "Claude devolvió un error"))

    async def interrupt(self) -> None:
        if self._client is not None:
            await self._client.interrupt()

    async def close(self) -> None:
        if self._client is not None:
            await self._client.disconnect()
            self._client = None


# Respuestas del cerebro simulado: la primera clave contenida en el pedido gana.
SCRIPT: list[tuple[str, str]] = [
    ("hola", "Hola, Roman. Sistema en línea y cerebro conectado."),
    ("hora", "No tengo reloj propio, pero la barra de arriba sabe."),
]


class ScriptedBrain:
    """Sin red ni API: para `cargo xtask test` y los tests de Python."""

    def __init__(self, delay: float = 0.02) -> None:
        self._delay = delay
        self._stop = False

    async def reply(self, text: str) -> AsyncIterator[str]:
        self._stop = False
        low = text.lower()
        answer = next((a for k, a in SCRIPT if k in low), f"Entendido: {text}")
        for i, word in enumerate(answer.split(" ")):
            if self._stop:
                return
            await asyncio.sleep(self._delay)
            yield word if i == 0 else " " + word

    async def interrupt(self) -> None:
        self._stop = True

    async def close(self) -> None:
        pass
