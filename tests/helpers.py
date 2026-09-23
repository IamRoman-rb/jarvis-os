"""Dobles de prueba compartidos. Ningún test llama a la API real (salvo los marcados `live`)."""

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    PermissionResultAllow,
    ResultMessage,
    SdkMcpTool,
    TextBlock,
    ToolPermissionContext,
)

from jarvis.policy.levels import Level, Origin, bare_name, mcp_name


@dataclass
class FakeConfirmer:
    """Responde siempre lo mismo y registra qué se le preguntó."""

    answer: bool
    calls: list[tuple[str, dict[str, Any], Level, Origin]] = field(default_factory=list)

    async def __call__(
        self, tool: str, tool_input: dict[str, Any], level: Level, origin: Origin
    ) -> bool:
        self.calls.append((tool, tool_input, level, origin))
        return self.answer


ToolCaller = Callable[[str, dict[str, Any]], Awaitable[dict[str, Any] | None]]
Script = Callable[[ToolCaller], Awaitable[str]]
"""Un "modelo" falso: recibe una función para pedir tools y devuelve el texto final."""


class FakeClaudeClient:
    """Reemplaza a ClaudeSDKClient reproduciendo el orden de permisos documentado del SDK:

    hooks PreToolUse → (``allowed_tools`` → se ejecuta) / (resto → ``can_use_tool``).
    JARVIS no pone nada en ``allowed_tools``, pero el doble respeta la regla igual.

    No hay red: el "razonamiento" lo decide ``script``.
    """

    def __init__(
        self, options: ClaudeAgentOptions, tools: list[SdkMcpTool[Any]], script: Script
    ) -> None:
        self.options = options
        self.handlers = {mcp_name(t.name): t.handler for t in tools}
        self.script = script
        self.prompt: str | None = None
        self.executed: list[str] = []
        self.denied: list[str] = []

    async def __aenter__(self) -> "FakeClaudeClient":
        return self

    async def __aexit__(self, *exc: object) -> None:
        return None

    async def query(self, prompt: str) -> None:
        self.prompt = prompt

    async def call_tool(self, name: str, tool_input: dict[str, Any]) -> dict[str, Any] | None:
        full = name if name.startswith("mcp__") or name[:1].isupper() else mcp_name(name)
        for matcher in (self.options.hooks or {}).get("PreToolUse", []):
            for hook in matcher.hooks:
                out = await hook(
                    {
                        "hook_event_name": "PreToolUse",
                        "tool_name": full,
                        "tool_input": tool_input,
                        "tool_use_id": "toolu_fake",
                        "session_id": "s",
                        "transcript_path": "",
                        "cwd": "",
                    },  # type: ignore[arg-type]
                    "toolu_fake",
                    {"signal": None},
                )
                assert out == {}, "el hook de auditoría no debe tomar decisiones"

        if full in self.options.allowed_tools:
            allowed = True
        else:
            assert self.options.can_use_tool is not None
            result = await self.options.can_use_tool(
                full, tool_input, ToolPermissionContext(tool_use_id="toolu_fake")
            )
            allowed = isinstance(result, PermissionResultAllow)

        if not allowed:
            self.denied.append(bare_name(full))
            return None
        self.executed.append(bare_name(full))
        return await self.handlers[full](tool_input)

    async def receive_response(self):  # type: ignore[no-untyped-def]
        text = await self.script(self.call_tool)
        yield AssistantMessage(content=[TextBlock(text)], model="fake")
        yield ResultMessage(
            subtype="success",
            duration_ms=0,
            duration_api_ms=0,
            is_error=False,
            num_turns=1,
            session_id="s",
            result=text,
        )


def tool_text(result: dict[str, Any] | None) -> str:
    assert result is not None
    return "\n".join(block["text"] for block in result["content"])
