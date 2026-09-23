"""Niveles de permiso de cada tool. Espejo de docs/permisos.md (hay un test que los compara).

Default deny: una tool que no figura acá no se puede registrar ni ejecutar.
"""

from enum import IntEnum, StrEnum

MCP_SERVER_NAME = "system"
MCP_PREFIX = f"mcp__{MCP_SERVER_NAME}__"


class Level(IntEnum):
    READ_ONLY = 1
    """Sin efectos: se aprueba sola."""
    REVERSIBLE = 2
    """Efecto visible pero reversible: confirmación liviana."""
    CRITICAL = 3
    """Destructivo o privilegiado: confirmación explícita, nunca desde el celular."""


class Origin(StrEnum):
    """De dónde viene el pedido."""

    LOCAL = "local"
    ANDROID = "android"


TOOL_LEVELS: dict[str, Level] = {
    "find_files": Level.READ_ONLY,
    "system_info": Level.READ_ONLY,
    "open_app": Level.REVERSIBLE,
    "trash_file": Level.CRITICAL,
}


class UnknownToolError(LookupError):
    """La tool no tiene nivel declarado."""


def bare_name(tool_name: str) -> str:
    """``mcp__system__find_files`` → ``find_files``. Otros nombres quedan igual."""
    return tool_name.removeprefix(MCP_PREFIX)


def mcp_name(tool_name: str) -> str:
    """``find_files`` → ``mcp__system__find_files`` (el nombre que ve el SDK)."""
    return f"{MCP_PREFIX}{bare_name(tool_name)}"


def level_of(tool_name: str) -> Level:
    """Nivel de una tool. Solo acepta tools de nuestro servidor MCP; el resto es desconocido."""
    if tool_name.startswith("mcp__") and not tool_name.startswith(MCP_PREFIX):
        raise UnknownToolError(tool_name)
    try:
        return TOOL_LEVELS[bare_name(tool_name)]
    except KeyError:
        raise UnknownToolError(tool_name) from None
