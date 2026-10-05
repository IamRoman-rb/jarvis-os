"""Las tools de JARVIS sobre JARVIS-OS.

No hacen nada acá: le piden al kernel que ejecute la acción (`accion`) y devuelven lo que
contesta (`resultado`). Las de nivel 2 y 3 pasan antes por `Gate.permit`, que pide la
confirmación en la pantalla de JARVIS (docs/permisos.md).
"""

from __future__ import annotations

from typing import Any, Protocol

from jarvis.policy.audit import audit
from jarvis.policy.levels import LEVELS, UnknownToolError, bare, level_of


class Kernel(Protocol):
    """La conexión con el kernel (la implementa `service.server.Session`)."""

    async def call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]: ...

    async def confirm(self, tool: str, level: int, description: str) -> bool: ...


# (nombre, descripción para Claude, parámetros)
SPECS: list[tuple[str, str, dict[str, type]]] = [
    ("estado_sistema", "Estado de la máquina: CPU, memoria, disco, red y ventanas.", {}),
    ("ventanas_abiertas", "Las ventanas abiertas en JARVIS-OS.", {}),
    (
        "listar_archivos",
        "Lista una carpeta del disco de JARVIS-OS (ej. /Documentos, /Descargas).",
        {"ruta": str},
    ),
    ("leer_archivo", "Lee un archivo de texto del disco de JARVIS-OS.", {"ruta": str}),
    ("buscar_archivos", "Busca archivos por nombre en todo el disco.", {"nombre": str}),
    (
        "abrir_archivo",
        "Abre un archivo o carpeta de JARVIS-OS con su app, como el doble clic: los textos en "
        "el editor, las imágenes en el visor, las carpetas en Archivos, las .html en el "
        "navegador. Usala cuando Roman pide ver, mostrar o abrir un archivo.",
        {"ruta": str},
    ),
    (
        "abrir_app",
        "Abre una app vacía: archivos, monitor, música, navegador (Brave), editor, terminal, "
        "configuración, consola, visor. Para un archivo puntual usá abrir_archivo.",
        {"app": str},
    ),
    (
        "abrir_web",
        "Abre una dirección en el navegador de JARVIS-OS (para que Roman la vea).",
        {"url": str},
    ),
    (
        "buscar_web",
        "Abre una búsqueda en el navegador de JARVIS-OS (para que Roman la vea). No te devuelve "
        "los resultados: para saber algo vos, usá WebSearch o WebFetch.",
        {"consulta": str},
    ),
    (
        "escribir_archivo",
        "Crea o reemplaza un archivo de texto (crea las carpetas que falten).",
        {"ruta": str, "contenido": str},
    ),
    ("crear_carpeta", "Crea una carpeta.", {"ruta": str}),
    ("copiar", "Copia un archivo a una carpeta.", {"origen": str, "destino": str}),
    ("mover", "Mueve un archivo o carpeta a otra carpeta.", {"origen": str, "destino": str}),
    ("cerrar_ventana", "Cierra la ventana de una app.", {"app": str}),
    ("a_papelera", "Mueve un archivo o carpeta a la Papelera (nunca borra).", {"ruta": str}),
    ("listar_proyectos", "Los proyectos de código de Roman (carpetas en su PC).", {}),
    (
        "abrir_proyecto",
        "Abre un proyecto de código con un agente que trabaja en él (se ve en la ventana "
        "Proyecto). seguir=true retoma la última conversación de Claude Code en esa carpeta; "
        "pedido es qué hacer (vacío = seguir con lo que estaban).",
        {"nombre": str, "seguir": bool, "pedido": str},
    ),
    (
        "ejecutar_comando",
        "Ejecuta un comando en la terminal de JARVIS-OS (jsh: ls, cat, echo, apt install, snap, "
        "winget, open…) y devuelve lo último que muestra. Si sigue corriendo (descargas), "
        "mirá cómo terminó con leer_terminal.",
        {"comando": str},
    ),
    ("leer_terminal", "Lo último que muestra la terminal de JARVIS-OS.", {}),
    (
        "modificar_sistema",
        "Modifica el propio JARVIS-OS (su interfaz, apps, drivers, el kernel o tu cerebro): un "
        "agente de código trabaja en el repositorio del sistema y se ve en la ventana Proyecto; "
        "cada edición y comando los aprueba Roman. pedido = qué cambiar, con todo el detalle "
        "que dio Roman. Cuando termine y Roman quiera verlo, usá aplicar_cambios_sistema.",
        {"pedido": str},
    ),
    (
        "aplicar_cambios_sistema",
        "Aplica lo que cambió modificar_sistema: verifica que compile y, si compila, JARVIS-OS "
        "se reinicia con la versión nueva (si no, devuelve el error y no reinicia).",
        {},
    ),
    (
        "recordar",
        "Anota algo en tu memoria para recordarlo en conversaciones futuras: datos de Roman, "
        "sus preferencias, fechas, decisiones. Una frase clara y completa.",
        {"dato": str},
    ),
    (
        "buscar_memoria",
        "Busca en tu memoria (recuerdos y conversaciones pasadas) con palabras clave: para "
        '"¿te acordás de...?", "¿qué hablamos de...?" o algo que Roman ya te contó.',
        {"consulta": str},
    ),
    (
        "olvidar",
        'Borra de tu memoria los recuerdos sobre algo (o "todo": recuerdos y conversaciones).',
        {"que": str},
    ),
    (
        "consultar_agente",
        "Le pregunta algo a otro agente de IA que Roman vinculó (claude, gemini, chatgpt o "
        "deepseek) y devuelve su respuesta: para una segunda opinión o algo que otro sepa mejor.",
        {"agente": str, "pregunta": str},
    ),
]

if {s[0] for s in SPECS} != set(LEVELS):
    raise RuntimeError("cada tool tiene que tener su nivel en policy/levels.py")


def describe(tool: str, args: dict[str, Any]) -> str:
    """Lo que se muestra al pedir la confirmación: completo, sin recortar."""
    a = {k: str(v) for k, v in args.items()}
    match bare(tool):
        case "escribir_archivo":
            n = len(a.get("contenido", ""))
            return f"Escribir el archivo {a.get('ruta', '?')} ({n} caracteres)."
        case "crear_carpeta":
            return f"Crear la carpeta {a.get('ruta', '?')}."
        case "copiar":
            return f"Copiar {a.get('origen', '?')} a {a.get('destino', '?')}."
        case "mover":
            return f"Mover {a.get('origen', '?')} a {a.get('destino', '?')}."
        case "modificar_sistema":
            return f"Modificar JARVIS-OS: {a.get('pedido', '?')}"
        case "aplicar_cambios_sistema":
            return "Compilar los cambios y reiniciar JARVIS-OS con la versión nueva."
        case "olvidar":
            return f"Borrar de la memoria de JARVIS: {a.get('que', '?')}"
        case "abrir_proyecto":
            what = a.get("pedido") or "seguir con lo que estaban trabajando"
            return f"Abrir el proyecto {a.get('nombre', '?')} y {what}."
        case "cerrar_ventana":
            return f"Cerrar la ventana de {a.get('app', '?')}."
        case "a_papelera":
            return f"Mover a la Papelera: {a.get('ruta', '?')}."
        case "ejecutar_comando":
            return f"Ejecutar en la terminal: {a.get('comando', '')}"
        case other:
            return f"{other}: {a}"


class Gate:
    """Los permisos: el nivel 1 pasa, el 2 y el 3 piden confirmación, lo desconocido no."""

    def __init__(self, kernel: Kernel) -> None:
        self.kernel = kernel

    async def permit(self, tool: str, args: dict[str, Any]) -> bool:
        try:
            level = level_of(tool)
        except UnknownToolError:
            audit("rechazada", tool, args, motivo="sin nivel declarado")
            return False
        if level == 1:
            audit("aprobada", bare(tool), args, nivel=1)
            return True
        ok = await self.kernel.confirm(bare(tool), level, describe(tool, args))
        audit("aprobada" if ok else "rechazada", bare(tool), args, nivel=level)
        return ok

    async def run(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        """Permiso + ejecución (lo usa el cerebro simulado; con Claude, el SDK llama a
        `permit` desde `can_use_tool` y después al handler de la tool)."""
        if not await self.permit(tool, args):
            return False, "El usuario rechazó la acción."
        return await self.kernel.call(bare(tool), args)


def build_server(kernel: Kernel) -> Any:
    """El servidor MCP "jarvis" con todas las tools, en el mismo proceso."""
    from claude_agent_sdk import create_sdk_mcp_server, tool

    def make(name: str, desc: str, schema: dict[str, type]) -> Any:
        @tool(name, desc, schema)
        async def handler(args: dict[str, Any]) -> dict[str, Any]:
            ok, data = await kernel.call(name, args)
            return {"content": [{"type": "text", "text": data}], "is_error": not ok}

        return handler

    return create_sdk_mcp_server(
        name="jarvis", version="0.1.0", tools=[make(n, d, s) for n, d, s in SPECS]
    )
