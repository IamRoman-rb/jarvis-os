"""`jarvis serve`: el servicio al que se conecta el kernel (ADR 0008).

Escucha solo en 127.0.0.1 (QEMU lo ve como 10.0.2.2). La primera línea de cada conexión tiene que
ser `hola` con el token de la sesión (lo genera `cargo xtask run` y se lo pasa al kernel por
fw_cfg); si no coincide, se corta. Cada conexión tiene su propio cerebro, así la conversación
sigue mientras el kernel esté conectado.
"""

from __future__ import annotations

import asyncio
import hmac
import itertools
import logging
from collections.abc import Callable
from typing import Any

from jarvis.agent.brain import Brain, BrainError
from jarvis.protocol import MAX_LINE, Message, ProtocolError, decode, encode

log = logging.getLogger("jarvis.serve")

HELLO_TIMEOUT = 10.0
#: Lo que puede tardar el kernel en hacer una acción, y Roman en decidir una confirmación.
CALL_TIMEOUT = 30.0
CONFIRM_TIMEOUT = 120.0


class Session:
    """Una conexión del kernel. También es el `Kernel` de las tools: les ejecuta las acciones y
    les pide las confirmaciones."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.reader = reader
        self.writer = writer
        self.brain: Brain | None = None
        self.current: asyncio.Task[None] | None = None
        self.current_id: int | None = None
        self._calls = itertools.count(1)
        self._pending: dict[int, asyncio.Future[Any]] = {}

    async def _ask(self, msg: dict[str, Any], wait: float) -> Any:
        call = next(self._calls)
        fut: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[call] = fut
        try:
            await self.send({**msg, "llamada": call})
            return await asyncio.wait_for(fut, wait)
        finally:
            self._pending.pop(call, None)

    async def call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        """El kernel ejecuta la acción y contesta `resultado`."""
        try:
            ok, data = await self._ask({"t": "accion", "tool": tool, "args": args}, CALL_TIMEOUT)
        except TimeoutError:
            return False, "JARVIS-OS no contestó a tiempo."
        return bool(ok), str(data)

    async def confirm(self, tool: str, level: int, description: str) -> bool:
        """Roman decide en la pantalla de JARVIS (sin respuesta a tiempo = no)."""
        try:
            return bool(
                await self._ask(
                    {"t": "confirmar", "tool": tool, "nivel": level, "descripcion": description},
                    CONFIRM_TIMEOUT,
                )
            )
        except TimeoutError:
            return False

    def _resolve(self, msg: Message, value: Any) -> None:
        fut = self._pending.get(msg.get("llamada", -1))
        if fut is not None and not fut.done():
            fut.set_result(value)

    async def send(self, msg: Message) -> None:
        self.writer.write(encode(msg))
        await self.writer.drain()

    async def answer(self, req_id: int, text: str) -> None:
        if self.brain is None:
            return
        try:
            async for delta in self.brain.reply(text):
                await self.send({"t": "texto", "id": req_id, "delta": delta})
            await self.send({"t": "fin", "id": req_id})
        except asyncio.CancelledError:
            await self.send({"t": "fin", "id": req_id})
            raise
        except BrainError as e:
            await self.send({"t": "error", "id": req_id, "msg": str(e)})
        except Exception as e:  # el kernel tiene que enterarse de cualquier falla
            log.exception("el cerebro falló")
            await self.send({"t": "error", "id": req_id, "msg": f"falla del cerebro: {e}"})

    async def cancel(self) -> None:
        if self.current is not None and not self.current.done():
            if self.brain is not None:
                await self.brain.interrupt()
            self.current.cancel()

    async def handle(self, msg: Message) -> None:
        t = msg["t"]
        if t == "pedido":
            req_id, text = msg.get("id"), msg.get("texto")
            if not isinstance(req_id, int) or not isinstance(text, str) or not text.strip():
                raise ProtocolError("pedido sin id o sin texto")
            await self.cancel()
            self.current_id = req_id
            self.current = asyncio.create_task(self.answer(req_id, text))
        elif t == "cancelar":
            if msg.get("id") == self.current_id:
                await self.cancel()
        elif t == "resultado":
            self._resolve(msg, (msg.get("ok") is True, msg.get("datos", "")))
        elif t == "confirmacion":
            self._resolve(msg, msg.get("ok") is True)
        else:
            log.info("mensaje desconocido: %s", t)


async def serve_connection(
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
    token: str,
    make_brain: Callable[[Session], Brain],
) -> None:
    peer = writer.get_extra_info("peername")
    session = Session(reader, writer)
    brain = session.brain = make_brain(session)
    try:
        line = await asyncio.wait_for(reader.readline(), HELLO_TIMEOUT)
        hello = decode(line)
        given = hello.get("token")
        if hello["t"] != "hola" or not isinstance(given, str):
            raise ProtocolError("se esperaba hola")
        if not hmac.compare_digest(given.encode(), token.encode()):
            log.warning("token inválido desde %s", peer)
            return
        log.info("kernel conectado desde %s (%s)", peer, hello.get("equipo", "?"))
        await session.send({"t": "listo"})
        while True:
            line = await reader.readline()
            if not line:
                break
            if len(line) > MAX_LINE:
                raise ProtocolError("renglón demasiado largo")
            await session.handle(decode(line))
    except (ProtocolError, TimeoutError) as e:
        log.warning("conexión cortada (%s): %s", peer, e)
    except (ConnectionError, asyncio.IncompleteReadError):
        pass
    finally:
        await session.cancel()
        await brain.close()
        writer.close()
        log.info("kernel desconectado (%s)", peer)


async def start(port: int, token: str, make_brain: Callable[[Session], Brain]) -> asyncio.Server:
    """Empieza a escuchar (puerto 0 = uno libre, para los tests)."""
    return await asyncio.start_server(
        lambda r, w: serve_connection(r, w, token, make_brain),
        host="127.0.0.1",
        port=port,
        limit=MAX_LINE + 1,
    )


async def serve(port: int, token: str, make_brain: Callable[[Session], Brain]) -> None:
    server = await start(port, token, make_brain)
    log.info("cerebro escuchando en 127.0.0.1:%d", port)
    async with server:
        await server.serve_forever()
