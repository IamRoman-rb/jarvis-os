"""`jarvis serve` con el cerebro simulado: token, respuestas de a pedazos y cancelar."""

import asyncio

from jarvis.agent.brain import ScriptedBrain
from jarvis.protocol import decode, encode
from jarvis.service.server import start

TOKEN = "t" * 32


async def connect(token: str = TOKEN) -> tuple[asyncio.StreamReader, asyncio.StreamWriter]:
    server = await start(0, TOKEN, lambda: ScriptedBrain(delay=0.01))
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
