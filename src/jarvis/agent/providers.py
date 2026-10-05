"""Los otros agentes de IA que se vinculan a JARVIS: Gemini, ChatGPT y DeepSeek.

Cada uno se vincula de dos maneras (Configuración → Asistente de JARVIS-OS):

- **Con Google**: se lanza en el anfitrión el programa oficial del proveedor, que abre el
  navegador para entrar (Gemini CLI con la cuenta de Google; Codex CLI con la de ChatGPT, que
  ofrece "Continuar con Google"). Las credenciales las guarda ese programa, nunca pasan por
  JARVIS. DeepSeek no tiene un programa así: se abre su página de claves (se entra con Google) y
  la clave se pega en el formulario.
- **Con el formulario**: una clave de API. Se valida contra el proveedor y se guarda en el
  anfitrión (`agentes.json` en la carpeta de configuración, solo legible por el usuario); al
  kernel solo le llegan los últimos 4 caracteres.

Con clave, el agente habla por la API y puede usar las tools de JARVIS (function calling), con
los mismos permisos que Claude. Con Google, habla por el programa del proveedor y solo contesta
texto.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import shutil
import tempfile
import urllib.error
import urllib.request
import webbrowser
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol

from platformdirs import user_config_path

from jarvis.protocol import Message

log = logging.getLogger("jarvis.agentes")

#: Lo que se espera a que Roman termine de entrar en el navegador.
LOGIN_TIMEOUT = 300.0
#: Lo que puede tardar una respuesta (con tools, cada vuelta).
ASK_TIMEOUT = 90.0
#: Vueltas de tools como máximo por pedido (para que un modelo no quede en un bucle).
MAX_ROUNDS = 8


class AgentError(RuntimeError):
    """El agente no pudo vincularse o responder (sin clave, clave inválida, sin red...)."""


@dataclass(frozen=True)
class Provider:
    id: str
    nombre: str
    #: "openai" (ChatGPT y DeepSeek hablan el mismo formato) o "gemini".
    api: str
    base_url: str
    modelo: str
    #: Dónde se crea una clave de API (se entra con Google).
    claves_url: str
    #: El programa del anfitrión para entrar con Google ("" = no hay: se abre `claves_url`).
    cli: str
    #: Cómo se instala ese programa (para decírselo a Roman si falta).
    instalar: str = ""


PROVIDERS: dict[str, Provider] = {
    "gemini": Provider(
        "gemini",
        "Gemini",
        "gemini",
        "https://generativelanguage.googleapis.com/v1beta",
        "gemini-2.5-flash",
        "https://aistudio.google.com/apikey",
        "gemini",
        "npm install -g @google/gemini-cli",
    ),
    "chatgpt": Provider(
        "chatgpt",
        "ChatGPT",
        "openai",
        "https://api.openai.com/v1",
        "gpt-5-mini",
        "https://platform.openai.com/api-keys",
        "codex",
        "npm install -g @openai/codex",
    ),
    "deepseek": Provider(
        "deepseek",
        "DeepSeek",
        "openai",
        "https://api.deepseek.com",
        "deepseek-chat",
        "https://platform.deepseek.com/api_keys",
        "",
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
    #: "clave" (API) o "google" (el programa del proveedor).
    metodo: str
    clave: str = ""
    #: Con quién se entró (con Google), si se sabe.
    email: str = ""

    def detail(self) -> str:
        """Lo que ve el kernel: nunca la clave entera."""
        if self.metodo == "clave":
            return f"clave ...{self.clave[-4:]}" if len(self.clave) >= 8 else "clave"
        return self.email or "Google"


class Vault:
    """Los vínculos, en `agentes.json` (carpeta de configuración del usuario, modo 600)."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or user_config_path("jarvis") / "agentes.json"

    def load(self) -> dict[str, Link]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}
        if not isinstance(data, dict):
            return {}
        links = {}
        for agent, v in data.items():
            if (
                agent in PROVIDERS
                and isinstance(v, dict)
                and v.get("metodo") in ("clave", "google")
            ):
                links[agent] = Link(
                    str(v["metodo"]), str(v.get("clave", "")), str(v.get("email", ""))
                )
        return links

    def _save(self, links: dict[str, Link]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        data = {
            a: {"metodo": lk.metodo, "clave": lk.clave, "email": lk.email}
            for a, lk in links.items()
        }
        tmp = self.path.with_suffix(".tmp")
        # Solo el usuario lo puede leer (en Windows, la carpeta del perfil ya es privada).
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
        os.replace(tmp, self.path)

    def set(self, agent: str, link: Link) -> None:
        links = self.load()
        links[agent] = link
        self._save(links)

    def remove(self, agent: str) -> None:
        links = self.load()
        if links.pop(agent, None) is not None:
            self._save(links)


# --- HTTP y programas del anfitrión (inyectables para los tests) --------------------------


class Http(Protocol):
    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        """(código HTTP, el JSON de la respuesta o el texto si no es JSON)."""
        ...


class UrllibHttp:
    """HTTP con la biblioteca estándar, en un hilo (las APIs son todas HTTPS fijas)."""

    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        return await asyncio.to_thread(self._request, method, url, headers, body)

    @staticmethod
    def _request(method: str, url: str, headers: dict[str, str], body: Any) -> tuple[int, Any]:
        if not url.startswith("https://"):
            raise AgentError("solo HTTPS")
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(  # noqa: S310 (siempre https://, ver arriba)
            url, data=data, method=method, headers={"Content-Type": "application/json", **headers}
        )
        try:
            with urllib.request.urlopen(req, timeout=ASK_TIMEOUT) as r:  # noqa: S310
                status, raw = r.status, r.read()
        except urllib.error.HTTPError as e:
            status, raw = e.code, e.read()
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise AgentError(f"sin conexión con el proveedor ({e})") from e
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


# --- Las tools en el formato de cada proveedor ---------------------------------------------

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


def gemini_tools(specs: list[ToolSpec]) -> list[dict[str, Any]]:
    decls = []
    for n, d, p in specs:
        decl: dict[str, Any] = {"name": n, "description": d}
        if p:
            decl["parameters"] = _schema(p)
        decls.append(decl)
    return [{"functionDeclarations": decls}]


# --- Los agentes -----------------------------------------------------------------------------


@dataclass
class AgentHub:
    """Los agentes vinculados: vincular, desvincular, su estado y preguntarles."""

    vault: Vault = field(default_factory=Vault)
    http: Http = field(default_factory=UrllibHttp)
    cli: CliRunner = run_cli
    #: Modelos elegidos en `config.toml` (`[agentes]`), por agente.
    modelos: dict[str, str] = field(default_factory=dict)
    #: El estado del último intento de vincular, por agente (lo ve Roman).
    estados: dict[str, str] = field(default_factory=dict)
    which: Callable[[str], str | None] = shutil.which
    open_url: Callable[[str], Any] = webbrowser.open
    #: Claude como consultado (sin tools): (sistema, pedido) → respuesta. None = no se consulta.
    claude: Callable[[str, str], Awaitable[str]] | None = None

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
            "metodo": link.metodo if link else "",
            "detalle": link.detail() if link else "",
            "estado": self.estados.get(agent, ""),
        }

    def messages(self) -> list[Message]:
        return [self.message(a) for a in PROVIDERS]

    def _model(self, p: Provider) -> str:
        return self.modelos.get(p.id) or p.modelo

    # Vincular ------------------------------------------------------------------------------

    async def link_key(self, agent: str, key: str) -> None:
        """El formulario: valida la clave contra el proveedor y la guarda."""
        p = provider(agent)
        key = key.strip()
        if len(key) < 8 or any(c.isspace() for c in key):
            raise AgentError("Esa no parece una clave de API.")
        if p.api == "gemini":
            status, data = await self.http.request(
                "GET", f"{p.base_url}/models?pageSize=1", {"x-goog-api-key": key}
            )
        else:
            status, data = await self.http.request(
                "GET", f"{p.base_url}/models", {"Authorization": f"Bearer {key}"}
            )
        if status in (400, 401, 403):
            raise AgentError(f"{p.nombre} rechazó la clave: {_error_text(data)}")
        if status >= 300:
            raise AgentError(f"{p.nombre} contestó {status}: {_error_text(data)}")
        self.vault.set(agent, Link("clave", key))
        self.estados[agent] = "Listo: clave guardada en el anfitrión."

    async def link_google(self, agent: str) -> None:
        """Abre el navegador del anfitrión para entrar con Google (o con la cuenta del
        proveedor, que ofrece "Continuar con Google")."""
        p = provider(agent)
        cli = self.which(p.cli) if p.cli else None
        if cli is None:
            self.open_url(p.claves_url)
            falta = f"No encontré {p.cli} ({p.instalar}). " if p.cli else ""
            self.estados[agent] = (
                f"{falta}Te abrí la página de claves de {p.nombre} en el navegador de la PC: "
                "entrá con Google, creá una clave y pegala en «Clave de API»."
            )
            return
        try:
            if p.id == "gemini":
                # Sin credenciales guardadas, Gemini CLI abre el navegador para entrar con Google.
                code, out = await self.cli(
                    [cli, "--output-format", "json"], "Respondé solo: ok", GEMINI_ENV, LOGIN_TIMEOUT
                )
                email = gemini_email()
                if code != 0 or email is None:
                    raise AgentError(_last_line(out) or "Gemini CLI no terminó de entrar.")
                self.vault.set(agent, Link("google", email=email))
            else:
                code, out = await self.cli([cli, "login"], "", {}, LOGIN_TIMEOUT)
                if code != 0:
                    raise AgentError(_last_line(out) or "Codex no terminó de entrar.")
                code, out = await self.cli([cli, "login", "status"], "", {}, 30.0)
                if code != 0:
                    raise AgentError(_last_line(out) or "Codex dice que no hay sesión.")
                self.vault.set(agent, Link("google", email=_last_line(out)[:60]))
        except TimeoutError as e:
            raise AgentError("Pasaron 5 minutos sin terminar de entrar.") from e
        self.estados[agent] = "Listo: entraste."

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
        """La respuesta del agente. Con `tools` (y vinculado con clave), puede usarlas."""
        if agent == "claude":
            if self.claude is None:
                raise AgentError("Claude no se puede consultar desde acá.")
            return await self.claude(system, prompt)
        p = provider(agent)
        link = self.vault.load().get(agent)
        if link is None:
            raise AgentError(f"{p.nombre} no está vinculado.")
        if link.metodo == "google":
            return await self._ask_cli(p, f"{system}\n\n---\n\n{prompt}")
        use_tools = tools if run_tool is not None else None
        if p.api == "gemini":
            return await self._ask_gemini(p, link.clave, system, prompt, use_tools, run_tool)
        return await self._ask_openai(p, link.clave, system, prompt, use_tools, run_tool)

    async def _ask_cli(self, p: Provider, text: str) -> str:
        cli = self.which(p.cli) if p.cli else None
        if cli is None:
            raise AgentError(f"No encontré {p.cli} en la PC ({p.instalar}).")
        try:
            if p.id == "gemini":
                code, out = await self.cli(
                    [cli, "--output-format", "json"], text, GEMINI_ENV, ASK_TIMEOUT
                )
                answer = _gemini_cli_answer(out)
            else:
                fd, name = tempfile.mkstemp(prefix="jarvis-codex-", suffix=".txt")
                os.close(fd)
                try:
                    args = [cli, "exec", "--skip-git-repo-check", "--sandbox", "read-only"]
                    args += ["--output-last-message", name, "-"]
                    code, out = await self.cli(args, text, {}, ASK_TIMEOUT)
                    answer = (await asyncio.to_thread(Path(name).read_text, "utf-8")).strip()
                finally:
                    with contextlib.suppress(OSError):
                        os.unlink(name)
        except TimeoutError as e:
            raise AgentError(f"{p.nombre} no contestó a tiempo.") from e
        if code != 0 or not answer:
            raise AgentError(f"{p.nombre}: {_last_line(out) or 'no contestó'}")
        return answer

    async def _ask_openai(
        self,
        p: Provider,
        key: str,
        system: str,
        prompt: str,
        tools: list[ToolSpec] | None,
        run_tool: ToolRunner | None,
    ) -> str:
        messages: list[dict[str, Any]] = [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
        ]
        for _ in range(MAX_ROUNDS):
            body: dict[str, Any] = {"model": self._model(p), "messages": messages}
            if tools:
                body["tools"] = openai_tools(tools)
            status, data = await self.http.request(
                "POST", f"{p.base_url}/chat/completions", {"Authorization": f"Bearer {key}"}, body
            )
            if status >= 300 or not isinstance(data, dict):
                raise AgentError(f"{p.nombre} contestó {status}: {_error_text(data)}")
            try:
                msg = data["choices"][0]["message"]
            except (KeyError, IndexError, TypeError) as e:
                raise AgentError(f"{p.nombre} contestó algo que no entiendo.") from e
            calls = msg.get("tool_calls") or []
            if not calls or run_tool is None:
                return str(msg.get("content") or "").strip()
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

    async def _ask_gemini(
        self,
        p: Provider,
        key: str,
        system: str,
        prompt: str,
        tools: list[ToolSpec] | None,
        run_tool: ToolRunner | None,
    ) -> str:
        contents: list[dict[str, Any]] = [{"role": "user", "parts": [{"text": prompt}]}]
        url = f"{p.base_url}/models/{self._model(p)}:generateContent"
        for _ in range(MAX_ROUNDS):
            body: dict[str, Any] = {
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": contents,
            }
            if tools:
                body["tools"] = gemini_tools(tools)
            status, data = await self.http.request("POST", url, {"x-goog-api-key": key}, body)
            if status >= 300 or not isinstance(data, dict):
                raise AgentError(f"{p.nombre} contestó {status}: {_error_text(data)}")
            try:
                content = data["candidates"][0]["content"]
                parts = content.get("parts", [])
            except (KeyError, IndexError, TypeError, AttributeError) as e:
                raise AgentError(f"{p.nombre} no devolvió una respuesta.") from e
            calls = [pt["functionCall"] for pt in parts if "functionCall" in pt]
            if not calls or run_tool is None:
                return "".join(
                    str(pt.get("text", "")) for pt in parts if not pt.get("thought")
                ).strip()
            # La respuesta del modelo va entera (lleva las firmas de su razonamiento).
            contents.append(content)
            results = []
            for call in calls:
                args = call.get("args") or {}
                ok, result = await run_tool(
                    str(call.get("name", "")), args if isinstance(args, dict) else {}
                )
                results.append(
                    {
                        "functionResponse": {
                            "name": call.get("name", ""),
                            "response": {"ok": ok, "resultado": result},
                        }
                    }
                )
            contents.append({"role": "user", "parts": results})
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


def _last_line(out: str) -> str:
    lines = [ln.strip() for ln in out.strip().splitlines() if ln.strip()]
    return lines[-1][:200] if lines else ""
