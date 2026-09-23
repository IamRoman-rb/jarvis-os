"""Tool ``open_app`` (nivel 2): abre una aplicación instalada por su id ``.desktop``."""

import os
import re
from collections.abc import Sequence
from pathlib import Path
from typing import Any

from platformdirs import site_data_path, user_data_path

from jarvis.core.config import JarvisConfig
from jarvis.tools import _common
from jarvis.tools._common import ToolResult, error, ok

_APP_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def validate_app_id(raw: Any) -> str:
    """Solo nombres simples (``firefox-esr``, ``org.gnome.Calculator``): nada de rutas ni args."""
    if not isinstance(raw, str):
        raise ValueError("Falta el id de la aplicación.")
    app_id = raw.strip().removesuffix(".desktop")
    if not _APP_ID_RE.fullmatch(app_id):
        raise ValueError(f"Id de aplicación inválido: {raw!r}.")
    return app_id


def application_dirs() -> list[Path]:
    """Carpetas ``applications/`` de XDG: primero las del usuario, después las del sistema."""
    dirs = [user_data_path() / "applications"]
    system = str(site_data_path(multipath=True)).split(os.pathsep)  # XDG_DATA_DIRS
    dirs += [Path(p) / "applications" for p in system]
    return dirs


def find_desktop_file(app_id: str, dirs: Sequence[Path]) -> Path | None:
    for d in dirs:
        candidate = d / f"{app_id}.desktop"
        if candidate.is_file():
            return candidate
    return None


async def open_app(args: dict[str, Any], cfg: JarvisConfig) -> ToolResult:
    try:
        app_id = validate_app_id(args.get("app_id"))
    except ValueError as e:
        return error(str(e))
    desktop = find_desktop_file(app_id, application_dirs())
    if desktop is None:
        return error(f"No encontré una aplicación instalada con id {app_id}.")
    try:
        result = await _common.run(["gio", "launch", str(desktop)])
    except FileNotFoundError:
        return error("No encontré el comando gio: no puedo abrir aplicaciones en este sistema.")
    if result.returncode != 0:
        return error(f"No pude abrir {app_id}: {result.stderr.strip()[:300]}")
    return ok(f"Abrí {app_id}.")
