"""El cerebro: recibe un pedido en texto y devuelve la respuesta de a pedazos.

- `ClaudeBrain`: Claude por el Agent SDK, con el login de Claude Code de la PC (sin API key).
  De las herramientas de Claude Code solo tiene WebSearch y WebFetch (leer la web); actúa sobre
  JARVIS-OS solo con las de JARVIS.
- `ScriptedBrain`: respuestas fijas, sin red ni API, para los tests (`jarvis serve --simulado`).
"""

from __future__ import annotations

import asyncio
import re
from collections.abc import AsyncIterator, Callable
from typing import Any, Protocol

from jarvis.agent.prompts import build_prompt
from jarvis.config import Config
from jarvis.policy.audit import audit
from jarvis.policy.levels import auto_approved, bare, builtin_tools
from jarvis.tools.system import Gate, Kernel, build_server


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
    def __init__(
        self,
        config: Config,
        kernel: Kernel,
        voice: bool = False,
        memory: Callable[[], str] | None = None,
    ) -> None:
        self._config = config
        self._voice = voice
        #: Lo que recuerda de antes (va en el prompt al conectarse).
        self._memory = memory
        self._kernel = kernel
        self._gate = Gate(kernel)
        self._client: Any = None

    async def _can_use_tool(self, name: str, args: dict[str, Any], _ctx: Any) -> Any:
        from claude_agent_sdk import PermissionResultAllow, PermissionResultDeny

        if await self._gate.permit(name, args):
            return PermissionResultAllow()
        return PermissionResultDeny(message="Roman rechazó la acción (o no está permitida).")

    async def _audit_hook(self, data: Any, _tool_use_id: Any, _ctx: Any) -> dict[str, Any]:
        # Corre antes que cualquier regla: queda registro de todo intento, aprobado o no.
        audit("intento", bare(str(data.get("tool_name", "?"))), dict(data.get("tool_input", {})))
        return {}

    def _options(self) -> Any:
        from claude_agent_sdk import ClaudeAgentOptions, HookMatcher

        return ClaudeAgentOptions(
            system_prompt=build_prompt(self._voice, memory=self._memory() if self._memory else ""),
            model=self._config.modelo,
            # De Claude Code, solo leer la web (policy/levels.py): JARVIS actúa con las suyas.
            tools=builtin_tools(),
            mcp_servers={"jarvis": build_server(self._kernel)},
            # Solo las de nivel 1 se aprueban solas; las de 2 y 3 caen en can_use_tool.
            allowed_tools=auto_approved(),
            can_use_tool=self._can_use_tool,
            hooks={"PreToolUse": [HookMatcher(hooks=[self._audit_hook])]},  # type: ignore[list-item]
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


# Pedidos con acción del cerebro simulado: (patrón, tool, argumentos a partir del match).
ACTIONS: list[tuple[str, str, Any]] = [
    (r"record[aá] que (.+)", "recordar", lambda m: {"dato": m[1]}),
    (r"te acord[aá]s (?:de|del) (.+?)\??$", "buscar_memoria", lambda m: {"consulta": m[1]}),
    (r"modific[aá] el sistema:? (.+)", "modificar_sistema", lambda m: {"pedido": m[1]}),
    (r"aplic[aá] los cambios", "aplicar_cambios_sistema", lambda m: {}),
    (
        r"abr[ií] (?:el )?proyecto (\S+)",
        "abrir_proyecto",
        lambda m: {"nombre": m[1], "seguir": True, "pedido": ""},
    ),
    (r"abr[ií] el navegador y busc[aá] (.+)", "buscar_web", lambda m: {"consulta": m[1]}),
    (r"abr[ií] (?:el |la )?(\w+)$", "abrir_app", lambda m: {"app": m[1]}),
    (r"cre[aá] (\S+) con (.+)", "escribir_archivo", lambda m: {"ruta": m[1], "contenido": m[2]}),
    (r"(?:borr[aá]|tir[aá]) (\S+)", "a_papelera", lambda m: {"ruta": m[1]}),
    (r"le[eé] (\S+)", "leer_archivo", lambda m: {"ruta": m[1]}),
    (r"mostr[aá]me (\S+)", "abrir_archivo", lambda m: {"ruta": m[1]}),
]


class ScriptedBrain:
    """Sin red ni API: para `cargo xtask test` y los tests de Python. Con `kernel`, algunos
    pedidos ejecutan una tool, con los mismos permisos que con Claude."""

    def __init__(self, kernel: Kernel | None = None, delay: float = 0.02) -> None:
        self._gate = Gate(kernel) if kernel is not None else None
        self._delay = delay
        self._stop = False

    async def _act(self, text: str) -> str | None:
        if self._gate is None:
            return None
        for pattern, tool, make_args in ACTIONS:
            m = re.search(pattern, text.strip(), re.IGNORECASE)
            if m:
                ok, data = await self._gate.run(tool, make_args(m))
                return f"Hecho ({tool}): {data}" if ok else f"No lo hice: {data}"
        return None

    async def reply(self, text: str) -> AsyncIterator[str]:
        self._stop = False
        low = text.lower()
        answer = await self._act(text) or next(
            (a for k, a in SCRIPT if k in low), f"Entendido: {text}"
        )
        for i, word in enumerate(answer.split(" ")):
            if self._stop:
                return
            await asyncio.sleep(self._delay)
            yield word if i == 0 else " " + word

    async def interrupt(self) -> None:
        self._stop = True

    async def close(self) -> None:
        pass
