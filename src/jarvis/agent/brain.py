"""Cerebro de JARVIS: conecta Claude (Agent SDK) con nuestras tools y la política de permisos.

Decisiones clave (ver docs/adr/0002-agent-sdk.md):

- ``tools=[]``: sin tools built-in (Bash, Edit…). Claude solo actúa con nuestras tools MCP.
- ``setting_sources=[]``: no se carga ``~/.claude/settings.json``. Si se cargara, una regla
  ``allow`` escrita para Claude Code podría aprobar tools de nivel 2/3 sin pasar por la política.
- ``allowed_tools=[]``: TODAS las tools pasan por ``can_use_tool`` → ``policy.gate``, que aprueba
  sola las de nivel 1 y pide confirmación para 2 y 3. Un único punto de decisión (ver ADR 0002).
- Hook ``PreToolUse`` = auditoría. Devuelve ``{}`` (sin decisión): si devolviera "allow",
  el SDK saltearía ``can_use_tool``.
- ``permission_mode="default"``. Nunca ``bypassPermissions`` ni ``acceptEdits``.
"""

from collections.abc import Callable
from typing import Any

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ClaudeSDKClient,
    HookMatcher,
    PermissionResultAllow,
    PermissionResultDeny,
    ResultMessage,
    TextBlock,
    ToolPermissionContext,
)
from claude_agent_sdk.types import HookContext, HookInput, HookJSONOutput, PermissionResult

from jarvis.agent.prompts import build_system_prompt
from jarvis.core.config import JarvisConfig
from jarvis.policy.audit import AuditEvent, AuditLog
from jarvis.policy.confirm import Confirmer
from jarvis.policy.gate import decide
from jarvis.policy.levels import (
    MCP_SERVER_NAME,
    Origin,
    UnknownToolError,
    bare_name,
    level_of,
)
from jarvis.tools.registry import build_system_server

ClientFactory = Callable[[ClaudeAgentOptions], ClaudeSDKClient]


class BrainError(RuntimeError):
    """Claude no pudo completar el pedido. El mensaje es apto para mostrar al usuario."""


def build_options(
    cfg: JarvisConfig,
    *,
    origin: Origin,
    confirmer: Confirmer,
    audit: AuditLog,
) -> ClaudeAgentOptions:
    async def can_use_tool(
        tool_name: str, tool_input: dict[str, Any], context: ToolPermissionContext
    ) -> PermissionResult:
        decision = await decide(
            tool_name, tool_input, origin=origin, confirmer=confirmer, audit=audit
        )
        if decision.allowed:
            return PermissionResultAllow()
        return PermissionResultDeny(message=decision.reason)

    async def audit_hook(
        hook_input: HookInput, tool_use_id: str | None, context: HookContext
    ) -> HookJSONOutput:
        if hook_input["hook_event_name"] == "PreToolUse":
            name = hook_input["tool_name"]
            try:
                level: int | None = int(level_of(name))
            except UnknownToolError:
                level = None
            await audit.record(
                origin=origin,
                tool=bare_name(name),
                level=level,
                tool_input=hook_input["tool_input"],
                event=AuditEvent.REQUESTED,
            )
        return {}  # sin decisión: que siga el flujo normal de permisos

    return ClaudeAgentOptions(
        system_prompt=build_system_prompt(origin, [str(d) for d in cfg.allowed_dirs]),
        model=cfg.model,
        tools=[],
        setting_sources=[],
        mcp_servers={MCP_SERVER_NAME: build_system_server(cfg)},
        allowed_tools=[],  # nada se autoaprueba fuera de policy.gate
        can_use_tool=can_use_tool,
        hooks={"PreToolUse": [HookMatcher(matcher=None, hooks=[audit_hook])]},
        permission_mode="default",
        max_turns=cfg.max_turns,
        max_budget_usd=cfg.max_budget_usd,
    )


def _default_client_factory(options: ClaudeAgentOptions) -> ClaudeSDKClient:
    return ClaudeSDKClient(options=options)


class Brain:
    """Un pedido = una conversación nueva. La memoria entre pedidos llega en fases futuras."""

    def __init__(
        self,
        cfg: JarvisConfig,
        *,
        confirmer: Confirmer,
        audit: AuditLog | None = None,
        origin: Origin = Origin.LOCAL,
        client_factory: ClientFactory = _default_client_factory,
    ) -> None:
        self.cfg = cfg
        self.origin = origin
        self.confirmer = confirmer
        self.audit = audit or AuditLog(cfg.audit_db)
        self._client_factory = client_factory

    async def ask(self, text: str) -> str:
        """Manda el pedido a Claude y devuelve la respuesta final en texto."""
        options = build_options(
            self.cfg, origin=self.origin, confirmer=self.confirmer, audit=self.audit
        )
        texts: list[str] = []
        result: ResultMessage | None = None
        async with self._client_factory(options) as client:
            await client.query(text)
            async for message in client.receive_response():
                if isinstance(message, AssistantMessage):
                    texts += [b.text for b in message.content if isinstance(b, TextBlock)]
                elif isinstance(message, ResultMessage):
                    result = message

        if result is not None and result.is_error:
            detail = f" (HTTP {result.api_error_status})" if result.api_error_status else ""
            raise BrainError(f"No pude completar el pedido: {result.subtype}{detail}.")
        if result is not None and result.result:
            return result.result
        return texts[-1] if texts else ""
