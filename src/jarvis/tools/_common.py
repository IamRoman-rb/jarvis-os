"""Utilidades compartidas por las tools: formato de respuesta MCP y ejecución de procesos."""

import asyncio
from dataclasses import dataclass
from typing import Any

ToolResult = dict[str, Any]


def ok(text: str) -> ToolResult:
    return {"content": [{"type": "text", "text": text}]}


def error(text: str) -> ToolResult:
    return {"content": [{"type": "text", "text": text}], "is_error": True}


@dataclass(frozen=True)
class ProcResult:
    returncode: int
    stdout: str
    stderr: str


async def run(args: list[str], timeout_s: float = 15.0) -> ProcResult:
    """Ejecuta un programa SIN shell: ``args`` es una lista, nunca un string interpretado.

    Así un nombre de archivo como ``a; rm -rf ~`` es solo un argumento más, no un comando.
    """
    proc = await asyncio.create_subprocess_exec(
        *args,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout_s)
    except TimeoutError:
        proc.kill()
        await proc.wait()
        return ProcResult(-1, "", f"{args[0]} no respondió en {timeout_s:.0f} s.")
    return ProcResult(
        proc.returncode or 0, out.decode(errors="replace"), err.decode(errors="replace")
    )
