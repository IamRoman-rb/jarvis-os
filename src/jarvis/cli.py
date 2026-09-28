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
    from jarvis.agent.brain import Brain, ClaudeBrain, ScriptedBrain
    from jarvis.config import Config
    from jarvis.projects import ClaudeProject, ProjectRunner, ScriptedProject
    from jarvis.service.server import Host, Session
    from jarvis.service.server import serve as run_server

    token = os.environ.get("JARVIS_CEREBRO_TOKEN", "")
    if len(token) < 16:
        typer.echo("falta JARVIS_CEREBRO_TOKEN (al menos 16 caracteres)", err=True)
        raise typer.Exit(2)
    config = Config.load()
    logging.basicConfig(level=logging.INFO, format="cerebro: %(message)s")

    def make_brain(session: Session) -> Brain:
        return ScriptedBrain(session) if simulado else ClaudeBrain(config, session)

    def make_project(path: Path, request: str, keep_going: bool) -> ProjectRunner:
        if simulado:
            return ScriptedProject(path, request, keep_going)
        return ClaudeProject(path, request, keep_going, config)

    root = Path(proyectos) if proyectos else config.proyectos
    host = Host(projects=root, make_project=make_project)

    async def main() -> None:
        if not simulado and not sin_voz:
            host.voice = await start_voice()
        await run_server(puerto or config.puerto, token, make_brain, host)

    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(main())


async def start_voice() -> "VoiceHub | None":
    """La voz del anfitrión, si están las bibliotecas y los modelos (si no, sigue sin voz)."""
    from jarvis.service.voicehub import VoiceHub
    from jarvis.voice.engine import Voice, VoiceUnavailableError

    try:
        voice = await asyncio.to_thread(Voice)
    except VoiceUnavailableError as e:
        logging.getLogger("jarvis.voz").info("sin voz: %s", e)
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

    from jarvis.voice.engine import PIPER_URL, PIPER_VOICE, WHISPER_MODEL, models_dir

    typer.echo(
        "Voy a bajar:\n"
        f"  - Whisper {WHISPER_MODEL} (voz a texto): ~470 MB, de Hugging Face\n"
        f"  - Piper {PIPER_VOICE} (texto a voz): ~115 MB, de Hugging Face\n"
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
    for suffix in (".onnx", ".onnx.json"):
        dest = models_dir() / f"{PIPER_VOICE}{suffix}"
        if not dest.exists():
            typer.echo(f"bajando {dest.name}...")
            urllib.request.urlretrieve(PIPER_URL.removesuffix(".onnx") + suffix, dest)  # noqa: S310
    typer.echo('Listo: la próxima vez que arranques JARVIS, decí "JARVIS".')


@voz_app.command("probar")
def voz_probar() -> None:
    """Escucha un pedido (sin palabra de activación), lo transcribe y lo repite en voz alta."""
    from jarvis.voice.engine import Voice, VoiceUnavailableError

    try:
        voice = Voice()
    except VoiceUnavailableError as e:
        typer.echo(str(e), err=True)
        raise typer.Exit(2) from None
    heard: list[str] = []

    def on_heard(text: str) -> None:
        heard.append(text)
        voice.stop()

    typer.echo("Hablá...")
    voice.listen_now()
    voice.run(on_heard, lambda on: None)
    text = heard[0] if heard else ""
    typer.echo(f"Entendí: {text!r}")
    if text:
        voice.speak(f"Entendí: {text}", lambda n: None)


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""
