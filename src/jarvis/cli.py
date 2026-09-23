"""Comando `jarvis`. Por ahora solo informa la versión; `ask` llega en la fase 1a."""

import typer

from jarvis import __version__

app = typer.Typer(help="JARVIS: asistente de JARVIS-OS.", no_args_is_help=True)


@app.command()
def version() -> None:
    """Muestra la versión instalada de JARVIS."""
    typer.echo(f"jarvis {__version__}")


@app.callback()
def main() -> None:
    """JARVIS: asistente de JARVIS-OS."""
