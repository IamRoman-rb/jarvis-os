"""Gemini, ChatGPT y DeepSeek: vincularlos (Google o clave), preguntarles con tools y el
cerebro conjunto (principal, consejo y reemplazo si el principal falla). Sin red: el HTTP y los
programas del anfitrión son de mentira."""

import asyncio
import json
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest

from jarvis.agent.brain import BrainError
from jarvis.agent.council import CouncilBrain, Mode, with_opinions
from jarvis.agent.providers import AgentError, AgentHub, Link, Vault, gemini_email
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


def hub(tmp_path: Path, http: FakeHttp | None = None, **kw: Any) -> AgentHub:
    return AgentHub(vault=Vault(tmp_path / "agentes.json"), http=http or FakeHttp(), **kw)


def test_la_boveda_guarda_y_al_kernel_solo_llegan_4_caracteres(tmp_path: Path) -> None:
    v = Vault(tmp_path / "agentes.json")
    v.set("deepseek", Link("clave", "sk-1234567890abcd"))
    v.set("gemini", Link("google", email="roman@gmail.com"))
    assert v.load()["deepseek"].clave == "sk-1234567890abcd"
    h = hub(tmp_path)
    msg = h.message("deepseek")
    assert msg["vinculado"] is True and msg["detalle"] == "clave ...abcd"
    assert "1234567890" not in json.dumps(h.messages())
    assert h.linked() == ["gemini", "deepseek"]
    v.remove("gemini")
    assert h.linked() == ["deepseek"]


async def test_formulario_valida_la_clave_antes_de_guardarla(tmp_path: Path) -> None:
    http = FakeHttp((401, {"error": {"message": "Incorrect API key"}}), (200, {"data": []}))
    h = hub(tmp_path, http)
    with pytest.raises(AgentError, match="rechazó la clave"):
        await h.link_key("chatgpt", "sk-mala-123456")
    assert h.linked() == []
    await h.link_key("chatgpt", " sk-buena-123456 ")
    assert h.vault.load()["chatgpt"] == Link("clave", "sk-buena-123456")
    assert http.calls[1][1] == "https://api.openai.com/v1/models"
    assert http.calls[1][2] == {"Authorization": "Bearer sk-buena-123456"}
    with pytest.raises(AgentError, match="no parece"):
        await h.link_key("gemini", "corta")


async def test_google_sin_programa_abre_la_pagina_de_claves(tmp_path: Path) -> None:
    opened: list[str] = []
    h = hub(tmp_path, which=lambda _: None, open_url=opened.append)
    await h.link_google("deepseek")
    assert opened == ["https://platform.deepseek.com/api_keys"]
    assert "pegala" in h.estados["deepseek"]
    assert h.linked() == []


async def test_google_con_codex_entra_con_la_cuenta_de_chatgpt(tmp_path: Path) -> None:
    ran: list[list[str]] = []

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        ran.append(args[1:])
        return 0, "Logged in using ChatGPT\n"

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    await h.link_google("chatgpt")
    assert ran == [["login"], ["login", "status"]]
    assert h.vault.load()["chatgpt"] == Link("google", email="Logged in using ChatGPT")
    assert h.message("chatgpt")["estado"] == "Listo: entraste."


async def test_con_google_el_texto_va_por_la_entrada_y_no_por_la_linea_de_comandos(
    tmp_path: Path,
) -> None:
    got: dict[str, Any] = {}

    async def cli(args: list[str], stdin: str, env: dict[str, str], wait: float) -> tuple[int, str]:
        got.update(args=args, stdin=stdin, env=env)
        return 0, 'Aviso: algo\n{"response": "Hola desde Gemini"}'

    h = hub(tmp_path, which=lambda name: f"/bin/{name}", cli=cli)
    h.vault.set("gemini", Link("google", email="roman@gmail.com"))
    assert await h.ask("gemini", "Sistema", "hola & del C:\\ | x") == "Hola desde Gemini"
    assert "hola & del" in got["stdin"] and not any("hola" in a for a in got["args"])
    assert got["env"] == {"GOOGLE_GENAI_USE_GCA": "true"}


def test_gemini_email(tmp_path: Path) -> None:
    assert gemini_email(tmp_path) is None
    (tmp_path / ".gemini").mkdir()
    (tmp_path / ".gemini" / "oauth_creds.json").write_text("{}")
    (tmp_path / ".gemini" / "google_accounts.json").write_text('{"active": "roman@gmail.com"}')
    assert gemini_email(tmp_path) == "roman@gmail.com"


async def test_deepseek_usa_las_tools_de_jarvis(tmp_path: Path) -> None:
    http = FakeHttp(
        (
            200,
            {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                {
                                    "id": "c1",
                                    "type": "function",
                                    "function": {
                                        "name": "abrir_app",
                                        "arguments": '{"app": "monitor"}',
                                    },
                                }
                            ],
                        }
                    }
                ]
            },
        ),
        (
            200,
            {"choices": [{"message": {"role": "assistant", "content": "Listo, abrí el monitor."}}]},
        ),
    )
    h = hub(tmp_path, http)
    h.vault.set("deepseek", Link("clave", "sk-deepseek-1234"))
    ran: list[tuple[str, dict[str, Any]]] = []

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        ran.append((name, args))
        return True, "abierta"

    specs = [("abrir_app", "Abre una app", {"app": str})]
    answer = await h.ask("deepseek", "Sos JARVIS", "abrí el monitor", specs, run_tool)
    assert answer == "Listo, abrí el monitor."
    assert ran == [("abrir_app", {"app": "monitor"})]
    url, body = http.calls[0][1], http.calls[0][3]
    assert url == "https://api.deepseek.com/chat/completions"
    assert body["model"] == "deepseek-chat"
    assert body["tools"][0]["function"]["parameters"]["required"] == ["app"]
    assert http.calls[1][3]["messages"][-1] == {
        "role": "tool",
        "tool_call_id": "c1",
        "content": "abierta",
    }


async def test_gemini_usa_las_tools_de_jarvis(tmp_path: Path) -> None:
    call = {"role": "model", "parts": [{"functionCall": {"name": "estado_sistema", "args": {}}}]}
    http = FakeHttp(
        (200, {"candidates": [{"content": call}]}),
        (
            200,
            {
                "candidates": [
                    {
                        "content": {
                            "role": "model",
                            "parts": [
                                {"text": "pensando", "thought": True},
                                {"text": "Todo bien."},
                            ],
                        }
                    }
                ]
            },
        ),
    )
    h = hub(tmp_path, http, modelos={"gemini": "gemini-2.5-pro"})
    h.vault.set("gemini", Link("clave", "AIza-gemini-1234"))

    async def run_tool(name: str, args: dict[str, Any]) -> tuple[bool, str]:
        return True, "CPU 3%"

    specs = [("estado_sistema", "Estado", {})]
    assert await h.ask("gemini", "Sos JARVIS", "¿cómo andás?", specs, run_tool) == "Todo bien."
    assert http.calls[0][1].endswith("/models/gemini-2.5-pro:generateContent")
    assert http.calls[0][2] == {"x-goog-api-key": "AIza-gemini-1234"}
    assert "parameters" not in http.calls[0][3]["tools"][0]["functionDeclarations"][0]
    sent = http.calls[1][3]["contents"]
    assert sent[1] == call
    assert sent[2]["parts"][0]["functionResponse"]["response"] == {
        "ok": True,
        "resultado": "CPU 3%",
    }


# --- El cerebro conjunto -----------------------------------------------------------------


class FakeClaude:
    def __init__(self, fail: bool = False) -> None:
        self.fail = fail
        self.prompts: list[str] = []

    async def reply(self, text: str) -> AsyncIterator[str]:
        self.prompts.append(text)
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
        if agent != "claude":
            h.vault.set(agent, Link("clave", f"clave-{agent}-1234"))
    asked: list[tuple[str, str]] = []

    async def ask(agent: str, system: str, prompt: str, *rest: Any) -> str:
        asked.append((agent, prompt))
        if answers[agent] == "ERROR":
            raise AgentError("sin saldo")
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

    h = hub(tmp_path, FakeHttp((200, {"data": []})))
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
    assert sessions[0].mode == Mode("deepseek", True)
    # El formulario: la clave se valida y queda guardada; al kernel le llega enmascarada.
    writer.write(
        encode(
            {"t": "vincular", "agente": "deepseek", "metodo": "clave", "clave": "sk-ds-123456789"}
        )
    )
    assert (await read())["estado"] == "Validando la clave..."
    done = await read()
    assert done["vinculado"] is True and done["detalle"] == "clave ...6789"
    assert "sk-ds" not in json.dumps(done)
    writer.write(encode({"t": "agentes_modo", "principal": "gemini", "consejo": False}))
    writer.write(encode({"t": "desvincular", "agente": "deepseek"}))
    assert (await read())["vinculado"] is False
    assert sessions[0].mode == Mode("gemini", False)
    # consultar_agente: solo a los vinculados.
    ok, data = await sessions[0].call("consultar_agente", {"agente": "gemini", "pregunta": "?"})
    assert not ok and "no está vinculado" in data
    writer.close()
    server.close()
