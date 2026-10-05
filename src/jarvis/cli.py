"""Comando `jarvis`."""

import asyncio
import contextlib
import logging
import os
from pathlib import Path
from typing import TYPE_CHECKING

import typer

from jarvis import __version__

if TYPE_CHECKING:
    from jarvis.config import Config
    from jarvis.service.voicehub import VoiceHub

app = typer.Typer(help="JARVIS: asistente de JARVIS-OS.", no_args_is_help=True)


@app.command()
def version() -> None:
    """Muestra la versión instalada de JARVIS."""
    typer.echo(f"jarvis {__version__}")


@app.command()
def serve(
    puerto: int = typer.Option(0, help="Puerto en 127.0.0.1 (0 = el de config.toml, 8121)."),
    simulado: bool = typer.Option(False, help="Respuestas fijas, sin Claude (para los tests)."),
    proyectos: str = typer.Option("", help="Carpeta de proyectos (vacío = la de config.toml)."),
    sin_voz: bool = typer.Option(False, help="Sin micrófono ni parlantes."),
) -> None:
    """El cerebro para el kernel de JARVIS-OS (lo levanta `cargo xtask run`).

    El token de la sesión llega por la variable JARVIS_CEREBRO_TOKEN (no por la línea de
    comandos, que la ven los demás procesos).
    """
    from jarvis import update
    from jarvis.agent.brain import Brain, ClaudeBrain, ScriptedBrain
    from jarvis.agent.council import CouncilBrain, ask_claude
    from jarvis.agent.providers import AgentHub
    from jarvis.config import Config
    from jarvis.projects import ClaudeProject, OsProject, ProjectRunner, ScriptedProject
    from jarvis.service.server import Host, Session
    from jarvis.service.server import serve as run_server

    token = os.environ.get("JARVIS_CEREBRO_TOKEN", "")
    if len(token) < 16:
        typer.echo("falta JARVIS_CEREBRO_TOKEN (al menos 16 caracteres)", err=True)
        raise typer.Exit(2)
    config = Config.load()
    setup_logging(config)

    def make_brain(session: Session) -> Brain:
        if simulado:
            return ScriptedBrain(session)
        voice = host.voice is not None
        memory = host.memory.context if host.memory else None
        claude = ClaudeBrain(config, session, voice=voice, memory=memory)
        if host.agents is None:
            return claude
        # Claude, Gemini, ChatGPT y DeepSeek juntos: el principal y el consejo los elige Roman.
        return CouncilBrain(
            claude, host.agents, session, lambda: session.mode, voice=voice, memory=memory
        )

    def make_os_project(request: str) -> ProjectRunner:
        # Solo se usa si `updater` encontró el repo (ver `Host` abajo).
        repo = updater.repo if updater else Path()
        if simulado:
            return ScriptedProject(repo, request, False)
        return OsProject(repo, request, config)

    def make_project(path: Path, request: str, keep_going: bool) -> ProjectRunner:
        if simulado:
            return ScriptedProject(path, request, keep_going)
        return ClaudeProject(path, request, keep_going, config)

    root = Path(proyectos) if proyectos else config.proyectos
    updater = update.from_env()
    host = Host(
        projects=root,
        make_project=make_project,
        make_os_project=make_os_project if updater else None,
        updater=updater,
    )
    if not simulado:
        from jarvis import account
        from jarvis.memory import Memory

        host.memory = Memory()
        host.account_status = account.status
        host.account_login = account.login
        from jarvis import weather

        place = weather.Place(*config.clima) if config.clima else None
        host.weather = lambda: weather.today(place)
        host.agents = AgentHub(
            modelos=config.agentes,
            claude=lambda system, prompt: ask_claude(config.modelo, system, prompt),
        )

    async def main() -> None:
        # Si muere el programa que lo lanzó (`cargo xtask run`), el cerebro también: si no,
        # queda ocupando el puerto y la próxima vez JARVIS le habla a un cerebro viejo.
        parent = os.environ.get("JARVIS_PADRE", "")
        if parent.isdigit():
            asyncio.get_running_loop().create_task(watch_parent(int(parent)))
        if not simulado and not sin_voz:
            host.voice = await start_voice()
        await run_server(puerto or config.puerto, token, make_brain, host)

    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(main())


def setup_logging(config: "Config") -> None:
    """A la consola y a `cerebro.log` (en la carpeta de logs del usuario), para diagnosticar
    aunque la terminal que lo lanzó ya no esté."""
    from logging.handlers import RotatingFileHandler

    fmt = logging.Formatter("cerebro: %(message)s")
    root = logging.getLogger()
    root.setLevel(logging.INFO)
    console = logging.StreamHandler()
    console.setFormatter(fmt)
    root.addHandler(console)
    try:
        config.log_dir().mkdir(parents=True, exist_ok=True)
        f = RotatingFileHandler(
            config.log_dir() / "cerebro.log", maxBytes=1_000_000, backupCount=2, encoding="utf-8"
        )
        f.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))
        root.addHandler(f)
    except OSError:
        pass
    # El Agent SDK avisa que las tools de nivel 1 no pasan por can_use_tool: es a propósito.
    import warnings

    warnings.filterwarnings("ignore", message="can_use_tool will not be invoked")


async def watch_parent(pid: int) -> None:
    import psutil

    while psutil.pid_exists(pid):  # noqa: ASYNC110 - no hay evento para "otro proceso murió"
        await asyncio.sleep(2)
    logging.getLogger("jarvis.serve").info("se cerró JARVIS (proceso %d): me cierro", pid)
    os._exit(0)


async def start_voice() -> "VoiceHub | None":
    """La voz del anfitrión, si están las bibliotecas y los modelos (si no, sigue sin voz)."""
    from jarvis.config import Config
    from jarvis.service.voicehub import VoiceHub
    from jarvis.voice.engine import Voice, VoiceUnavailableError

    try:
        voice = await asyncio.to_thread(Voice, Config.load().voz)
    except VoiceUnavailableError as e:
        logging.getLogger("jarvis.voz").warning("sin voz: %s", e)
        return None
    except Exception:
        logging.getLogger("jarvis.voz").exception("no pude iniciar la voz")
        return None
    hub = VoiceHub(voice, asyncio.get_running_loop())
    hub.start()
    logging.getLogger("jarvis.voz").info('voz lista: decí "JARVIS" (o Win+J en JARVIS-OS)')
    return hub


voz_app = typer.Typer(help="La voz de JARVIS (micrófono y parlantes de esta PC).")
app.add_typer(voz_app, name="voz")


@voz_app.command("instalar")
def voz_instalar(si: bool = typer.Option(False, "--si", help="No preguntar.")) -> None:
    """Baja los modelos de voz (~590 MB en total)."""
    import urllib.request

    from jarvis.voice.engine import VOICES, WHISPER_MODEL, models_dir, voice_url

    typer.echo(
        "Voy a bajar:\n"
        f"  - Whisper {WHISPER_MODEL} (voz a texto): ~470 MB, de Hugging Face\n"
        "  - Piper, las voces jarvis (davefx) y daniela (texto a voz): ~180 MB, de Hugging Face\n"
        f"en {models_dir()} y en la caché de Hugging Face."
    )
    if not si and not typer.confirm("¿Sigo?"):
        raise typer.Exit(1)
    try:
        from faster_whisper import WhisperModel
    except ImportError:
        typer.echo("Primero: uv sync --extra voice", err=True)
        raise typer.Exit(2) from None
    WhisperModel(WHISPER_MODEL, device="cpu", compute_type="int8")
    models_dir().mkdir(parents=True, exist_ok=True)
    for name, (model, _, _, _) in VOICES.items():
        for suffix in (".onnx", ".onnx.json"):
            dest = models_dir() / f"{model}{suffix}"
            if not dest.exists():
                typer.echo(f"bajando {dest.name} (voz {name})...")
                url = voice_url(name).removesuffix(".onnx") + suffix
                urllib.request.urlretrieve(url, dest)  # noqa: S310
    typer.echo('Listo: la próxima vez que arranques JARVIS, decí "JARVIS".')


@voz_app.command("nivel")
def voz_nivel(segundos: int = 10) -> None:
    """Muestra el nivel del micrófono en vivo (para ver si llega tu voz)."""
    import sounddevice as sd

    from jarvis.voice.audio import FRAME, RATE, Segmenter, rms

    seg = Segmenter()
    typer.echo(f"micrófono: {sd.query_devices(kind='input')['name']} (hablá...)")
    with sd.RawInputStream(samplerate=RATE, channels=1, dtype="int16", blocksize=FRAME) as mic:
        for _ in range(segundos * RATE // FRAME):
            data, _ = mic.read(FRAME)
            e = rms(bytes(data))
            seg.feed(bytes(data))
            mark = "VOZ" if e >= seg.threshold() else "   "
            typer.echo(
                f"{mark} {int(e):6d} umbral {int(seg.threshold()):5d} "
                + "#" * min(60, int(e) // 20)
            )


@voz_app.command("decir")
def voz_decir(texto: str, voz: str = "") -> None:
    """Dice un texto con la voz de JARVIS (para probar los parlantes y la voz)."""
    from jarvis.config import Config
    from jarvis.voice.engine import Voice

    Voice(voz or Config.load().voz).speak(texto, lambda n: None)


@voz_app.command("probar")
def voz_probar() -> None:
    """Escucha un pedido (sin palabra de activación), lo transcribe y lo repite en voz alta."""
    from jarvis.voice.engine import Voice, VoiceUnavailableError
    from jarvis.voice.listener import Event

    try:
        voice = Voice()
    except VoiceUnavailableError as e:
        typer.echo(str(e), err=True)
        raise typer.Exit(2) from None
    heard: list[str] = []

    def on_event(ev: Event) -> None:
        if ev.kind == "orden":
            heard.append(ev.text)
            voice.stop()

    typer.echo("Hablá...")
    voice.listen_now()
    voice.run(on_event)
    text = heard[0] if heard else ""
    typer.echo(f"Entendí: {text!r}")
    if text:
        voice.speak(f"Entendí: {text}", lambda n: None)


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""
