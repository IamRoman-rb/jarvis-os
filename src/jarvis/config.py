"""Configuración del cerebro: `config.toml` en la carpeta de configuración del usuario.

Ejemplo (todo es opcional):

    modelo = "claude-sonnet-5"
    proyectos = "C:/Users/USER/Documents/proyectos"
    puerto = 8121

    [agentes]  # el modelo de cada agente vinculado (Configuración → Asistente)
    gemini = "gemini-2.5-pro"
    chatgpt = "gpt-5"
    deepseek = "deepseek-reasoner"

    cerebro_local = "sombra"  # el NLU propio: "apagado", "sombra" (solo mide) o "primero"
    diagnostico = false       # true: pedidos.jsonl guarda el texto de cada pedido (C0)

    [clima]  # dónde estás, para el clima del saludo (sin esto, se usa la ubicación de la IP)
    latitud = -34.6
    longitud = -58.4
    lugar = "Buenos Aires"
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
    #: La voz de JARVIS: "cine" (asistente de película: grave, calma, metálica), "jarvis" (grave,
    #: con toque de IA) o "daniela".
    voz: str = "cine"
    #: El modelo de Gemini, ChatGPT o DeepSeek (vacío = el de `agent/providers.py`).
    agentes: dict[str, str] = field(default_factory=dict)
    #: (latitud, longitud, lugar) para el clima; None = la ubicación aproximada de la IP.
    clima: tuple[float, float, str] | None = None
    #: El entendedor de órdenes propio (ADR 0019, C3): "apagado", "sombra" (predice y anota,
    #: no actúa) o "primero" (resuelve solo lo de nivel 1 y las respuestas locales).
    cerebro_local: str = "sombra"
    #: El registro por pedido (C0) guarda el texto del pedido (si no, solo su largo y huella).
    diagnostico: bool = False

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
            voz=str(data.get("voz", "cine")),
            clima=_clima(data.get("clima")),
            cerebro_local=_local_mode(data.get("cerebro_local")),
            diagnostico=data.get("diagnostico") is True,
            agentes={
                str(k): v
                for k, v in (data.get("agentes") or {}).items()
                if isinstance(v, str) and v
            },
        )


def _local_mode(value: object) -> str:
    return (
        value if isinstance(value, str) and value in ("apagado", "sombra", "primero") else "sombra"
    )


def _clima(data: object) -> tuple[float, float, str] | None:
    if not isinstance(data, dict):
        return None
    lat, lon = data.get("latitud"), data.get("longitud")
    if not isinstance(lat, int | float) or not isinstance(lon, int | float):
        return None
    if not (-90 <= lat <= 90 and -180 <= lon <= 180):
        return None
    return float(lat), float(lon), str(data.get("lugar", ""))
