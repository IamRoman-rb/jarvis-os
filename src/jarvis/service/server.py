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
from collections.abc import Awaitable, Callable, Coroutine
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jarvis import account
from jarvis.agent.brain import Brain, BrainError
from jarvis.agent.council import LEADS, Mode
from jarvis.agent.providers import PROVIDERS, AgentError, AgentHub
from jarvis.projects import ProjectError, ProjectRunner, list_projects, resolve_project
from jarvis.protocol import MAX_LINE, Message, ProtocolError, decode, encode
from jarvis.service.voicehub import VoiceHub
from jarvis.update import UpdateError, Updater

log = logging.getLogger("jarvis.serve")

HELLO_TIMEOUT = 10.0
#: Lo que puede tardar el kernel en hacer una acción, y Roman en decidir una confirmación.
CALL_TIMEOUT = 30.0
CONFIRM_TIMEOUT = 120.0


#: Tools que se atienden acá, en el anfitrión (no en el kernel).
HOST_TOOLS = {
    "listar_proyectos",
    "abrir_proyecto",
    "consultar_agente",
    "modificar_sistema",
    "aplicar_cambios_sistema",
}

MakeProject = Callable[[Path, str, bool], ProjectRunner]
#: El clima de hoy, como una frase (para el saludo de la mañana).
WeatherCall = Callable[[], Awaitable[str]]

GREETINGS = {"manana": "Buenos días", "tarde": "Buenas tardes", "noche": "Buenas noches"}


def part_of_day(hour: int) -> str:
    """Mañana de 5 a 11, tarde de 12 a 19 y noche el resto."""
    if 5 <= hour < 12:
        return "manana"
    return "tarde" if 12 <= hour < 20 else "noche"


AccountCall = Callable[[], Awaitable[account.Account]]


@dataclass
class Host:
    """Lo del anfitrión que usa una sesión: la carpeta de proyectos y cómo abrir uno."""

    projects: Path
    make_project: MakeProject | None = None
    #: La voz del anfitrión (None = sin micrófono ni parlantes).
    voice: VoiceHub | None = None
    #: La cuenta de Claude (Configuración → Asistente). None = no se pregunta (tests).
    account_status: AccountCall | None = None
    account_login: AccountCall | None = None
    #: Gemini, ChatGPT y DeepSeek (Configuración → Asistente). None = no se vinculan (tests).
    agents: AgentHub | None = None
    #: El clima para el saludo de la mañana. None = sin clima (tests, sin red).
    weather: WeatherCall | None = None
    #: Modificar JARVIS-OS desde adentro: el agente sobre su repo y cómo aplicar los cambios.
    make_os_project: Callable[[str], ProjectRunner] | None = None
    updater: Updater | None = None


class Session:
    """Una conexión del kernel. También es el `Kernel` de las tools: les ejecuta las acciones y
    les pide las confirmaciones."""

    def __init__(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter, host: Host | None = None
    ) -> None:
        self.reader = reader
        self.writer = writer
        self.host = host
        self.project: ProjectRunner | None = None
        self.project_task: asyncio.Task[None] | None = None
        self._background: set[asyncio.Task[None]] = set()
        self.brain: Brain | None = None
        self.current: asyncio.Task[None] | None = None
        self.current_id: int | None = None
        self._calls = itertools.count(1)
        self._pending: dict[int, asyncio.Future[Any]] = {}
        self._login: asyncio.Task[None] | None = None
        self._linking: dict[str, asyncio.Task[None]] = {}
        #: El agente principal y el consejo (los elige Roman en Configuración → Asistente).
        self.mode = Mode()

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
        """El kernel ejecuta la acción y contesta `resultado` (las de proyectos, acá)."""
        if tool in HOST_TOOLS:
            return await self._host_call(tool, args)
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

    async def _host_call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        if tool == "consultar_agente":
            return await self._consult(str(args.get("agente", "")), str(args.get("pregunta", "")))
        if tool == "modificar_sistema":
            return self._start_os_project(str(args.get("pedido", "")))
        if tool == "aplicar_cambios_sistema":
            return await self._apply_update()
        if self.host is None:
            return False, "Este JARVIS no tiene carpeta de proyectos."
        root = self.host.projects
        if tool == "listar_proyectos":
            names = list_projects(root)
            return True, "\n".join(names) if names else f"No hay proyectos en {root}."
        try:
            path = resolve_project(root, str(args.get("nombre", "")))
        except ProjectError as e:
            return False, str(e)
        if self.project_task is not None and not self.project_task.done():
            return False, f"Ya estoy trabajando en {self.project.name if self.project else '?'}."
        if self.host.make_project is None:
            return False, "No puedo abrir proyectos."
        runner = self.host.make_project(
            path, str(args.get("pedido", "")), args.get("seguir", True) is not False
        )
        self.project = runner
        self.project_task = asyncio.create_task(self._run_project(runner))
        return True, f"Abrí {path.name}: el avance se ve en la ventana Proyecto de JARVIS-OS."

    def _start_os_project(self, request: str) -> tuple[bool, str]:
        make = self.host.make_os_project if self.host else None
        if make is None:
            return False, "No encuentro el código de JARVIS-OS en esta PC."
        if not request.strip():
            return False, "Falta qué cambiar."
        if self.project_task is not None and not self.project_task.done():
            return False, f"Ya estoy trabajando en {self.project.name if self.project else '?'}."
        runner = make(request)
        self.project = runner
        self.project_task = asyncio.create_task(self._run_project(runner))
        return True, (
            "Empecé a modificar JARVIS-OS: el avance se ve en la ventana Proyecto y cada cambio "
            "lo aprueba Roman. Cuando termine, aplicar_cambios_sistema lo pone en marcha."
        )

    async def _apply_update(self) -> tuple[bool, str]:
        updater = self.host.updater if self.host else None
        if updater is None:
            return False, "No encuentro el código de JARVIS-OS en esta PC."
        if self.project_task is not None and not self.project_task.done():
            return False, "El agente todavía está trabajando: esperá a que termine."
        try:
            await updater.check()
            updater.request_restart()
        except UpdateError as e:
            return False, str(e)
        # El kernel se apaga en unos segundos (así llega esta respuesta) y xtask lo rearma.
        await self.send({"t": "reiniciar", "motivo": "actualizar"})
        return True, "Compila. JARVIS-OS se reinicia en unos segundos con la versión nueva."

    async def _consult(self, agent: str, question: str) -> tuple[bool, str]:
        from jarvis.agent.council import ADVISOR_PROMPT

        hub = self.host.agents if self.host else None
        agent = agent.strip().lower()
        if hub is None:
            return False, "Este JARVIS no tiene otros agentes."
        if agent != "claude" and agent not in hub.linked():
            linked = ", ".join(["claude", *hub.linked()])
            return False, f"«{agent}» no está vinculado. Se puede consultar a: {linked}."
        try:
            return True, await asyncio.wait_for(hub.ask(agent, ADVISOR_PROMPT, question), 120.0)
        except (AgentError, TimeoutError) as e:
            return False, str(e) or "no contestó a tiempo"

    def set_mode(self, msg: Message) -> None:
        lead = msg.get("principal")
        if isinstance(lead, str) and lead in LEADS:
            self.mode.principal = lead
        if isinstance(msg.get("consejo"), bool):
            self.mode.consejo = msg["consejo"]

    async def send_agents(self, only: str | None = None) -> None:
        """El estado de los agentes (uno `agente` por cada uno)."""
        hub = self.host.agents if self.host else None
        if hub is None:
            return
        for m in hub.messages():
            if only is None or m["id"] == only:
                await self.send(m)

    async def _link(self, agent: str, metodo: str, key: str) -> None:
        hub = self.host.agents if self.host else None
        if hub is None:
            return
        try:
            if metodo == "clave":
                hub.estados[agent] = "Validando la clave..."
                await self.send_agents(agent)
                await hub.link_key(agent, key)
            else:
                hub.estados[agent] = "Seguí en el navegador de la PC (entrá con Google)."
                await self.send_agents(agent)
                await hub.link_google(agent)
        except AgentError as e:
            hub.estados[agent] = str(e)
        except Exception as e:  # el kernel tiene que enterarse de cualquier falla
            log.exception("falló vincular %s", agent)
            hub.estados[agent] = f"falla: {e}"
        await self.send_agents(agent)

    async def _run_project(self, runner: ProjectRunner) -> None:
        async def emit(ev: str, text: str) -> None:
            await self.send({"t": "proyecto", "nombre": runner.name, "ev": ev, "texto": text})

        try:
            await runner.run(emit, self.confirm)
        except asyncio.CancelledError:
            await emit("fin", "Detenido.")
            raise
        except Exception as e:  # el kernel tiene que enterarse de cualquier falla
            log.exception("el agente del proyecto falló")
            await emit("error", str(e))

    def _spawn(self, coro: Coroutine[Any, Any, None]) -> asyncio.Task[None]:
        task = asyncio.create_task(coro)
        self._background.add(task)
        task.add_done_callback(self._background.discard)
        return task

    async def send_account(self, call: AccountCall | None, doing: str = "") -> None:
        """Pregunta la cuenta (o inicia sesión) y se la manda al kernel como `cuenta`."""
        if call is None:
            await self.send({"t": "cuenta", "estado": "Este cerebro no maneja cuentas."})
            return
        if doing:
            await self.send({"t": "cuenta", "estado": doing})
        try:
            acc = await call()
        except account.AccountError as e:
            await self.send({"t": "cuenta", "estado": str(e)})
            return
        except Exception as e:  # el kernel tiene que enterarse de cualquier falla
            log.exception("falló la cuenta")
            await self.send({"t": "cuenta", "estado": f"falla: {e}"})
            return
        estado = ("Listo: entraste." if doing else "") if acc.logged_in else ""
        await self.send(
            {
                "t": "cuenta",
                "sesion": acc.logged_in,
                "email": acc.email,
                "plan": acc.plan,
                "estado": estado,
            }
        )

    async def stop_project(self) -> None:
        if self.project_task is not None and not self.project_task.done():
            if self.project is not None:
                await self.project.stop()
            self.project_task.cancel()

    def _resolve(self, msg: Message, value: Any) -> None:
        fut = self._pending.get(msg.get("llamada", -1))
        if fut is not None and not fut.done():
            fut.set_result(value)

    async def send(self, msg: Message) -> None:
        self.writer.write(encode(msg))
        await self.writer.drain()

    async def answer(self, req_id: int, text: str, by_voice: bool = False) -> None:
        if self.brain is None:
            return
        full = ""
        try:
            async for delta in self.brain.reply(text):
                full += delta
                await self.send({"t": "texto", "id": req_id, "delta": delta})
            await self.send({"t": "fin", "id": req_id})
            # Con voz, JARVIS siempre contesta en voz alta (se lo hayan pedido hablando o
            # escribiendo).
            voice = self.host.voice if self.host else None
            if voice is not None:
                task = asyncio.create_task(voice.speak(full))
                self._background.add(task)
                task.add_done_callback(self._background.discard)
        except asyncio.CancelledError:
            await self.send({"t": "fin", "id": req_id})
            raise
        except BrainError as e:
            await self.send({"t": "error", "id": req_id, "msg": str(e)})
        except Exception as e:  # el kernel tiene que enterarse de cualquier falla
            log.exception("el cerebro falló")
            await self.send({"t": "error", "id": req_id, "msg": f"falla del cerebro: {e}"})

    async def greet(self, req_id: int, moment: str, name: str) -> None:
        """El saludo al arrancar JARVIS-OS: según la hora y, a la mañana, con el clima."""
        if moment not in GREETINGS:
            from datetime import datetime

            moment = part_of_day(datetime.now().hour)
        name = name.strip()[:24]
        text = GREETINGS[moment] + (f", {name}." if name else ".")
        updater = self.host.updater if self.host else None
        news = updater.take_result() if updater is not None else None
        if news:
            text += " " + news
        weather = self.host.weather if self.host else None
        if moment == "manana" and weather is not None:
            from jarvis.weather import WeatherError

            try:
                text += " " + await weather()
            except WeatherError as e:
                log.warning("clima: %s", e)
                text += " No pude ver el clima ahora."
            except Exception:  # el saludo no puede fallar por el clima
                log.exception("falló el clima")
                text += " No pude ver el clima ahora."
        await self.send({"t": "texto", "id": req_id, "delta": text})
        await self.send({"t": "fin", "id": req_id})
        voice = self.host.voice if self.host else None
        if voice is not None:
            self._spawn(voice.speak(text))

    def _hush(self) -> None:
        if self.host is not None and self.host.voice is not None:
            self.host.voice.hush()

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
            self._hush()
            self.current_id = req_id
            by_voice = msg.get("origen") == "voz"
            self.current = asyncio.create_task(self.answer(req_id, text, by_voice))
        elif t == "cancelar":
            self._hush()
            if msg.get("id") == self.current_id:
                await self.cancel()
        elif t == "resultado":
            self._resolve(msg, (msg.get("ok") is True, msg.get("datos", "")))
        elif t == "confirmacion":
            self._resolve(msg, msg.get("ok") is True)
        elif t == "proyecto_detener":
            await self.stop_project()
        elif t == "decir":
            # Una respuesta local de JARVIS-OS a una orden por voz: también en voz alta.
            text = msg.get("texto")
            voice = self.host.voice if self.host else None
            if isinstance(text, str) and voice is not None:
                task = asyncio.create_task(voice.speak(text))
                self._background.add(task)
                task.add_done_callback(self._background.discard)
        elif t == "cuenta":
            self._spawn(self.send_account(self.host.account_status if self.host else None))
        elif t == "iniciar_sesion":
            if self._login is not None and not self._login.done():
                await self.send({"t": "cuenta", "estado": "Ya hay un inicio de sesión en curso."})
                return
            self._login = self._spawn(
                self.send_account(
                    self.host.account_login if self.host else None,
                    'Seguí en el navegador de la PC: elegí "Continuar con Google".',
                )
            )
        elif t == "saludo":
            req_id = msg.get("id")
            if not isinstance(req_id, int):
                raise ProtocolError("saludo sin id")
            self.current_id = req_id
            self.current = asyncio.create_task(
                self.greet(req_id, str(msg.get("momento", "")), str(msg.get("nombre", "")))
            )
        elif t == "agentes":
            await self.send_agents()
        elif t == "agentes_modo":
            self.set_mode(msg)
        elif t == "vincular":
            agent, metodo = msg.get("agente"), msg.get("metodo")
            hub = self.host.agents if self.host else None
            if hub is None or agent not in PROVIDERS:
                raise ProtocolError("vincular: agente desconocido")
            running = self._linking.get(agent)
            if running is not None and not running.done():
                hub.estados[agent] = "Ya hay un inicio de sesión en curso."
                await self.send_agents(agent)
                return
            key = msg.get("clave")
            self._linking[agent] = self._spawn(
                self._link(agent, "clave" if metodo == "clave" else "google", str(key or ""))
            )
        elif t == "desvincular":
            agent = msg.get("agente")
            hub = self.host.agents if self.host else None
            if hub is not None and isinstance(agent, str):
                try:
                    hub.unlink(agent)
                except AgentError as e:
                    log.info("desvincular: %s", e)
                    return
                await self.send_agents(agent)
        elif t == "escuchar":
            if self.host is not None and self.host.voice is not None:
                self.host.voice.listen_now()
            else:
                await self.send({"t": "escuchando", "activo": False, "motivo": "sin voz"})
        else:
            log.info("mensaje desconocido: %s", t)


async def serve_connection(
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
    token: str,
    make_brain: Callable[[Session], Brain],
    host: Host | None = None,
) -> None:
    peer = writer.get_extra_info("peername")
    session = Session(reader, writer, host)
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
        session.set_mode(hello)
        if host is not None and host.voice is not None:
            host.voice.session = session
            parlantes = hello.get("parlantes")
            host.voice.kernel_audio = isinstance(parlantes, int) and parlantes > 0
        await session.send({"t": "listo", "voz": host is not None and host.voice is not None})
        if host is not None and host.account_status is not None:
            session._spawn(session.send_account(host.account_status))
        await session.send_agents()
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
        await session.stop_project()
        await brain.close()
        writer.close()
        log.info("kernel desconectado (%s)", peer)


async def start(
    port: int, token: str, make_brain: Callable[[Session], Brain], host: Host | None = None
) -> asyncio.Server:
    """Empieza a escuchar (puerto 0 = uno libre, para los tests)."""
    return await asyncio.start_server(
        lambda r, w: serve_connection(r, w, token, make_brain, host),
        host="127.0.0.1",
        port=port,
        limit=MAX_LINE + 1,
    )


async def serve(
    port: int, token: str, make_brain: Callable[[Session], Brain], host: Host | None = None
) -> None:
    server = await start(port, token, make_brain, host)
    log.info("cerebro escuchando en 127.0.0.1:%d", port)
    async with server:
        await server.serve_forever()
