"""El servidor MCP de las herramientas de JARVIS para Gemini CLI y Codex (ver `toolbridge`).

Lo lanzan esos programas (`python -m jarvis.agent.mcp_proxy`) y habla MCP por stdio. Ofrece las
mismas herramientas que tiene Claude (`tools.system.SPECS`) y le pasa cada llamada al puente del
cerebro, que es el que decide (permisos y confirmaciones). Sin el puente no hace nada.
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

from mcp import types
from mcp.server.lowlevel import Server
from mcp.server.stdio import stdio_server

from jarvis.agent import toolbridge
from jarvis.tools.system import SPECS


def schema(params: dict[str, type]) -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            k: {"type": "boolean" if t is bool else "string"} for k, t in params.items()
        },
        "required": list(params),
    }


def tools() -> list[types.Tool]:
    return [types.Tool(name=n, description=d, input_schema=schema(p)) for n, d, p in SPECS]


async def list_tools(_ctx: Any, _params: Any) -> types.ListToolsResult:
    return types.ListToolsResult(tools=tools())


async def call_tool(_ctx: Any, params: types.CallToolRequestParams) -> types.CallToolResult:
    address = os.environ.get(toolbridge.ENV, "")
    if not address:
        ok, data = False, "JARVIS no está conectado (falta el puente del cerebro)."
    else:
        try:
            ok, data = await toolbridge.call(address, params.name, dict(params.arguments or {}))
        except (OSError, ValueError) as e:
            ok, data = False, f"no se pudo hablar con el cerebro de JARVIS: {e}"
    return types.CallToolResult(
        content=[types.TextContent(type="text", text=data)], is_error=not ok
    )


def server() -> Server[Any]:
    return Server(
        "jarvis",
        instructions="Las herramientas de JARVIS-OS: actúan sobre el sistema operativo de Roman.",
        on_list_tools=list_tools,
        on_call_tool=call_tool,
    )


async def main() -> None:
    s = server()
    async with stdio_server() as (read, write):
        await s.run(read, write, s.create_initialization_options())


if __name__ == "__main__":
    asyncio.run(main())
