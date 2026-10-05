"""El puente de herramientas: lo que conecta a Gemini y ChatGPT (vinculados con Google) con el
cerebro de JARVIS.

Esos agentes corren como programas del anfitrión (Gemini CLI, Codex). Ambos saben usar
herramientas por MCP: se les configura un servidor MCP (`mcp_proxy`), que ellos mismos lanzan, y
ese servidor le pasa cada llamada a este puente. El puente la corre con el mismo `Gate` que usa
Claude: los mismos niveles de permiso, las mismas confirmaciones y el mismo registro.

```text
Gemini CLI / Codex ──MCP (stdio)──▶ mcp_proxy ──TCP 127.0.0.1 + token──▶ ToolBridge ──▶ Gate
```

El puente vive solo mientras dura la respuesta del agente, escucha solo en 127.0.0.1 y exige un
token aleatorio de esa respuesta (otro programa de la PC no puede usarlo).
"""

from __future__ import annotations

import asyncio
import hmac
import json
import secrets
from collections.abc import Awaitable, Callable
from typing import Any

#: La variable de entorno con la que `mcp_proxy` encuentra el puente: "puerto:token".
ENV = "JARVIS_PUENTE"
#: Un pedido no puede ser más largo que esto.
MAX_LINE = 1024 * 1024

ToolRunner = Callable[[str, dict[str, Any]], Awaitable[tuple[bool, str]]]


class ToolBridge:
    """`async with ToolBridge(run_tool) as bridge:` y `bridge.address` va en `ENV`."""

    def __init__(self, run_tool: ToolRunner) -> None:
        self._run_tool = run_tool
        self.token = secrets.token_hex(16)
        self.port = 0
        self._server: asyncio.Server | None = None
        #: Las llamadas que pasaron (nombre, ok), para el registro y los tests.
        self.calls: list[tuple[str, bool]] = []

    @property
    def address(self) -> str:
        return f"{self.port}:{self.token}"

    async def __aenter__(self) -> ToolBridge:
        self._server = await asyncio.start_server(self._handle, "127.0.0.1", 0, limit=MAX_LINE)
        self.port = self._server.sockets[0].getsockname()[1]
        return self

    async def __aexit__(self, *exc: object) -> None:
        if self._server is not None:
            self._server.close()
            await self._server.wait_closed()

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while line := await reader.readline():
                writer.write(json.dumps(await self._answer(line)).encode() + b"\n")
                await writer.drain()
        except (ConnectionError, ValueError):
            pass
        finally:
            writer.close()

    async def _answer(self, line: bytes) -> dict[str, Any]:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            return {"ok": False, "datos": "pedido inválido"}
        if not isinstance(msg, dict) or not hmac.compare_digest(
            str(msg.get("token", "")), self.token
        ):
            return {"ok": False, "datos": "token inválido"}
        name = str(msg.get("tool", ""))
        args = msg.get("args")
        try:
            ok, data = await self._run_tool(name, args if isinstance(args, dict) else {})
        except Exception as e:  # el agente tiene que enterarse de cualquier falla
            ok, data = False, f"falla: {e}"
        self.calls.append((name, ok))
        return {"ok": ok, "datos": data}


async def call(address: str, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
    """Una llamada al puente (la usa `mcp_proxy`)."""
    port, _, token = address.partition(":")
    reader, writer = await asyncio.open_connection("127.0.0.1", int(port), limit=MAX_LINE)
    try:
        writer.write(json.dumps({"token": token, "tool": tool, "args": args}).encode() + b"\n")
        await writer.drain()
        reply = json.loads(await reader.readline())
        return bool(reply.get("ok")), str(reply.get("datos", ""))
    finally:
        writer.close()
