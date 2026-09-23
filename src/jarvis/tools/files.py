"""Tools de archivos: ``find_files`` (nivel 1) y ``trash_file`` (nivel 3)."""

import asyncio
import fnmatch
import os
from collections.abc import Sequence
from pathlib import Path
from typing import Any

from jarvis.core.config import JarvisConfig
from jarvis.policy.paths import PathNotAllowedError, is_inside, resolve_inside_allowlist
from jarvis.tools import _common
from jarvis.tools._common import ToolResult, error, ok

MAX_RESULTS = 50
MAX_PATTERN_LEN = 200


# --- find_files -------------------------------------------------------------------------------


def validate_pattern(raw: Any) -> str:
    """Devuelve un patrón glob en minúsculas. Sin comodines, busca como substring."""
    if not isinstance(raw, str) or not raw.strip():
        raise ValueError("Falta el patrón de búsqueda.")
    pattern = raw.strip()
    if len(pattern) > MAX_PATTERN_LEN:
        raise ValueError("El patrón es demasiado largo.")
    if any(c in pattern for c in "/\\\n\r\0"):
        raise ValueError("El patrón es un nombre de archivo, no una ruta.")
    if not any(c in pattern for c in "*?["):
        pattern = f"*{pattern}*"
    return pattern.lower()


def search(
    pattern: str, roots: Sequence[Path], limit: int = MAX_RESULTS
) -> tuple[list[Path], bool]:
    """Busca archivos cuyo nombre coincida. Devuelve (resultados, ¿se cortó por el límite?).

    No sigue symlinks de directorios, saltea carpetas ocultas (.git, .cache…) y descarta
    archivos que, resueltos, apuntan fuera de la allowlist.
    """
    results: list[Path] = []
    for root in roots:
        if not root.is_dir():
            continue
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
            for name in sorted(filenames):
                if not fnmatch.fnmatch(name.lower(), pattern):
                    continue
                path = Path(dirpath) / name
                if not is_inside(path.resolve(), roots):
                    continue
                if len(results) >= limit:
                    return results, True
                results.append(path)
    return results, False


async def find_files(args: dict[str, Any], cfg: JarvisConfig) -> ToolResult:
    try:
        pattern = validate_pattern(args.get("pattern"))
    except ValueError as e:
        return error(str(e))
    results, truncated = await asyncio.to_thread(search, pattern, cfg.allowed_dirs)
    if not results:
        return ok("No encontré archivos con ese nombre en las carpetas permitidas.")
    lines = [str(p) for p in results]
    if truncated:
        lines.append(f"(hay más resultados; mostré los primeros {MAX_RESULTS})")
    return ok("\n".join(lines))


# --- trash_file -------------------------------------------------------------------------------


def validate_trash_path(raw: Any, cfg: JarvisConfig) -> Path:
    if not isinstance(raw, str):
        raise PathNotAllowedError("Falta la ruta del archivo.")
    path = resolve_inside_allowlist(raw, cfg.allowed_dirs)
    if not os.path.lexists(path):
        raise PathNotAllowedError(f"No existe {path}.")
    return path


async def trash_file(args: dict[str, Any], cfg: JarvisConfig) -> ToolResult:
    """Mueve a la papelera con ``gio trash``. Nunca borra definitivamente."""
    try:
        path = validate_trash_path(args.get("path"), cfg)
    except PathNotAllowedError as e:
        return error(str(e))
    try:
        result = await _common.run(["gio", "trash", "--", str(path)])
    except FileNotFoundError:
        return error("No encontré el comando gio: no puedo usar la papelera en este sistema.")
    if result.returncode != 0:
        return error(f"No pude mover {path} a la papelera: {result.stderr.strip()[:300]}")
    return ok(f"Moví {path} a la papelera.")
