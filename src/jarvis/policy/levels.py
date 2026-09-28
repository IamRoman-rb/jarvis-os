"""El nivel de permiso de cada tool de JARVIS (fuente de verdad: docs/permisos.md).

1. Sin confirmación: solo lectura, o abrir algo (una app, una página).
2. Confirmación en la pantalla de JARVIS: cambios reversibles.
3. Confirmación obligatoria con un clic (el teclado y la voz solo pueden rechazar): borrar,
   instalar, comandos.

Una tool que no está acá no se registra ni se puede usar (default deny).
"""

from __future__ import annotations

#: Prefijo con el que el Agent SDK nombra las tools del servidor MCP "jarvis".
MCP_PREFIX = "mcp__jarvis__"

LEVELS: dict[str, int] = {
    "estado_sistema": 1,
    "ventanas_abiertas": 1,
    "listar_archivos": 1,
    "leer_archivo": 1,
    "buscar_archivos": 1,
    "abrir_app": 1,
    "abrir_web": 1,
    "buscar_web": 1,
    "escribir_archivo": 2,
    "crear_carpeta": 2,
    "copiar": 2,
    "mover": 2,
    "cerrar_ventana": 2,
    "a_papelera": 3,
    "ejecutar_comando": 3,
}


class UnknownToolError(KeyError):
    """Una tool sin nivel declarado: no se puede usar."""


def bare(name: str) -> str:
    """`mcp__jarvis__abrir_app` → `abrir_app`."""
    return name.removeprefix(MCP_PREFIX)


def level_of(name: str) -> int:
    try:
        return LEVELS[bare(name)]
    except KeyError:
        raise UnknownToolError(name) from None


def auto_approved() -> list[str]:
    """Las de nivel 1, con el prefijo del SDK (van en `allowed_tools`)."""
    return [MCP_PREFIX + n for n, lvl in LEVELS.items() if lvl == 1]
