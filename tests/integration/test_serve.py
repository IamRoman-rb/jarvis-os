"""`jarvis serve` con el cerebro simulado: token, respuestas de a pedazos y cancelar."""

import asyncio

from jarvis.agent.brain import ScriptedBrain
from jarvis.protocol import decode, encode
from jarvis.service.server import start

TOKEN = "t" * 32


async def connect(token: str = TOKEN) -> tuple[asyncio.StreamReader, asyncio.StreamWriter]:
    server = await start(0, TOKEN, lambda s: ScriptedBrain(s, delay=0.01))
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(encode({"t": "hola", "token": token, "equipo": "jarvis"}))
    await writer.drain()
    return reader, writer


async def read(reader: asyncio.StreamReader) -> dict[str, object]:
    return decode(await asyncio.wait_for(reader.readline(), 5))


async def test_responde_de_a_pedazos() -> None:
    reader, writer = await connect()
    assert (await read(reader))["t"] == "listo"
    writer.write(encode({"t": "pedido", "id": 1, "texto": "hola JARVIS", "origen": "consola"}))
    text = ""
    while True:
        msg = await read(reader)
        if msg["t"] == "fin":
            break
        assert msg["t"] == "texto" and msg["id"] == 1
        text += str(msg["delta"])
    assert text == "Hola, Roman. Sistema en línea y cerebro conectado."
    writer.close()


async def test_token_invalido_corta_la_conexion() -> None:
    reader, writer = await connect("otro" * 8)
    assert await asyncio.wait_for(reader.read(), 5) == b""
    writer.close()


async def test_cancelar_termina_la_respuesta() -> None:
    reader, writer = await connect()
    await read(reader)
    writer.write(encode({"t": "pedido", "id": 7, "texto": "una respuesta larga " * 20}))
    await read(reader)
    writer.write(encode({"t": "cancelar", "id": 7}))
    await writer.drain()
    seen = 0
    while (msg := await read(reader))["t"] != "fin":
        seen += 1
    assert msg["id"] == 7
    assert seen < 60, "cortó antes de terminar"
    writer.close()


async def test_un_renglon_invalido_corta_sin_romper_el_servicio() -> None:
    reader, writer = await connect()
    await read(reader)
    writer.write(b"esto no es json\n")
    await writer.drain()
    assert await asyncio.wait_for(reader.read(), 5) == b""


async def test_una_accion_la_ejecuta_el_kernel_y_la_confirmacion_la_decide_roman() -> None:
    reader, writer = await connect()
    await read(reader)
    writer.write(encode({"t": "pedido", "id": 2, "texto": "abrí el navegador y buscá rust"}))
    msg = await read(reader)
    assert msg["t"] == "accion" and msg["tool"] == "buscar_web"
    assert msg["args"] == {"consulta": "rust"}
    writer.write(encode({"t": "resultado", "llamada": msg["llamada"], "ok": True, "datos": "ok"}))
    text = ""
    while (m := await read(reader))["t"] != "fin":
        text += str(m["delta"])
    assert text.startswith("Hecho (buscar_web)")

    writer.write(encode({"t": "pedido", "id": 3, "texto": "borrá /Documentos/tesis.txt"}))
    msg = await read(reader)
    assert msg["t"] == "confirmar" and msg["nivel"] == 3
    writer.write(encode({"t": "confirmacion", "llamada": msg["llamada"], "ok": False}))
    text = ""
    while (m := await read(reader))["t"] != "fin":
        assert m["t"] == "texto", "sin confirmación no hay acción"
        text += str(m["delta"])
    assert text.startswith("No lo hice")
    writer.close()


async def test_abrir_un_proyecto_muestra_el_avance_y_pide_confirmar(tmp_path) -> None:  # type: ignore[no-untyped-def]
    from jarvis.projects import ScriptedProject
    from jarvis.service.server import Host

    (tmp_path / "jarvis-os").mkdir()
    host = Host(projects=tmp_path, make_project=lambda p, r, k: ScriptedProject(p, r, k))
    server = await start(0, TOKEN, lambda s: ScriptedBrain(s, delay=0.0), host)
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(encode({"t": "hola", "token": TOKEN}))
    await read(reader)
    writer.write(encode({"t": "pedido", "id": 1, "texto": "abrí el proyecto jarvis y seguí"}))
    seen: list[str] = []
    while True:
        msg = await read(reader)
        if msg["t"] == "confirmar":
            assert msg["nivel"] == 2
            if "abrir_proyecto" in str(msg["tool"]):
                writer.write(encode({"t": "confirmacion", "llamada": msg["llamada"], "ok": True}))
            else:
                assert msg["descripcion"] == "[jarvis-os] Edit README.md"
                writer.write(encode({"t": "confirmacion", "llamada": msg["llamada"], "ok": False}))
        elif msg["t"] == "proyecto":
            seen.append(f"{msg['ev']}: {msg['texto']}")
            if msg["ev"] == "fin":
                break
    assert seen[0] == "inicio: jarvis-os: Seguí con lo que estábamos trabajando."
    assert "texto: No toqué nada." in seen
    writer.close()
