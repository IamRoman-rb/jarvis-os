"""Gemini, ChatGPT y DeepSeek: vincularlos sin claves (Google o en la PC), preguntarles con las
tools de JARVIS y el cerebro conjunto (principal, consejo y reemplazo si el principal falla o se
queda sin uso). Sin red: el HTTP y los programas del anfitrión son de mentira."""

import asyncio
import json
import tomllib
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest

from jarvis.agent import toolbridge
from jarvis.agent.brain import BrainError, ClaudeBrain
from jarvis.agent.council import EXHAUSTED_FOR, CouncilBrain, Mode, with_opinions
from jarvis.agent.providers import (
    OLLAMA,
    AgentError,
    AgentHub,
    Link,
    Vault,
    gemini_email,
)
from jarvis.protocol import decode, encode
from jarvis.service.server import Host, start


class FakeHttp:
    """Contesta en orden lo que tiene en `answers` y anota cada pedido."""

    def __init__(self, *answers: tuple[int, Any]) -> None:
        self.answers = list(answers)
        self.calls: list[tuple[str, str, dict[str, str], Any]] = []

    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        self.calls.append((method, url, headers, body))
        return self.answers.pop(0)


def read_json(path: str) -> Any:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")


def hub(tmp_path: Path, http: FakeHttp | None = None, **kw: Any) -> AgentHub:
    kw.setdefault("codex_dir", lambda: tmp_path / "codex")
    kw.setdefault("gemini_dir", lambda: tmp_path / "gemini")
    return AgentHub(vault=Vault(tmp_path / "agentes.json"), http=http or FakeHttp(), **kw)


def test_sin_claves_las_viejas_quedan_pero_no_se_usan(tmp_path: Path) -> None:
    path = tmp_path / "agentes.json"
    # Como quedaba antes: DeepSeek con una clave de API.
    path.write_text(json.dumps({"deepseek": {"metodo": "clave", "clave": "sk-vieja-1234"}}))
    v = Vault(path)
    assert v.load() == {}, "una clave ya no vincula a nadie"
    v.set("gemini", Link("google", "roman@gmail.com"))
    v.set("deepseek", Link("local", "deepseek-r1:8b"))
    assert json.loads(path.read_text())["gemini"] == {
        "metodo": "google",
        "detalle": "roman@gmail.com",
    }
    h = hub(tmp_path)
    assert h.linked() == ["gemini", "deepseek"]
    assert h.message("deepseek")["detalle"] == "en esta PC (deepseek-r1:8b)"
    assert h.message("chatgpt")["metodo"] == "google"
    v.remove("deepseek")
    assert "sk-vieja" not in path.read_text(), "desvincular borra también lo de antes"
    # El formato de antes de Google ("email") se sigue leyendo.
    path.write_text(json.dumps({"chatgpt": {"metodo": "google", "email": "Logged in"}}))
    assert v.load()["chatgpt"] == Link("google", "Logged in")


async def test_sin_el_programa_abre_su_pagina(tmp_path: Path) -> None:
    opened: list[str] = []
    h = hub(tmp_path, which=lambda _: None, open_url=opened.append)
    with pytest.raises(AgentError, match="No encontré gemini"):
        await h.link("gemini")
    with pytest.raises(AgentError, match="No encontré ollama"):
        await h.link("deepseek")
    assert opened == ["https://github.com/google-gemini/gemini-cli", "https://ollama.com/download"]
    assert h.linked() == []


async def test_google_con_codex_entra_con_la_cuenta_de_chatgpt(tmp_path: Path) -> None:
    ran: list[list[str]] = []

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        ran.append(args[1:])
        return 0, "Logged in using ChatGPT\n"

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    await h.link("chatgpt")
    assert ran == [["login"], ["login", "status"]]
    assert h.vault.load()["chatgpt"] == Link("google", "Logged in using ChatGPT")
    assert "conectado al cerebro" in h.message("chatgpt")["estado"]


async def test_deepseek_corre_en_la_pc_con_ollama(tmp_path: Path) -> None:
    ran: list[list[str]] = []

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        ran.append(args)
        return 0, "success\n"

    # Ollama anda pero no tiene el modelo: lo baja (una sola vez).
    http = FakeHttp((200, {"models": [{"name": "llama3:8b"}]}))
    h = hub(tmp_path, http, which=lambda name: f"/bin/{name}", cli=cli)
    await h.link("deepseek")
    assert ran == [["/bin/ollama", "pull", "deepseek-r1:8b"]]
    assert http.calls[0][1] == f"{OLLAMA}/api/tags"
    assert h.vault.load()["deepseek"] == Link("local", "deepseek-r1:8b")
    assert "sin cuenta ni clave" in h.message("deepseek")["estado"]
    # Ollama instalado pero cerrado: se dice.
    http.answers.append((500, "no"))
    with pytest.raises(AgentError, match="no está andando"):
        await h.link("deepseek")


async def test_gemini_entra_con_google_y_usa_las_tools_por_el_puente(tmp_path: Path) -> None:
    got: dict[str, Any] = {}
    ext_path = tmp_path / "gemini" / "extensions" / "jarvis" / "gemini-extension.json"

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        got.update(args=args, stdin=stdin, env=env, ext=read_json(str(ext_path)))
        # Como Gemini CLI: lanza el servidor MCP de la extensión (con la variable del puente
        # reemplazada), que le pasa la tool al cerebro.
        server = got["ext"]["mcpServers"]["jarvis"]
        assert server["args"] == ["-m", "jarvis.agent.mcp_proxy"] and server["trust"] is True
        assert server["env"] == {toolbridge.ENV: "${" + toolbridge.ENV + "}"}
        got["tool"] = await toolbridge.call(env[toolbridge.ENV], "abrir_app", {"app": "monitor"})
        return 0, 'Aviso: algo\n{"response": "Listo, abrí el monitor."}'

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    h.vault.set("gemini", Link("google", "roman@gmail.com"))
    ran: list[tuple[str, dict[str, Any]]] = []

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        ran.append((name, args))
        return True, "abierta"

    specs: list[tuple[str, str, dict[str, type]]] = [("abrir_app", "Abre una app", {"app": str})]
    answer = await h.ask("gemini", "Sos JARVIS", "abrí el monitor & del C:\\ | x", specs, run_tool)
    assert answer == "Listo, abrí el monitor."
    assert ran == [("abrir_app", {"app": "monitor"})] and got["tool"] == (True, "abierta")
    # El texto va por la entrada, nunca en la línea de comandos.
    assert "del C:" in got["stdin"] and not any("monitor" in a for a in got["args"])
    assert got["env"]["GOOGLE_GENAI_USE_GCA"] == "true"
    assert got["env"]["GEMINI_CLI_TRUST_WORKSPACE"] == "true"
    # Solo la extensión y el servidor de JARVIS (no los que Roman tenga para Gemini).
    assert got["args"][-4:] == ["-e", "jarvis", "--allowed-mcp-server-names", "jarvis"]
    # De las herramientas de Gemini, solo leer la web: no toca la PC.
    excluded = got["ext"]["excludeTools"]
    assert "run_shell_command" in excluded and "write_file" in excluded
    assert "google_web_search" not in excluded and "web_fetch" not in excluded


async def test_gemini_sin_tools_no_tiene_servidor_mcp(tmp_path: Path) -> None:
    seen: list[list[str]] = []

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        seen.append(args)
        return 0, '{"response": "Mi opinión."}'

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    h.vault.set("gemini", Link("google"))
    assert await h.ask("gemini", "Opiná", "¿qué hago?") == "Mi opinión."
    assert seen[0][-2:] == ["--allowed-mcp-server-names", "ninguno"]


async def test_chatgpt_usa_las_tools_con_el_perfil_de_jarvis(tmp_path: Path) -> None:
    got: dict[str, Any] = {}

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        got.update(args=args, stdin=stdin)
        profile = tomllib.loads((tmp_path / "codex" / "jarvis.config.toml").read_text())
        got["profile"] = profile
        got["tool"] = await toolbridge.call(env[toolbridge.ENV], "estado_sistema", {})
        last = args[args.index("--output-last-message") + 1]
        write(last, "Todo bien, CPU al 3%.")
        return 0, ""

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    h.vault.set("chatgpt", Link("google", "Logged in"))

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        return True, "CPU 3%"

    specs = [("estado_sistema", "Estado", {})]
    assert await h.ask("chatgpt", "Sos JARVIS", "¿cómo andás?", specs, run_tool) == (
        "Todo bien, CPU al 3%."
    )
    assert got["tool"] == (True, "CPU 3%")
    assert got["args"][1:4] == ["exec", "-p", "jarvis"]
    p = got["profile"]
    # Sin su terminal y sin pedir aprobación (los permisos los pide JARVIS).
    assert p["features"]["shell_tool"] is False and p["approval_policy"] == "never"
    assert p["mcp_servers"]["jarvis"]["env_vars"] == [toolbridge.ENV]
    assert p["mcp_servers"]["jarvis"]["args"] == ["-m", "jarvis.agent.mcp_proxy"]


def test_gemini_email(tmp_path: Path) -> None:
    assert gemini_email(tmp_path) is None
    (tmp_path / ".gemini").mkdir()
    (tmp_path / ".gemini" / "oauth_creds.json").write_text("{}")
    (tmp_path / ".gemini" / "google_accounts.json").write_text('{"active": "roman@gmail.com"}')
    assert gemini_email(tmp_path) == "roman@gmail.com"


async def test_deepseek_local_usa_las_tools_de_jarvis(tmp_path: Path) -> None:
    call = {
        "id": "c1",
        "type": "function",
        "function": {"name": "abrir_app", "arguments": '{"app": "monitor"}'},
    }
    http = FakeHttp(
        (200, {"choices": [{"message": {"role": "assistant", "tool_calls": [call]}}]}),
        (
            200,
            {
                "choices": [
                    {"message": {"content": "<think>a ver...</think>Listo, abrí el monitor."}}
                ]
            },
        ),
    )
    h = hub(tmp_path, http)
    h.vault.set("deepseek", Link("local", "deepseek-r1:8b"))

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        return True, "abierta"

    specs = [("abrir_app", "Abre una app", {"app": str})]
    answer = await h.ask("deepseek", "Sos JARVIS", "abrí el monitor", specs, run_tool)
    assert answer == "Listo, abrí el monitor.", "sin el razonamiento"
    url, headers, body = http.calls[0][1], http.calls[0][2], http.calls[0][3]
    assert url == f"{OLLAMA}/v1/chat/completions" and headers == {}, "sin clave"
    assert body["model"] == "deepseek-r1:8b"
    assert body["tools"][0]["function"]["parameters"]["required"] == ["app"]
    assert http.calls[1][3]["messages"][-1] == {
        "role": "tool",
        "tool_call_id": "c1",
        "content": "abierta",
    }


async def test_un_modelo_local_sin_tools_contesta_igual(tmp_path: Path) -> None:
    http = FakeHttp(
        (400, {"error": {"message": "registry.ollama.ai/x does not support tools"}}),
        (200, {"choices": [{"message": {"content": "Solo texto."}}]}),
    )
    h = hub(tmp_path, http)
    h.vault.set("deepseek", Link("local", "x"))

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        return True, ""

    assert await h.ask("deepseek", "s", "p", [("a", "b", {})], run_tool) == "Solo texto."
    assert "tools" not in http.calls[1][3]


# --- Claude sin uso ------------------------------------------------------------------------


class _FakeSdkClient:
    def __init__(self, messages: list[Any]) -> None:
        self.messages = messages

    async def query(self, text: str) -> None:
        pass

    async def receive_response(self) -> AsyncIterator[Any]:
        for m in self.messages:
            yield m


async def test_claude_sin_uso_es_un_error_antes_de_mostrar_nada() -> None:
    from claude_agent_sdk import AssistantMessage, ResultMessage, TextBlock

    from jarvis.config import Config

    limit = AssistantMessage(
        content=[TextBlock(text="You've hit your limit · resets 3pm")],
        model="claude",
        error="rate_limit",
    )
    brain = ClaudeBrain(Config(), kernel=None)  # type: ignore[arg-type]
    brain._client = _FakeSdkClient([limit])
    shown: list[str] = []
    with pytest.raises(BrainError) as e:
        async for d in brain.reply("hola"):
            shown.append(d)
    assert e.value.agotado and shown == []
    assert "sin uso" in str(e.value)
    # Versiones que no marcan el error: se reconoce el texto del límite.
    old = AssistantMessage(
        content=[TextBlock(text="Claude AI usage limit reached|1700")], model="c"
    )
    brain._client = _FakeSdkClient([old])
    with pytest.raises(BrainError) as e:
        async for _ in brain.reply("hola"):
            pass
    assert e.value.agotado
    # Un 429 al final también.
    result = ResultMessage(
        subtype="success",
        duration_ms=1,
        duration_api_ms=1,
        is_error=True,
        num_turns=1,
        session_id="s",
        result="Too many requests",
        api_error_status=429,
    )
    brain._client = _FakeSdkClient([result])
    with pytest.raises(BrainError) as e:
        async for _ in brain.reply("hola"):
            pass
    assert e.value.agotado
    # Una respuesta normal pasa.
    ok = AssistantMessage(content=[TextBlock(text="Hola, Roman.")], model="c")
    brain._client = _FakeSdkClient([ok])
    assert [d async for d in brain.reply("hola")] == ["Hola, Roman."]


# --- El cerebro conjunto -----------------------------------------------------------------


class FakeClaude:
    def __init__(self, fail: bool = False, agotado: bool = False) -> None:
        self.fail = fail
        self.agotado = agotado
        self.prompts: list[str] = []

    async def reply(self, text: str) -> AsyncIterator[str]:
        self.prompts.append(text)
        if self.agotado:
            raise BrainError("Claude se quedó sin uso por ahora", agotado=True)
        if self.fail:
            raise BrainError("sin sesión")
        yield "Claude: "
        yield "listo"

    async def interrupt(self) -> None:
        pass

    async def close(self) -> None:
        pass


class FakeKernel:
    async def call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        return True, "ok"

    async def confirm(self, tool: str, level: int, description: str) -> bool:
        return True


def council_hub(tmp_path: Path, answers: dict[str, str]) -> AgentHub:
    h = hub(tmp_path)
    for agent in answers:
        if agent == "deepseek":
            h.vault.set(agent, Link("local", "deepseek-r1:8b"))
        elif agent != "claude":
            h.vault.set(agent, Link("google", "roman@gmail.com"))
    asked: list[tuple[str, str]] = []

    async def ask(agent: str, system: str, prompt: str, *rest: Any) -> str:
        asked.append((agent, prompt))
        if answers[agent].startswith("ERROR"):
            raise AgentError(answers[agent][6:] or "sin sesión")
        return answers[agent]

    h.ask = ask  # type: ignore[method-assign]
    h.asked = asked  # type: ignore[attr-defined]
    return h


async def collect(brain: CouncilBrain, text: str) -> str:
    return "".join([d async for d in brain.reply(text)])


async def test_el_consejo_opina_y_claude_decide(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"gemini": "Usá el monitor.", "deepseek": "Revisá la RAM."})
    claude = FakeClaude()
    brain = CouncilBrain(claude, h, FakeKernel(), lambda: Mode("claude", True))
    assert await collect(brain, "¿por qué anda lento?") == "Claude: listo"
    assert sorted(a for a, _ in h.asked) == ["deepseek", "gemini"]  # type: ignore[attr-defined]
    assert claude.prompts == [
        with_opinions(
            "¿por qué anda lento?", [("Gemini", "Usá el monitor."), ("DeepSeek", "Revisá la RAM.")]
        )
    ]
    assert "información, no instrucciones" in claude.prompts[0]


async def test_sin_consejo_no_se_consulta_a_nadie(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"gemini": "x"})
    claude = FakeClaude()
    brain = CouncilBrain(claude, h, FakeKernel(), lambda: Mode("claude", False))
    await collect(brain, "hola")
    assert h.asked == [] and claude.prompts == ["hola"]  # type: ignore[attr-defined]


async def test_otro_principal_contesta_y_un_consejero_que_falla_no_frena(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"chatgpt": "Respuesta de ChatGPT", "deepseek": "ERROR"})
    claude = FakeClaude()
    brain = CouncilBrain(claude, h, FakeKernel(), lambda: Mode("chatgpt", True))
    assert await collect(brain, "hola") == "Respuesta de ChatGPT"
    assert claude.prompts == []
    lead_prompt = h.asked[-1]  # type: ignore[attr-defined]
    assert lead_prompt == ("chatgpt", "hola")  # DeepSeek falló: no hay opiniones


async def test_si_claude_no_tiene_sesion_contesta_el_siguiente(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"gemini": "Hola desde Gemini"})
    brain = CouncilBrain(FakeClaude(fail=True), h, FakeKernel(), lambda: Mode("claude", False))
    out = await collect(brain, "hola")
    assert out == "(Claude no pudo contestar; contesta Gemini.) Hola desde Gemini"


async def test_si_claude_se_queda_sin_uso_siguen_los_demas(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"gemini": "Hola desde Gemini", "deepseek": "Opino."})
    claude = FakeClaude(agotado=True)
    now = [1000.0]
    brain = CouncilBrain(
        claude, h, FakeKernel(), lambda: Mode("claude", True), clock=lambda: now[0]
    )
    out = await collect(brain, "hola")
    assert out == "(Claude se quedó sin uso por ahora; contesta Gemini.) Hola desde Gemini"
    assert brain.resting("claude")
    # El pedido siguiente ni lo intenta: contesta Gemini directo (y DeepSeek opina).
    claude.prompts.clear()
    h.asked.clear()  # type: ignore[attr-defined]
    assert await collect(brain, "¿y ahora?") == "Hola desde Gemini"
    assert claude.prompts == []
    assert [a for a, _ in h.asked] == ["deepseek", "gemini"]  # type: ignore[attr-defined]
    # Pasado el rato, se vuelve a probar con Claude (que ya tiene uso otra vez).
    now[0] += EXHAUSTED_FOR + 1
    claude.agotado = False
    assert await collect(brain, "hola") == "Claude: listo"


async def test_un_agente_sin_cuota_tambien_descansa(tmp_path: Path) -> None:
    h = council_hub(
        tmp_path, {"gemini": "ERROR:Gemini: Quota exceeded (429)", "chatgpt": "Desde ChatGPT"}
    )
    brain = CouncilBrain(FakeClaude(fail=True), h, FakeKernel(), lambda: Mode("gemini", False))
    out = await collect(brain, "hola")
    assert out == "(Claude no pudo contestar; contesta ChatGPT.) Desde ChatGPT"
    assert brain.resting("gemini") and not brain.resting("claude")


async def test_si_nadie_puede_contestar_es_un_error(tmp_path: Path) -> None:
    h = council_hub(tmp_path, {"gemini": "ERROR"})
    brain = CouncilBrain(FakeClaude(fail=True), h, FakeKernel(), lambda: Mode("gemini", False))
    with pytest.raises(BrainError, match="Ningún agente"):
        await collect(brain, "hola")


# --- El protocolo con el kernel ----------------------------------------------------------

TOKEN = "t" * 32


async def test_vincular_desde_el_kernel_y_consultar_agente(tmp_path: Path) -> None:
    from jarvis.agent.brain import ScriptedBrain
    from jarvis.service.server import Session

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        return 0, ""

    h = hub(
        tmp_path,
        FakeHttp((200, {"models": [{"name": "deepseek-r1:8b"}]})),
        which=lambda name: f"/bin/{name}",
        cli=cli,
    )
    sessions: list[Session] = []

    def make_brain(s: Session) -> ScriptedBrain:
        sessions.append(s)
        return ScriptedBrain(s, delay=0.0)

    server = await start(0, TOKEN, make_brain, Host(projects=tmp_path, agents=h))
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)

    async def read() -> dict[str, Any]:
        return decode(await asyncio.wait_for(reader.readline(), 5))

    hello = {"t": "hola", "token": TOKEN, "principal": "deepseek", "consejo": True}
    writer.write(encode(hello))
    assert (await read())["t"] == "listo"
    # Al conectarse, el estado de los tres.
    first = [await read() for _ in range(3)]
    assert [m["id"] for m in first] == ["gemini", "chatgpt", "deepseek"]
    assert all(m["t"] == "agente" and m["vinculado"] is False for m in first)
    assert [m["metodo"] for m in first] == ["google", "google", "local"]
    assert sessions[0].mode == Mode("deepseek", True)
    # DeepSeek se vincula en la PC (Ollama ya tiene el modelo). Un kernel viejo que mande una
    # clave igual pasa por ahí: la clave no se usa.
    writer.write(encode({"t": "vincular", "agente": "deepseek", "metodo": "clave", "clave": "x"}))
    assert "Ollama" in (await read())["estado"]
    done = await read()
    assert done["vinculado"] is True and done["detalle"] == "en esta PC (deepseek-r1:8b)"
    writer.write(encode({"t": "agentes_modo", "principal": "gemini", "consejo": False}))
    writer.write(encode({"t": "desvincular", "agente": "deepseek"}))
    assert (await read())["vinculado"] is False
    assert sessions[0].mode == Mode("gemini", False)
    # consultar_agente: solo a los vinculados.
    ok, data = await sessions[0].call("consultar_agente", {"agente": "gemini", "pregunta": "?"})
    assert not ok and "no está vinculado" in data
    writer.close()
    server.close()
