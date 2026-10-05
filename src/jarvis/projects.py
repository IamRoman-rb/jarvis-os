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

    def decide(self, name: str, args: dict[str, Any]) -> int | None:
        return decide(self.path, name, args)

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
            level = self.decide(tool, args)
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


# --- Modificar JARVIS-OS desde adentro ------------------------------------------------------

#: Comandos que solo leen: sin confirmar (el grafo de graphify y el estado de git).
OS_READ_COMMANDS = (
    "graphify query ",
    "graphify path ",
    "graphify explain ",
    "git status",
    "git diff",
    "git log",
    "git show",
)
#: Comandos que verifican (compilan, prueban, revisan): nivel 2, se aprueban con el teclado.
OS_CHECK_COMMANDS = (
    "cargo test",
    "cargo check",
    "cargo clippy",
    "cargo build",
    "cargo fmt",
    "uv run pytest",
    "uv run ruff",
    "uv run mypy",
)
#: Si el comando tiene alguno, puede encadenar otro: vuelve a nivel 3.
SHELL_META = (";", "&", "|", ">", "<", "`", "$(", "\n")


def os_command_level(command: str) -> int:
    """El nivel de un comando del agente que modifica JARVIS-OS (los demás proyectos: 3)."""
    cmd = command.strip()
    if any(m in cmd for m in SHELL_META):
        return 3
    # `cd kernel && cargo test` no pasa (tiene &): el agente usa rutas o --manifest-path.
    if cmd.startswith(OS_READ_COMMANDS):
        # `git diff --output=archivo` escribe: ya no es solo leer.
        return 3 if "--output" in cmd or " -o" in cmd else 1
    if cmd.startswith(OS_CHECK_COMMANDS):
        return 2
    return 3


OS_PROMPT = """\
Estás modificando JARVIS-OS, el sistema operativo de Roman, MIENTRAS él lo usa: el kernel en
Rust (kernel/: escritorio, drivers, red, gráficos...) y el cerebro en Python (src/jarvis/). Lo
pidió hablándole a JARVIS, así que tu avance se ve en la ventana Proyecto.

Cómo trabajar:
- Antes de leer código, ubicá lo que hay que tocar con el grafo del proyecto:
  `graphify query "<pregunta>"`, `graphify explain "<símbolo>"` o `graphify path "A" "B"`
  (graphify-out/graph.json). Después leé solo los archivos que te señala.
- Seguí el CLAUDE.md (abajo) y los ADR de docs/adr/. Cambios de arquitectura: explicalos antes.
- Verificá lo que cambiaste: `cargo test -p <crate> --manifest-path kernel/Cargo.toml`,
  `cargo clippy --manifest-path kernel/Cargo.toml --workspace --exclude jarvis-kernel
  --all-targets -- -D warnings`, `cargo fmt --manifest-path kernel/Cargo.toml --all`, y para
  Python `uv run pytest`, `uv run ruff check .` y `uv run mypy`. Un comando por vez, sin `&&`
  ni `;` (así Roman los aprueba rápido).
- Drivers nuevos: la lógica va en kernel/drivers (no_std, sin unsafe, con tests en el host) y
  los registros en kernel/kernel/src, como los que ya están (ver ADR 0011).
- No hagas commit ni push salvo que Roman lo pida. No toques target/: la imagen la está usando
  QEMU. Los cambios se aplican cuando JARVIS llama a aplicar_cambios_sistema (recompila y
  reinicia JARVIS-OS); no lo hagas vos.
- Al terminar, resumí en pocas frases qué cambiaste y cómo lo probaste.
"""


class OsProject(ClaudeProject):
    """El agente de código sobre el propio repositorio de JARVIS-OS (`modificar_sistema`).
    Como un proyecto, pero con instrucciones del sistema operativo y los comandos que solo
    leen o verifican con menos fricción (ver `os_command_level`)."""

    def __init__(self, repo: Path, request: str, config: Config) -> None:
        super().__init__(repo, request, keep_going=False, config=config)
        self.name = "JARVIS-OS"

    def _system_prompt(self) -> Any:
        prompt = super()._system_prompt()
        prompt["append"] = OS_PROMPT + "\n" + prompt["append"]
        return prompt

    def decide(self, name: str, args: dict[str, Any]) -> int | None:
        level = decide(self.path, name, args)
        if level == 3 and name == "Bash":
            return os_command_level(str(args.get("command", "")))
        return level
