"""Validación de rutas contra la allowlist. Toda tool que recibe una ruta pasa por acá."""

from collections.abc import Sequence
from pathlib import Path

from jarvis.core.config import forbidden_dirs


class PathNotAllowedError(ValueError):
    """La ruta está fuera de lo que JARVIS puede tocar."""


def is_inside(path: Path, roots: Sequence[Path]) -> bool:
    """True si ``path`` (ya resuelta) está dentro de alguna raíz y no en una ruta prohibida."""
    if any(path == bad or path.is_relative_to(bad) for bad in forbidden_dirs()):
        return False
    return any(path.is_relative_to(root) for root in roots)


def resolve_inside_allowlist(raw: str, allowed_dirs: Sequence[Path]) -> Path:
    """Valida una ruta pedida por el modelo y devuelve su forma absoluta.

    Se resuelven los symlinks de los directorios padres (así un ``~/Facultad/atajo -> /etc``
    no sirve para escapar), pero no el del último componente: si la ruta es un symlink,
    la operación actúa sobre el link y no sobre su destino.
    """
    if not raw or any(c in raw for c in "\n\r\0"):
        raise PathNotAllowedError("Ruta vacía o con caracteres de control.")
    path = Path(raw).expanduser()
    if not path.is_absolute():
        raise PathNotAllowedError(f"La ruta tiene que ser absoluta: {raw!r}.")
    if ".." in path.parts:
        raise PathNotAllowedError(f"La ruta no puede contener '..': {raw!r}.")
    if path.name in ("", "."):
        raise PathNotAllowedError(f"Ruta inválida: {raw!r}.")

    candidate = path.parent.resolve() / path.name
    if not is_inside(candidate, allowed_dirs):
        raise PathNotAllowedError(f"{candidate} está fuera de las carpetas permitidas.")
    if candidate in allowed_dirs:
        raise PathNotAllowedError(f"{candidate} es una carpeta raíz permitida: no se toca entera.")
    return candidate
