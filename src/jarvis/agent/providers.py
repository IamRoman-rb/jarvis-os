"""Los otros agentes de IA que se vinculan a JARVIS: Gemini, ChatGPT y DeepSeek. Sin claves de
API: se entra con la cuenta (Google) o el modelo corre en la PC.

- **Gemini**: con la cuenta de Google, por Gemini CLI (el programa oficial de Google). Al
  vincularlo abre el navegador de la PC para entrar; las credenciales las guarda ese programa,
  nunca pasan por JARVIS.
- **ChatGPT**: con la cuenta de ChatGPT, por Codex CLI (el programa oficial de OpenAI). La
  página para entrar ofrece "Continuar con Google".
- **DeepSeek**: no tiene un programa para entrar con Google (su API solo funciona con claves).
  Corre **en la PC**, con Ollama: el modelo abierto de DeepSeek, sin cuenta ni clave.

Los tres son parte del cerebro: tienen las mismas tools de JARVIS que Claude, con los mismos
permisos. Gemini y ChatGPT las usan por MCP a través del puente del cerebro (`toolbridge`), con
las herramientas propias de esos programas apagadas (salvo leer la web: no tocan la PC);
DeepSeek, por la API local de Ollama.

Las claves de API de antes quedan en `agentes.json` pero ya no se usan.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import re
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
import webbrowser
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol

from platformdirs import user_config_path

from jarvis.agent import toolbridge
from jarvis.protocol import Message

log = logging.getLogger("jarvis.agentes")

#: Lo que se espera a que Roman termine de entrar en el navegador.
LOGIN_TIMEOUT = 300.0
#: Lo que puede tardar bajar el modelo local (unos GB).
PULL_TIMEOUT = 3600.0
#: Lo que puede tardar una respuesta (con tools, cada vuelta en la API; todo, en los programas).
ASK_TIMEOUT = 90.0
CLI_TIMEOUT = 240.0
#: Vueltas de tools como máximo por pedido (para que un modelo no quede en un bucle).
MAX_ROUNDS = 8
#: Ollama, en la PC.
OLLAMA = "http://127.0.0.1:11434"


class AgentError(RuntimeError):
    """El agente no pudo vincularse o responder (sin sesión, sin el programa, sin red...)."""


@dataclass(frozen=True)
class Provider:
    id: str
    nombre: str
    #: "google" (un programa del anfitrión con la cuenta) o "local" (Ollama en la PC).
    metodo: str
    #: El programa del anfitrión (gemini, codex, ollama).
    cli: str
    #: Cómo se instala ese programa (para decírselo a Roman si falta).
    instalar: str
    #: Dónde se baja, si falta (se abre en el navegador).
    pagina: str
    #: El modelo (solo el local; los otros usan el de su programa).
    modelo: str = ""


PROVIDERS: dict[str, Provider] = {
    "gemini": Provider(
        "gemini",
        "Gemini",
        "google",
        "gemini",
        "npm install -g @google/gemini-cli",
        "https://github.com/google-gemini/gemini-cli",
    ),
    "chatgpt": Provider(
        "chatgpt",
        "ChatGPT",
        "google",
        "codex",
        "npm install -g @openai/codex",
        "https://github.com/openai/codex",
    ),
    "deepseek": Provider(
        "deepseek",
        "DeepSeek",
        "local",
        "ollama",
        "https://ollama.com/download",
        "https://ollama.com/download",
        "deepseek-r1:8b",
    ),
}


def provider(agent: str) -> Provider:
    try:
        return PROVIDERS[agent]
    except KeyError:
        raise AgentError(f"No conozco al agente «{agent}».") from None


# --- Los vínculos guardados ----------------------------------------------------------------


@dataclass(frozen=True)
class Link:
    #: "google" o "local".
    metodo: str
    #: Con quién se entró (con Google) o el modelo (local).
    detalle: str = ""

    def detail(self) -> str:
        """Lo que ve el kernel."""
        if self.metodo == "local":
            return f"en esta PC ({self.detalle})" if self.detalle else "en esta PC"
        return self.detalle or "Google"


class Vault:
    """Los vínculos, en `agentes.json` (carpeta de configuración del usuario, modo 600). Las
    claves de API de antes se dejan como están, pero no se usan."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or user_config_path("jarvis") / "agentes.json"

    def _raw(self) -> dict[str, Any]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}
        return data if isinstance(data, dict) else {}

    def load(self) -> dict[str, Link]:
        links = {}
        for agent, v in self._raw().items():
            p = PROVIDERS.get(agent)
            if p is not None and isinstance(v, dict) and v.get("metodo") == p.metodo:
                # "email" era el nombre del campo antes.
                links[agent] = Link(p.metodo, str(v.get("detalle") or v.get("email") or ""))
        return links

    def _save(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        # Solo el usuario lo puede leer (en Windows, la carpeta del perfil ya es privada).
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
        os.replace(tmp, self.path)

    def set(self, agent: str, link: Link) -> None:
        data = self._raw()
        data[agent] = {"metodo": link.metodo, "detalle": link.detalle}
        self._save(data)

    def remove(self, agent: str) -> None:
        data = self._raw()
        if data.pop(agent, None) is not None:
            self._save(data)


# --- HTTP y programas del anfitrión (inyectables para los tests) --------------------------


class Http(Protocol):
    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        """(código HTTP, el JSON de la respuesta o el texto si no es JSON)."""
        ...


class UrllibHttp:
    """HTTP con la biblioteca estándar, en un hilo. Solo a Ollama, en la PC."""

    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        return await asyncio.to_thread(self._request, method, url, headers, body)

    @staticmethod
    def _request(method: str, url: str, headers: dict[str, str], body: Any) -> tuple[int, Any]:
        if not url.startswith(OLLAMA + "/"):
            raise AgentError("solo a Ollama, en esta PC")
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(  # noqa: S310 (siempre Ollama local, ver arriba)
            url, data=data, method=method, headers={"Content-Type": "application/json", **headers}
        )
        try:
            with urllib.request.urlopen(req, timeout=CLI_TIMEOUT) as r:  # noqa: S310
                status, raw = r.status, r.read()
        except urllib.error.HTTPError as e:
            status, raw = e.code, e.read()
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise AgentError(f"Ollama no está andando en la PC ({e})") from e
        text = raw.decode("utf-8", "replace")
        try:
            return status, json.loads(text)
        except json.JSONDecodeError:
            return status, text


#: Corre un programa: (argumentos, lo que va por la entrada, variables extra, espera) →
#: (código de salida, salida).
CliRunner = Callable[[list[str], str, dict[str, str], float], Awaitable[tuple[int, str]]]


async def run_cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
    """El texto de Roman va por la entrada, nunca en la línea de comandos (en Windows estos
    programas son .cmd y cmd.exe interpretaría `&` o `|`)."""
    with tempfile.TemporaryDirectory(prefix="jarvis-") as cwd:
        proc = await asyncio.create_subprocess_exec(
            *args,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            cwd=cwd,
            env={**os.environ, **env},
        )
        try:
            out, _ = await asyncio.wait_for(proc.communicate(stdin.encode()), wait)
        except TimeoutError:
            proc.kill()
            await proc.wait()
            raise
        return proc.returncode or 0, out.decode("utf-8", "replace")


def _error_text(data: Any) -> str:
    if isinstance(data, dict):
        err = data.get("error")
        if isinstance(err, dict):
            return str(err.get("message") or err)[:200]
        if err:
            return str(err)[:200]
    return str(data)[:200]


# --- Las tools en el formato de cada uno ------------------------------------------------------

#: (nombre, descripción, parámetros), como `tools.system.SPECS`.
ToolSpec = tuple[str, str, dict[str, type]]
#: Ejecuta una tool (con permiso): (ok, datos).
ToolRunner = Callable[[str, dict[str, Any]], Awaitable[tuple[bool, str]]]


def _schema(params: dict[str, type]) -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            k: {"type": "boolean" if t is bool else "string"} for k, t in params.items()
        },
        "required": list(params),
    }


def openai_tools(specs: list[ToolSpec]) -> list[dict[str, Any]]:
    return [
        {"type": "function", "function": {"name": n, "description": d, "parameters": _schema(p)}}
        for n, d, p in specs
    ]


#: Las herramientas propias de Gemini CLI que se apagan: todo lo que toca la PC (la terminal y
#: los archivos) o no sirve acá. Quedan las de leer la web (como Claude).
GEMINI_EXCLUDED_TOOLS = [
    "run_shell_command",
    "write_file",
    "replace",
    "edit",
    "read_file",
    "read_many_files",
    "list_directory",
    "glob",
    "grep_search",
    "search_file_content",
    "save_memory",
    "write_todos",
    "codebase_investigator",
    "activate_skill",
    "get_internal_docs",
    "cli_help",
    "enter_plan_mode",
    "exit_plan_mode",
    "complete_task",
    "ask_user",
]


def gemini_extension() -> dict[str, Any]:
    """La extensión de Gemini CLI para JARVIS (`~/.gemini/extensions/jarvis`): el servidor MCP
    con las tools de JARVIS y las herramientas propias de Gemini apagadas. El puente le llega por
    la variable de entorno (Gemini la reemplaza al lanzar el servidor)."""
    return {
        "name": "jarvis",
        "version": "1.0.0",
        "mcpServers": {
            "jarvis": {
                "command": sys.executable,
                "args": ["-m", "jarvis.agent.mcp_proxy"],
                "env": {toolbridge.ENV: "${" + toolbridge.ENV + "}"},
                # Sin la confirmación de Gemini: los permisos los pide JARVIS (el Gate).
                "trust": True,
            }
        },
        "excludeTools": GEMINI_EXCLUDED_TOOLS,
    }


def gemini_home() -> Path:
    return Path.home() / ".gemini"


def codex_profile(with_tools: bool) -> str:
    """El perfil de Codex para JARVIS (`-p`): sin su terminal, sin pedir aprobación (no puede
    preguntarle a nadie) y, con tools, el servidor MCP de JARVIS. El puente le llega por la
    variable de entorno (`env_vars`): así nada raro pasa por la línea de comandos."""
    lines = ['approval_policy = "never"', "", "[features]", "shell_tool = false"]
    if with_tools:
        lines += [
            "",
            "[mcp_servers.jarvis]",
            f"command = {json.dumps(sys.executable)}",
            'args = ["-m", "jarvis.agent.mcp_proxy"]',
            f'env_vars = ["{toolbridge.ENV}"]',
        ]
    return "\n".join(lines) + "\n"


def codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")


# --- Los agentes -----------------------------------------------------------------------------


@dataclass
class AgentHub:
    """Los agentes vinculados: vincular, desvincular, su estado y preguntarles."""

    vault: Vault = field(default_factory=Vault)
    http: Http = field(default_factory=UrllibHttp)
    cli: CliRunner = run_cli
    #: Modelos elegidos en `config.toml` (`[agentes]`), por agente (solo el local).
    modelos: dict[str, str] = field(default_factory=dict)
    #: El estado del último intento de vincular, por agente (lo ve Roman).
    estados: dict[str, str] = field(default_factory=dict)
    which: Callable[[str], str | None] = shutil.which
    open_url: Callable[[str], Any] = webbrowser.open
    #: Claude como consultado (sin tools): (sistema, pedido) → respuesta. None = no se consulta.
    claude: Callable[[str, str], Awaitable[str]] | None = None
    #: Dónde van los perfiles de Codex y la extensión de Gemini (inyectables para los tests).
    codex_dir: Callable[[], Path] = codex_home
    gemini_dir: Callable[[], Path] = gemini_home

    def linked(self) -> list[str]:
        """Los vinculados, en el orden de `PROVIDERS`."""
        links = self.vault.load()
        return [a for a in PROVIDERS if a in links]

    def message(self, agent: str) -> Message:
        """El estado de un agente para el kernel (`agente{...}`)."""
        link = self.vault.load().get(agent)
        return {
            "t": "agente",
            "id": agent,
            "nombre": provider(agent).nombre,
            "vinculado": link is not None,
            "metodo": provider(agent).metodo,
            "detalle": link.detail() if link else "",
            "estado": self.estados.get(agent, ""),
        }

    def messages(self) -> list[Message]:
        return [self.message(a) for a in PROVIDERS]

    def _model(self, p: Provider) -> str:
        return self.modelos.get(p.id) or p.modelo

    def _program(self, p: Provider) -> str:
        cli = self.which(p.cli)
        if cli is None:
            self.open_url(p.pagina)
            raise AgentError(
                f"No encontré {p.cli} en la PC: instalalo ({p.instalar}) y volvé a vincular. "
                "Te abrí la página en el navegador."
            )
        return cli

    # Vincular ------------------------------------------------------------------------------

    async def link(self, agent: str) -> None:
        """Vincula el agente como corresponde (con Google o en la PC)."""
        p = provider(agent)
        if p.metodo == "local":
            await self._link_local(p)
        else:
            await self._link_google(p)

    async def _link_google(self, p: Provider) -> None:
        """Abre el navegador del anfitrión para entrar con la cuenta (Google)."""
        cli = self._program(p)
        try:
            if p.id == "gemini":
                # Sin credenciales guardadas, Gemini CLI abre el navegador para entrar con Google.
                code, out = await self._gemini(cli, "Respondé solo: ok", None, LOGIN_TIMEOUT)
                email = gemini_email()
                if code != 0 or email is None:
                    raise AgentError(_last_line(out) or "Gemini CLI no terminó de entrar.")
                self.vault.set(p.id, Link("google", email))
            else:
                code, out = await self.cli([cli, "login"], "", {}, LOGIN_TIMEOUT)
                if code != 0:
                    raise AgentError(_last_line(out) or "Codex no terminó de entrar.")
                code, out = await self.cli([cli, "login", "status"], "", {}, 30.0)
                if code != 0:
                    raise AgentError(_last_line(out) or "Codex dice que no hay sesión.")
                self.vault.set(p.id, Link("google", _last_line(out)[:60]))
        except TimeoutError as e:
            raise AgentError("Pasaron 5 minutos sin terminar de entrar.") from e
        self.estados[p.id] = "Listo: entraste y quedó conectado al cerebro."

    async def _link_local(self, p: Provider) -> None:
        """El modelo en la PC con Ollama: si falta, lo baja (una vez)."""
        cli = self._program(p)
        model = self._model(p)
        status, data = await self.http.request("GET", f"{OLLAMA}/api/tags", {})
        if status >= 300 or not isinstance(data, dict):
            raise AgentError(
                "Ollama está instalado pero no está andando: abrilo y volvé a vincular."
            )
        have = {m.get("name") for m in data.get("models", []) if isinstance(m, dict)}
        if model not in have:
            self.estados[p.id] = f"Bajando {model} a la PC (unos GB, puede tardar)..."
            try:
                code, out = await self.cli([cli, "pull", model], "", {}, PULL_TIMEOUT)
            except TimeoutError as e:
                raise AgentError(f"Bajar {model} tardó más de una hora.") from e
            if code != 0:
                raise AgentError(_last_line(out) or f"Ollama no pudo bajar {model}.")
        self.vault.set(p.id, Link("local", model))
        self.estados[p.id] = "Listo: corre en esta PC, sin cuenta ni clave."

    def unlink(self, agent: str) -> None:
        provider(agent)
        self.vault.remove(agent)
        self.estados[agent] = "Desvinculado."

    # Preguntar -----------------------------------------------------------------------------

    async def ask(
        self,
        agent: str,
        system: str,
        prompt: str,
        tools: list[ToolSpec] | None = None,
        run_tool: ToolRunner | None = None,
    ) -> str:
        """La respuesta del agente. Con `tools` y `run_tool`, puede usar las de JARVIS."""
        if agent == "claude":
            if self.claude is None:
                raise AgentError("Claude no se puede consultar desde acá.")
            return await self.claude(system, prompt)
        p = provider(agent)
        if self.vault.load().get(agent) is None:
            raise AgentError(f"{p.nombre} no está vinculado.")
        use_tools = tools if run_tool is not None else None
        if p.metodo == "local":
            return await self._ask_local(p, system, prompt, use_tools, run_tool)
        return await self._ask_cli(
            p, f"{system}\n\n---\n\n{prompt}", run_tool if use_tools else None
        )

    async def _gemini(
        self, cli: str, text: str, bridge: str | None, wait: float
    ) -> tuple[int, str]:
        """Gemini CLI con la cuenta de Google y la extensión de JARVIS (y ninguna otra)."""
        ext = self.gemini_dir() / "extensions" / "jarvis"
        ext.mkdir(parents=True, exist_ok=True)
        (ext / "gemini-extension.json").write_text(
            json.dumps(gemini_extension(), indent=2), encoding="utf-8"
        )
        env = {
            **GEMINI_ENV,
            # Corre en una carpeta temporal vacía: que confíe en ella (si no, apaga el MCP).
            "GEMINI_CLI_TRUST_WORKSPACE": "true",
            toolbridge.ENV: bridge or "",
        }
        # Sin tools (una opinión del consejo), ningún servidor MCP.
        servers = "jarvis" if bridge is not None else "ninguno"
        args = [cli, "--output-format", "json", "-e", "jarvis"]
        args += ["--allowed-mcp-server-names", servers]
        return await self.cli(args, text, env, wait)

    async def _codex(self, cli: str, text: str, bridge: str | None) -> tuple[int, str, str]:
        """Codex con la cuenta de ChatGPT y el perfil de JARVIS. (código, salida, respuesta)."""
        name = "jarvis" if bridge is not None else "jarvis-consulta"
        home = self.codex_dir()
        home.mkdir(parents=True, exist_ok=True)
        (home / f"{name}.config.toml").write_text(
            codex_profile(bridge is not None), encoding="utf-8"
        )
        fd, last = tempfile.mkstemp(prefix="jarvis-codex-", suffix=".txt")
        os.close(fd)
        try:
            args = [cli, "exec", "-p", name, "--skip-git-repo-check", "--sandbox", "read-only"]
            args += ["--output-last-message", last, "-"]
            env = {toolbridge.ENV: bridge} if bridge is not None else {}
            code, out = await self.cli(args, text, env, CLI_TIMEOUT)
            answer = (await asyncio.to_thread(Path(last).read_text, "utf-8")).strip()
        finally:
            with contextlib.suppress(OSError):
                os.unlink(last)
        return code, out, answer

    async def _ask_cli(self, p: Provider, text: str, run_tool: ToolRunner | None) -> str:
        cli = self.which(p.cli)
        if cli is None:
            raise AgentError(f"No encontré {p.cli} en la PC ({p.instalar}).")
        try:
            async with contextlib.AsyncExitStack() as stack:
                bridge = None
                if run_tool is not None:
                    bridge = (
                        await stack.enter_async_context(toolbridge.ToolBridge(run_tool))
                    ).address
                if p.id == "gemini":
                    code, out = await self._gemini(cli, text, bridge, CLI_TIMEOUT)
                    answer = _gemini_cli_answer(out)
                else:
                    code, out, answer = await self._codex(cli, text, bridge)
        except TimeoutError as e:
            raise AgentError(f"{p.nombre} no contestó a tiempo.") from e
        if code != 0 or not answer:
            raise AgentError(f"{p.nombre}: {_last_line(out) or 'no contestó'}")
        return answer

    async def _ask_local(
        self,
        p: Provider,
        system: str,
        prompt: str,
        tools: list[ToolSpec] | None,
        run_tool: ToolRunner | None,
    ) -> str:
        """Ollama en la PC, con su API compatible con la de OpenAI (y function calling)."""
        messages: list[dict[str, Any]] = [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
        ]
        url = f"{OLLAMA}/v1/chat/completions"
        for _ in range(MAX_ROUNDS):
            body: dict[str, Any] = {"model": self._model(p), "messages": messages}
            if tools:
                body["tools"] = openai_tools(tools)
            status, data = await self.http.request("POST", url, {}, body)
            if status == 400 and tools and "tools" in _error_text(data):
                # Este modelo no sabe usar tools: contesta solo texto.
                tools = None
                continue
            if status >= 300 or not isinstance(data, dict):
                raise AgentError(f"{p.nombre} contestó {status}: {_error_text(data)}")
            try:
                msg = data["choices"][0]["message"]
            except (KeyError, IndexError, TypeError) as e:
                raise AgentError(f"{p.nombre} contestó algo que no entiendo.") from e
            calls = msg.get("tool_calls") or []
            if not calls or run_tool is None:
                return _without_thinking(str(msg.get("content") or ""))
            messages.append(msg)
            for call in calls:
                fn = call.get("function", {})
                try:
                    args = json.loads(fn.get("arguments") or "{}")
                except json.JSONDecodeError:
                    args = {}
                ok, result = await run_tool(
                    str(fn.get("name", "")), args if isinstance(args, dict) else {}
                )
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": call.get("id", ""),
                        "content": result if ok else f"ERROR: {result}",
                    }
                )
        raise AgentError(f"{p.nombre} encadenó demasiadas herramientas.")


#: Gemini CLI con la cuenta de Google (no con una clave de API).
GEMINI_ENV = {"GOOGLE_GENAI_USE_GCA": "true"}


def gemini_email(home: Path | None = None) -> str | None:
    """Con quién entró Gemini CLI (`~/.gemini`); None si no hay credenciales."""
    base = (home or Path.home()) / ".gemini"
    if not (base / "oauth_creds.json").exists():
        return None
    try:
        data = json.loads((base / "google_accounts.json").read_text(encoding="utf-8"))
        return str(data.get("active") or "Google")
    except (OSError, json.JSONDecodeError, AttributeError):
        return "Google"


def _gemini_cli_answer(out: str) -> str:
    """`gemini --output-format json` imprime `{"response": ...}` (a veces con avisos antes)."""
    start = out.find("{")
    if start >= 0:
        with contextlib.suppress(json.JSONDecodeError):
            data = json.loads(out[start:])
            if isinstance(data, dict) and isinstance(data.get("response"), str):
                return str(data["response"]).strip()
    return out.strip()


def _without_thinking(text: str) -> str:
    """DeepSeek-R1 puede devolver su razonamiento entre `<think>`: no es la respuesta."""
    return re.sub(r"<think>.*?</think>", "", text, flags=re.S).strip()


def _last_line(out: str) -> str:
    lines = [ln.strip() for ln in out.strip().splitlines() if ln.strip()]
    return lines[-1][:200] if lines else ""
