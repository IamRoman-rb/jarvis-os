"""Comando `jarvis`."""

import asyncio
import os
from pathlib import Path
from typing import Annotated

import typer
from claude_agent_sdk import ClaudeSDKError
from pydantic import ValidationError

from jarvis import __version__
from jarvis.agent.brain import Brain, BrainError
from jarvis.core.config import load_config
from jarvis.policy.confirm import TerminalConfirmer

app = typer.Typer(help="JARVIS: asistente de JARVIS-OS.", no_args_is_help=True)


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""


@app.command()
def version() -> None:
    """Muestra la versión instalada de JARVIS."""
    typer.echo(f"jarvis {__version__}")


@app.command()
def ask(
    texto: Annotated[str, typer.Option("--texto", "-t", help="Pedido para JARVIS.")],
    config: Annotated[
        Path | None, typer.Option("--config", help="Ruta a un config.toml alternativo.")
    ] = None,
) -> None:
    """Le hace un pedido a JARVIS por texto."""
    if not os.environ.get("ANTHROPIC_API_KEY"):
        typer.echo("Falta ANTHROPIC_API_KEY en el entorno. Ver README: 'Probar JARVIS'.", err=True)
        raise typer.Exit(2)
    try:
        cfg = load_config(config)
    except (ValidationError, OSError, ValueError) as e:
        typer.echo(f"Configuración inválida: {e}", err=True)
        raise typer.Exit(2) from None

    brain = Brain(cfg, confirmer=TerminalConfirmer())
    try:
        answer = asyncio.run(brain.ask(texto))
    except BrainError as e:
        typer.echo(str(e), err=True)
        raise typer.Exit(1) from None
    except ClaudeSDKError as e:
        # Solo el tipo: el detalle puede incluir la salida cruda del proceso del SDK.
        typer.echo(f"Falló la conexión con Claude ({type(e).__name__}).", err=True)
        raise typer.Exit(1) from None
    typer.echo(answer)
