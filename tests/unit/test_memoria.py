"""La memoria de JARVIS: conversaciones y recuerdos que sobreviven entre sesiones."""

import asyncio
from pathlib import Path
from typing import Any

from jarvis.agent.brain import ScriptedBrain
from jarvis.agent.prompts import build_prompt
from jarvis.memory import Memory
from jarvis.protocol import decode, encode
from jarvis.service.server import Host, Session, start


def test_recuerda_las_conversaciones_de_la_sesion_anterior(tmp_path: Path) -> None:
    m = Memory(tmp_path)
    assert m.context() == ""
    m.log("¿cuándo rindo Sistemas Operativos?", "El lunes 12 a las 9.")
    # En la misma sesión no hace falta (Claude ya tiene la conversación).
    assert m.context() == ""
    # Al día siguiente (otra sesión), sí.
    later = Memory(tmp_path)
    ctx = later.context()
    assert "Sistemas Operativos" in ctx and "El lunes 12 a las 9." in ctx
    assert "no instrucciones" in ctx
    assert ctx in build_prompt(False, memory=ctx)


def test_recordar_y_olvidar(tmp_path: Path) -> None:
    m = Memory(tmp_path)
    assert m.remember("Roman prefiere la voz grave") == "Anotado: Roman prefiere la voz grave"
    assert m.remember("roman prefiere la  voz grave") == "Ya lo tenía anotado."
    m.remember("El perro de Roman se llama Thor")
    assert "El perro de Roman se llama Thor" in Memory(tmp_path).context()
    assert m.forget("perro") == "Olvidé: El perro de Roman se llama Thor"
    assert m.forget("gato") == "No tenía nada anotado sobre «gato»."
    assert [f.texto for f in m.facts()] == ["Roman prefiere la voz grave"]
    m.log("hola", "hola Roman")
    assert m.forget("todo") == "Borré 1 recuerdos y todas las conversaciones."
    assert m.facts() == [] and m.exchanges() == []


def test_buscar_en_la_memoria(tmp_path: Path) -> None:
    m = Memory(tmp_path)
    m.log("ayudame con el driver de la placa de red", "Usá el RTL8168 de kernel/drivers.")
    m.log("¿qué hora es?", "Las 10.")
    m.remember("La placa de red de la notebook es una Realtek")
    found = m.search("¿te acordás de la placa de red?")
    assert found.splitlines()[0].endswith(
        "[recuerdo] La placa de red de la notebook es una Realtek"
    )
    assert "RTL8168" in found and "Las 10" not in found
    assert "No encontré" in m.search("astronomía")
    # Sin palabras útiles, pide algo concreto.
    assert "palabra clave" in m.search("¿qué?")


def test_no_guarda_respuestas_vacias_y_se_recorta(tmp_path: Path, monkeypatch: Any) -> None:
    import jarvis.memory as mem

    monkeypatch.setattr(mem, "MAX_EXCHANGES", 10)
    m = Memory(tmp_path)
    m.log("hola", "")
    assert m.exchanges() == []
    for i in range(30):
        m.log(f"pedido {i}", f"respuesta {i}")
    got = m.exchanges()
    assert len(got) <= 12 and got[-1].pedido == "pedido 29"


TOKEN = "t" * 32


async def test_cada_pedido_queda_en_la_memoria_y_las_tools(tmp_path: Path) -> None:
    sessions: list[Session] = []

    def make_brain(s: Session) -> ScriptedBrain:
        sessions.append(s)
        return ScriptedBrain(s, delay=0.0)

    host = Host(projects=tmp_path, memory=Memory(tmp_path / "memoria"))
    server = await start(0, TOKEN, make_brain, host)
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)

    async def read() -> dict[str, Any]:
        return decode(await asyncio.wait_for(reader.readline(), 5))

    writer.write(encode({"t": "hola", "token": TOKEN}))
    assert (await read())["t"] == "listo"
    # "recordá que..." usa la tool recordar (nivel 1: sin confirmar).
    writer.write(encode({"t": "pedido", "id": 1, "texto": "recordá que mi materia favorita es SO"}))
    text = ""
    while (msg := await read())["t"] != "fin":
        text += str(msg.get("delta", ""))
    assert "Anotado: mi materia favorita es SO" in text
    assert host.memory is not None
    assert [f.texto for f in host.memory.facts()] == ["mi materia favorita es SO"]
    assert host.memory.exchanges()[-1].pedido == "recordá que mi materia favorita es SO"
    ok, found = await sessions[0].call("buscar_memoria", {"consulta": "materia favorita"})
    assert ok and "mi materia favorita es SO" in found
    writer.close()
    server.close()
