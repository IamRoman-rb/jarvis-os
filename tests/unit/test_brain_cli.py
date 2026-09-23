from typing import Any

import pytest
from claude_agent_sdk import ResultMessage
from typer.testing import CliRunner

from jarvis import cli
from jarvis.agent.brain import Brain, BrainError
from jarvis.core.config import JarvisConfig
from jarvis.policy.audit import AuditLog
from jarvis.tools.registry import build_tools
from tests.helpers import FakeClaudeClient, FakeConfirmer, ToolCaller, tool_text


async def test_brain_usa_tools_de_nivel_1_sin_preguntar(cfg: JarvisConfig, audit: AuditLog) -> None:
    async def script(call_tool: ToolCaller) -> str:
        info = tool_text(await call_tool("system_info", {}))
        return "Tenés esto: " + info.splitlines()[0]

    confirmer = FakeConfirmer(answer=False)
    brain = Brain(
        cfg,
        confirmer=confirmer,
        audit=audit,
        client_factory=lambda opts: FakeClaudeClient(opts, build_tools(cfg), script),
    )
    answer = await brain.ask("¿cómo está la máquina?")
    assert answer.startswith("Tenés esto: sistema:")
    assert confirmer.calls == []


class _ErrorClient(FakeClaudeClient):
    async def receive_response(self):  # type: ignore[no-untyped-def]
        yield ResultMessage(
            subtype="success",
            duration_ms=0,
            duration_api_ms=0,
            is_error=True,
            num_turns=1,
            session_id="s",
            api_error_status=529,
        )


async def test_brain_error_de_api(cfg: JarvisConfig, audit: AuditLog) -> None:
    async def script(_: ToolCaller) -> str:
        return ""

    brain = Brain(
        cfg,
        confirmer=FakeConfirmer(False),
        audit=audit,
        client_factory=lambda opts: _ErrorClient(opts, [], script),
    )
    with pytest.raises(BrainError, match="529"):
        await brain.ask("hola")


# --- CLI --------------------------------------------------------------------------------------


def test_cli_sin_api_key(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    result = CliRunner().invoke(cli.app, ["ask", "--texto", "hola"])
    assert result.exit_code == 2
    assert "ANTHROPIC_API_KEY" in result.output


def test_cli_ask_imprime_respuesta(monkeypatch: pytest.MonkeyPatch, cfg: JarvisConfig) -> None:
    monkeypatch.setenv("ANTHROPIC_API_KEY", "clave-de-prueba")
    monkeypatch.setattr(cli, "load_config", lambda _path: cfg)

    async def fake_ask(self: Any, text: str) -> str:
        return f"Recibí: {text}"

    monkeypatch.setattr(Brain, "ask", fake_ask)
    result = CliRunner().invoke(cli.app, ["ask", "--texto", "hola JARVIS"])
    assert result.exit_code == 0
    assert "Recibí: hola JARVIS" in result.output


def test_cli_error_de_brain(monkeypatch: pytest.MonkeyPatch, cfg: JarvisConfig) -> None:
    monkeypatch.setenv("ANTHROPIC_API_KEY", "clave-de-prueba")
    monkeypatch.setattr(cli, "load_config", lambda _path: cfg)

    async def fake_ask(self: Any, text: str) -> str:
        raise BrainError("No pude completar el pedido: success (HTTP 529).")

    monkeypatch.setattr(Brain, "ask", fake_ask)
    result = CliRunner().invoke(cli.app, ["ask", "--texto", "hola"])
    assert result.exit_code == 1
    assert "clave-de-prueba" not in result.output
