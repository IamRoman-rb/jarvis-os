"""Registro de tools: une cada handler con su nivel y lo expone como servidor MCP in-process.

Si una tool no tiene nivel declarado en ``policy/levels.py``, el registro falla (default deny).
"""

from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from typing import Any

from claude_agent_sdk import SdkMcpTool, ToolAnnotations, create_sdk_mcp_server, tool
from claude_agent_sdk.types import McpSdkServerConfig

from jarvis.core.config import JarvisConfig
from jarvis.policy.levels import MCP_SERVER_NAME, Level, level_of
from jarvis.tools import apps, files, system
from jarvis.tools._common import ToolResult

Handler = Callable[[dict[str, Any], JarvisConfig], Awaitable[ToolResult]]


@dataclass(frozen=True)
class ToolSpec:
    name: str
    description: str
    input_schema: dict[str, Any]
    handler: Handler


SPECS: tuple[ToolSpec, ...] = (
    ToolSpec(
        "find_files",
        "Busca archivos por nombre dentro de las carpetas permitidas del usuario. "
        "Acepta comodines (*.pdf) o parte del nombre.",
        {"pattern": str},
        files.find_files,
    ),
    ToolSpec(
        "system_info",
        "Devuelve el estado de la máquina: sistema, CPU, RAM, disco y tiempo encendida.",
        {},
        system.system_info,
    ),
    ToolSpec(
        "open_app",
        "Abre una aplicación instalada por su id .desktop (por ejemplo firefox-esr).",
        {"app_id": str},
        apps.open_app,
    ),
    ToolSpec(
        "trash_file",
        "Mueve un archivo a la papelera (nunca lo borra definitivamente). Recibe la ruta absoluta.",
        {"path": str},
        files.trash_file,
    ),
)


def _annotations(level: Level) -> ToolAnnotations:
    return ToolAnnotations(
        readOnlyHint=level is Level.READ_ONLY,
        destructiveHint=level is Level.CRITICAL,
    )


def build_tools(cfg: JarvisConfig, specs: Sequence[ToolSpec] = SPECS) -> list[SdkMcpTool[Any]]:
    """Convierte las specs en tools del SDK. Lanza ``UnknownToolError`` si falta un nivel."""
    built: list[SdkMcpTool[Any]] = []
    for spec in specs:
        level = level_of(spec.name)  # default deny: sin nivel, no se registra

        def _bind(spec: ToolSpec) -> Callable[[dict[str, Any]], Awaitable[ToolResult]]:
            async def handler(args: dict[str, Any]) -> ToolResult:
                return await spec.handler(args, cfg)

            return handler

        decorator = tool(spec.name, spec.description, spec.input_schema, _annotations(level))
        built.append(decorator(_bind(spec)))
    return built


def build_system_server(cfg: JarvisConfig) -> McpSdkServerConfig:
    return create_sdk_mcp_server(name=MCP_SERVER_NAME, version="0.1.0", tools=build_tools(cfg))
