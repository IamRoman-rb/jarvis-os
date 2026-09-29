"""Proyectos: "JARVIS, abrí jarvis-os y seguí con lo que estábamos trabajando".

Un agente de código (el de Claude Code, por el Agent SDK) trabajando en una carpeta de la raíz
de proyectos del anfitrión. Con `seguir`, retoma la última conversación de Claude Code en esa
carpeta (`continue_conversation`). Su avance se ve en la ventana Proyecto de JARVIS-OS.

Permisos (docs/permisos.md):
- leer y buscar dentro del proyecto: sin confirmar;
- editar o crear archivos del proyecto: nivel 2;
- cualquier otra cosa (Bash, red, subagentes): nivel 3, con el comando entero en pantalla;
- nada fuera de la carpeta del proyecto, ni `.ssh`, `.gnupg`, credenciales o `/etc`.

Los ajustes de Claude Code (`settings.json`) no se cargan: sus reglas `allow` saltearían las
confirmaciones. El CLAUDE.md del proyecto sí se le pasa al agente.
"""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any, Protocol

from jarvis.config import Config
from jarvis.policy.audit import audit

#: Lo que el agente de proyecto puede hacer sin preguntar.
READ_ONLY = {"Read", "Glob", "Grep", "LS", "TodoWrite"}
#: Editar archivos del proyecto: nivel 2.
EDITS = {"Edit", "MultiEdit", "Write", "NotebookEdit"}
#: Nunca, aunque estén dentro de la raíz.
SENSITIVE = (".ssh", ".gnupg", ".aws", ".azure", ".kube", "credentials", "/etc/", "\\etc\\")

CONTINUE = "Seguí con lo que estábamos trabajando."

Emit = Callable[[str, str], Awaitable[None]]
Confirm = Callable[[str, int, str], Awaitable[bool]]


class ProjectError(ValueError):
    """Un proyecto que no existe o no se puede abrir."""


def list_projects(root: Path) -> list[str]:
    if not root.is_dir():
        return []
    return sorted(p.name for p in root.iterdir() if p.is_dir() and not p.name.startswith("."))


def resolve_project(root: Path, name: str) -> Path:
    """La carpeta del proyecto `name` (sin distinguir mayúsculas; si no, la única que lo
    contiene). Siempre dentro de `root`."""
    wanted = name.strip().lower()
    if not wanted or "/" in wanted or "\\" in wanted or wanted.startswith("."):
        raise ProjectError(f"nombre de proyecto inválido: {name!r}")
    names = list_projects(root)
    exact = [n for n in names if n.lower() == wanted]
    partial = [n for n in names if wanted in n.lower()]
    match exact or partial:
        case [one]:
            path = (root / one).resolve()
        case []:
            raise ProjectError(f'No hay un proyecto "{name}" en {root}.')
        case many:
            raise ProjectError(f'"{name}" puede ser: {", ".join(many)}.')
    if not path.is_relative_to(root.resolve()):
        raise ProjectError("El proyecto está fuera de la carpeta de proyectos.")
    return path


def path_allowed(project: Path, raw: str) -> bool:
    """Una ruta que usa el agente: dentro del proyecto y nada sensible."""
    if any(s in raw.replace("\\", "/").lower() for s in SENSITIVE):
        return False
    p = Path(raw)
    p = (p if p.is_absolute() else project / p).resolve()
    return p.is_relative_to(project.resolve())


def describe_tool(name: str, args: dict[str, Any]) -> str:
    """Lo que se muestra al confirmar: completo."""
    if name == "Bash":
        return f"Ejecutar en el anfitrión: {args.get('command', '')}"
    if name in EDITS:
        return f"{name} {args.get('file_path', args.get('notebook_path', '?'))}"
    return f"{name}: {json.dumps(args, ensure_ascii=False)}"


def decide(project: Path, name: str, args: dict[str, Any]) -> int | None:
    """El nivel que necesita una tool del agente de proyecto (None = prohibida)."""
    paths = [str(v) for k, v in args.items() if k in ("file_path", "notebook_path", "path")]
    if any(not path_allowed(project, p) for p in paths):
        return None
    text = json.dumps(args, ensure_ascii=False).lower()
    if any(s in text for s in SENSITIVE):
        return None
    if name in READ_ONLY:
        return 1
    if name in EDITS:
        return 2
    return 3


class ProjectRunner(Protocol):
    name: str

    async def run(self, emit: Emit, confirm: Confirm) -> None: ...

    async def stop(self) -> None: ...


class ClaudeProject:
    def __init__(self, path: Path, request: str, keep_going: bool, config: Config) -> None:
        self.path = path
        self.name = path.name
        self.request = request or CONTINUE
        self.keep_going = keep_going
        self.config = config
        self._client: Any = None

    def _system_prompt(self) -> Any:
        extra = (
            f"Trabajás en el proyecto {self.name} de Roman, a pedido de JARVIS. Respondé en "
            "español rioplatense. Cada edición y cada comando los aprueba Roman en su pantalla."
        )
        claude_md = self.path / "CLAUDE.md"
        if claude_md.is_file():
            extra += "\n\n# CLAUDE.md del proyecto\n" + claude_md.read_text(encoding="utf-8")
        return {"type": "preset", "preset": "claude_code", "append": extra}

    async def run(self, emit: Emit, confirm: Confirm) -> None:
        from claude_agent_sdk import (
            AssistantMessage,
            ClaudeAgentOptions,
            ClaudeSDKClient,
            PermissionResultAllow,
            PermissionResultDeny,
            ResultMessage,
            TextBlock,
            ToolUseBlock,
        )

        async def can_use_tool(tool: str, args: dict[str, Any], _ctx: Any) -> Any:
            level = decide(self.path, tool, args)
            desc = describe_tool(tool, args)
            if level is None:
                audit("rechazada", f"proyecto:{tool}", args, motivo="fuera del proyecto")
                return PermissionResultDeny(message="Fuera del proyecto o sensible: prohibido.")
            ok = level == 1 or await confirm(f"proyecto:{tool}", level, f"[{self.name}] {desc}")
            audit("aprobada" if ok else "rechazada", f"proyecto:{tool}", args, nivel=level)
            if ok:
                return PermissionResultAllow()
            return PermissionResultDeny(message="Roman rechazó la acción.")

        options = ClaudeAgentOptions(
            system_prompt=self._system_prompt(),
            model=self.config.modelo,
            cwd=str(self.path),
            continue_conversation=self.keep_going,
            allowed_tools=[],
            can_use_tool=can_use_tool,
            # Nunca bypassPermissions ni acceptEdits: cada cambio lo aprueba Roman.
            permission_mode="default",
            setting_sources=[],
        )
        self._client = ClaudeSDKClient(options=options)
        await emit("inicio", f"{self.name}: {self.request}")
        try:
            await self._client.connect()
            await self._client.query(self.request)
            async for msg in self._client.receive_response():
                if isinstance(msg, AssistantMessage):
                    for block in msg.content:
                        if isinstance(block, TextBlock) and block.text.strip():
                            await emit("texto", block.text)
                        elif isinstance(block, ToolUseBlock):
                            await emit("herramienta", describe_tool(block.name, block.input))
                elif isinstance(msg, ResultMessage):
                    await emit("error" if msg.is_error else "fin", str(msg.result or "Listo."))
        finally:
            await self._client.disconnect()

    async def stop(self) -> None:
        if self._client is not None:
            await self._client.interrupt()


class ScriptedProject:
    """Sin Claude: para `cargo xtask test`. Pide confirmar una edición (nivel 2)."""

    def __init__(self, path: Path, request: str, keep_going: bool) -> None:
        self.path = path
        self.name = path.name
        self.request = request or CONTINUE

    async def run(self, emit: Emit, confirm: Confirm) -> None:
        await emit("inicio", f"{self.name}: {self.request}")
        await emit("texto", "Leo dónde habíamos quedado.")
        await emit("herramienta", "Read CLAUDE.md")
        ok = await confirm("proyecto:Edit", 2, f"[{self.name}] Edit README.md")
        await emit("texto", "Edité el README." if ok else "No toqué nada.")
        await emit("fin", "Listo.")

    async def stop(self) -> None:
        pass
