"""Configuración del cerebro: `config.toml` en la carpeta de configuración del usuario.

Ejemplo (todo es opcional):

    modelo = "claude-sonnet-5"
    proyectos = "C:/Users/USER/Documents/proyectos"
    puerto = 8121
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from platformdirs import user_config_path, user_log_path

DEFAULT_PORT = 8121


def _default_projects() -> Path:
    return Path.home() / "Documents" / "proyectos"


@dataclass(frozen=True)
class Config:
    #: `None` = el modelo por defecto de Claude Code.
    modelo: str | None = None
    proyectos: Path = field(default_factory=_default_projects)
    puerto: int = DEFAULT_PORT
    #: La voz de JARVIS: "jarvis" (grave, con toque de IA) o "daniela".
    voz: str = "jarvis"

    @staticmethod
    def path() -> Path:
        return user_config_path("jarvis") / "config.toml"

    @staticmethod
    def log_dir() -> Path:
        return user_log_path("jarvis")

    @classmethod
    def load(cls, path: Path | None = None) -> Config:
        p = path or cls.path()
        if not p.exists():
            return cls()
        data = tomllib.loads(p.read_text(encoding="utf-8"))
        modelo = data.get("modelo")
        puerto = data.get("puerto", DEFAULT_PORT)
        proyectos = data.get("proyectos")
        return cls(
            modelo=modelo if isinstance(modelo, str) and modelo else None,
            proyectos=Path(proyectos) if isinstance(proyectos, str) else _default_projects(),
            puerto=puerto if isinstance(puerto, int) and 0 < puerto < 65536 else DEFAULT_PORT,
            voz=str(data.get("voz", "jarvis")),
        )
