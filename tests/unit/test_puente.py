"""El puente de herramientas: Gemini CLI y Codex usan las tools de JARVIS por MCP. Acá un
cliente MCP de verdad lanza `mcp_proxy` (como lo hacen ellos) y llama una tool, que llega al
puente del cerebro."""

import os
import sys
from typing import Any

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from jarvis.agent import toolbridge
from jarvis.agent.toolbridge import ToolBridge


async def test_una_tool_por_mcp_llega_al_cerebro() -> None:
    ran: list[tuple[str, dict[str, Any]]] = []

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        ran.append((name, args))
        return (True, "abierta") if name == "abrir_app" else (False, "Roman lo rechazó")

    async with ToolBridge(run_tool) as bridge:
        params = StdioServerParameters(
            command=sys.executable,
            args=["-m", "jarvis.agent.mcp_proxy"],
            env={**os.environ, toolbridge.ENV: bridge.address},
        )
        async with (
            stdio_client(params) as (read, write),
            ClientSession(read, write) as session,
        ):
            await session.initialize()
            names = [t.name for t in (await session.list_tools()).tools]
            assert "abrir_app" in names and "estado_sistema" in names
            r = await session.call_tool("abrir_app", {"app": "monitor"})
            assert not r.is_error and r.content[0].text == "abierta"  # type: ignore[union-attr]
            r = await session.call_tool("a_papelera", {"ruta": "/x"})
            assert r.is_error and "rechazó" in r.content[0].text  # type: ignore[union-attr]
    assert ran == [("abrir_app", {"app": "monitor"}), ("a_papelera", {"ruta": "/x"})]
    assert bridge.calls == [("abrir_app", True), ("a_papelera", False)]


async def test_sin_el_token_no_se_puede_usar() -> None:
    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        if name == "rompe":
            raise RuntimeError("se rompió")
        return True, "ok"

    async with ToolBridge(run_tool) as bridge:
        ok, data = await toolbridge.call(f"{bridge.port}:otro-token", "abrir_app", {})
        assert not ok and data == "token inválido"
        assert bridge.calls == [], "sin el token no llega a ninguna tool"
        assert await toolbridge.call(bridge.address, "abrir_app", {}) == (True, "ok")
        # Si la tool falla, el agente se entera (no se corta la conexión).
        assert await toolbridge.call(bridge.address, "rompe", {}) == (False, "falla: se rompió")
    assert bridge.calls == [("abrir_app", True), ("rompe", False)]
