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
    from jarvis.nlu.router import LocalFirstBrain, Understanding, load_or_train
    from jarvis.projects import ClaudeProject, OsProject, ProjectRunner, ScriptedProject
    from jarvis.service.server import Host, Session
    from jarvis.service.server import serve as run_server
    from jarvis.tools.system import Gate

    token = os.environ.get("JARVIS_CEREBRO_TOKEN", "")
    if len(token) < 16:
        typer.echo("falta JARVIS_CEREBRO_TOKEN (al menos 16 caracteres)", err=True)
        raise typer.Exit(2)
    config = Config.load()
    setup_logging(config)

    # El entendedor de órdenes propio (C3): se carga (o entrena, unos segundos) en otro hilo;
    # mientras tanto todo va al modelo de lenguaje.
    nlu_box: list[Understanding] = []

    def make_brain(session: Session) -> Brain:
        if simulado:
            return ScriptedBrain(session)
        voice = host.voice is not None
        memory = host.memory.context if host.memory else None
        claude = ClaudeBrain(config, session, voice=voice, memory=memory)
        inner: Brain = claude
        if host.agents is not None:
            # Claude, Gemini, ChatGPT y DeepSeek juntos: el principal y el consejo los elige Roman.
            inner = CouncilBrain(
                claude, host.agents, session, lambda: session.mode, voice=voice, memory=memory
            )
        return LocalFirstBrain(
            inner,
            Gate(session),
            lambda: config.cerebro_local,
            lambda: nlu_box[0] if nlu_box else None,
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
        keep_text=config.diagnostico,
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
        if not simulado and config.cerebro_local != "apagado":

            async def load_nlu() -> None:
                try:
                    model = await asyncio.to_thread(load_or_train)
                except Exception:
                    logging.getLogger("jarvis.nlu").exception("el NLU propio no cargó")
                    return
                nlu_box.append(Understanding(model))
                logging.getLogger("jarvis.nlu").info(
                    "NLU propio listo (modo %s)", config.cerebro_local
                )

            asyncio.get_running_loop().create_task(load_nlu())
        if not simulado and not sin_voz:
            host.voice = await start_voice()
        from jarvis.camera import CameraHub, StillSource

        if not simulado:
            from jarvis.gestures import GestureHub

            # La cámara se abre solo cuando JARVIS-OS activa los gestos.
            host.gestures = GestureHub(asyncio.get_running_loop())
            # La vista de la app Cámara y "bloquear al irme" (ADR 0022): también a pedido.
            host.camera = CameraHub(asyncio.get_running_loop())
        else:
            # Simulado: una foto fija hace de cámara (la app Cámara se prueba sin webcam).
            host.camera = CameraHub(asyncio.get_running_loop(), StillSource)
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
    """Baja los modelos de voz (~730 MB en total)."""
    import urllib.request

    from jarvis.voice.engine import (
        KOKORO_BASE,
        KOKORO_FILES,
        VOICES,
        WHISPER_MODEL,
        kokoro_dir,
        models_dir,
        sha256_file,
        voice_url,
    )

    typer.echo(
        "Voy a bajar:\n"
        f"  - Whisper {WHISPER_MODEL} (voz a texto): ~470 MB, de Hugging Face\n"
        "  - Kokoro-82M (la voz de JARVIS): ~120 MB, de la release de kokoro-onnx en GitHub\n"
        "  - Piper, las voces de respaldo (davefx y daniela): ~180 MB, de Hugging Face\n"
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
    kokoro_dir().mkdir(parents=True, exist_ok=True)
    for name, (digest, size) in KOKORO_FILES.items():
        dest = kokoro_dir() / name
        if not dest.exists():
            typer.echo(f"bajando {name} ({size // 1_000_000} MB)...")
            part = dest.with_suffix(dest.suffix + ".parte")
            urllib.request.urlretrieve(KOKORO_BASE + name, part)  # noqa: S310
            if sha256_file(part) != digest:
                part.unlink()
                typer.echo(f"{name}: el SHA-256 no coincide; no lo uso.", err=True)
                raise typer.Exit(3)
            part.replace(dest)
    for name, spec in VOICES.items():
        if spec.engine != "piper":
            continue
        for suffix in (".onnx", ".onnx.json"):
            dest = models_dir() / f"{spec.model}{suffix}"
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
def voz_decir(
    texto: str,
    voz: str = "",
    wav: str = "",
) -> None:
    """Dice un texto con la voz de JARVIS (para probar los parlantes y la voz). Con `--wav
    archivo.wav` lo guarda en vez de reproducirlo."""
    from jarvis.config import Config
    from jarvis.voice.engine import Voice

    voice = Voice(voz or Config.load().voz)
    if not wav:
        voice.speak(texto, lambda n: None)
        return
    import wave

    pieces: list[bytes] = []
    rate = 0

    def keep(r: int, pcm: bytes) -> None:
        nonlocal rate
        rate = r
        pieces.append(pcm)

    voice.speak_pcm(texto, keep, paced=False)
    with wave.open(wav, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(pieces))
    typer.echo(f"{wav} ({len(b''.join(pieces)) // 2 / max(rate, 1):.1f} s)")


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


nlu_app = typer.Typer(help="El entendedor de órdenes propio de JARVIS (C3).")
app.add_typer(nlu_app, name="nlu")


@nlu_app.command("entrenar")
def nlu_train() -> None:
    """Genera el corpus, entrena la red y la guarda (la usa `jarvis serve`)."""
    import time as _time

    from jarvis.nlu import dataset
    from jarvis.nlu.router import model_path, train

    start = _time.monotonic()
    model = train()
    model.save(model_path())
    typer.echo(
        f"Entrenado en {_time.monotonic() - start:.1f} s con {model.meta.get('train')} frases "
        f"(corpus {dataset.VERSION}). Umbrales: {model.threshold:.2f} sin evidencia, "
        f"{model.threshold_evidence:.2f} con evidencia. Guardado en {model_path()}"
    )


@nlu_app.command("evaluar")
def nlu_evaluate() -> None:
    """Las métricas en el conjunto de prueba (nunca usado para entrenar ni para elegir nada):
    la red contra las reglas de hoy."""
    from jarvis.nlu import baseline, dataset, evaluate
    from jarvis.nlu import slots as slot_rules
    from jarvis.nlu.intents import BY_NAME
    from jarvis.nlu.router import Understanding, load_or_train

    test = dataset.generate("test")
    ood = dataset.ood_heldout()
    understanding = Understanding(load_or_train())
    typer.echo(evaluate.evaluate(understanding.predictor(), test, ood).text("NLU propio (red)"))

    def rules(text: str) -> tuple[str | None, dict[str, str] | None]:
        intent = baseline.predict(text)
        if intent is None:
            return None, None
        return intent, slot_rules.extract(intent, text) if BY_NAME[intent].slots else {}

    typer.echo("")
    typer.echo(evaluate.evaluate(rules, test, ood).text("Reglas de hoy (línea de base)"))


@nlu_app.command("probar")
def nlu_try(frase: str) -> None:
    """Qué haría el NLU con una frase (sin ejecutar nada)."""
    from jarvis.nlu.router import Understanding, load_or_train

    d = Understanding(load_or_train()).decide(frase)
    typer.echo(f"acción: {d.action} · intención: {d.intent or '(se abstiene)'} · mejor: {d.best}")
    typer.echo(f"confianza: {d.confidence:.2f} · tool: {d.tool} · argumentos: {d.args}")
    if d.answer:
        typer.echo(f"respuesta: {d.answer}")
    if d.reason:
        typer.echo(f"motivo: {d.reason}")


@nlu_app.command("informe")
def nlu_report() -> None:
    """Lo que dice el registro de pedidos (C0): cuánto se resolvió sin el modelo de lenguaje."""
    from jarvis.nlu import telemetry

    typer.echo(telemetry.summary(telemetry.read()))


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""
