"""Comando `jarvis`."""

import asyncio
import contextlib
import logging
import os

import typer

from jarvis import __version__

app = typer.Typer(help="JARVIS: asistente de JARVIS-OS.", no_args_is_help=True)


@app.command()
def version() -> None:
    """Muestra la versión instalada de JARVIS."""
    typer.echo(f"jarvis {__version__}")


@app.command()
def serve(
    puerto: int = typer.Option(0, help="Puerto en 127.0.0.1 (0 = el de config.toml, 8121)."),
    simulado: bool = typer.Option(False, help="Respuestas fijas, sin Claude (para los tests)."),
) -> None:
    """El cerebro para el kernel de JARVIS-OS (lo levanta `cargo xtask run`).

    El token de la sesión llega por la variable JARVIS_CEREBRO_TOKEN (no por la línea de
    comandos, que la ven los demás procesos).
    """
    from jarvis.agent.brain import Brain, ClaudeBrain, ScriptedBrain
    from jarvis.config import Config
    from jarvis.service.server import Session
    from jarvis.service.server import serve as run_server

    token = os.environ.get("JARVIS_CEREBRO_TOKEN", "")
    if len(token) < 16:
        typer.echo("falta JARVIS_CEREBRO_TOKEN (al menos 16 caracteres)", err=True)
        raise typer.Exit(2)
    config = Config.load()
    logging.basicConfig(level=logging.INFO, format="cerebro: %(message)s")

    def make_brain(session: Session) -> Brain:
        return ScriptedBrain(session) if simulado else ClaudeBrain(config, session)

    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(run_server(puerto or config.puerto, token, make_brain))


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""
