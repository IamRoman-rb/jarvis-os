"""Configuración de JARVIS.

Se lee de ``~/.config/jarvis/config.toml`` (ruta XDG resuelta por platformdirs). Si el archivo no
existe se usan los valores por defecto, así JARVIS arranca sin configuración previa.

Ejemplo de ``config.toml``::

    model = "claude-sonnet-5"
    allowed_dirs = ["~/Facultad", "~/Proyectos"]
    max_budget_usd = 0.5
"""

import tomllib
from pathlib import Path

from platformdirs import user_config_path, user_state_path
from pydantic import BaseModel, ConfigDict, Field, field_validator

APP_NAME = "jarvis"


def forbidden_dirs() -> list[Path]:
    """Rutas que ninguna tool puede tocar, aunque el usuario las agregue a la allowlist."""
    home = Path.home()
    return [(home / ".ssh").resolve(), (home / ".gnupg").resolve(), Path("/etc").resolve()]


def default_config_path() -> Path:
    return user_config_path(APP_NAME) / "config.toml"


def _default_audit_db() -> Path:
    return user_state_path(APP_NAME) / "audit.sqlite3"


def _default_allowed_dirs() -> list[Path]:
    return [Path("~/Facultad"), Path("~/Proyectos")]


def _overlaps(a: Path, b: Path) -> bool:
    return a == b or a.is_relative_to(b) or b.is_relative_to(a)


class JarvisConfig(BaseModel):
    """Configuración tipada. Las rutas se guardan expandidas y absolutas."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    model: str = "claude-sonnet-5"
    allowed_dirs: list[Path] = Field(default_factory=_default_allowed_dirs)
    max_turns: int = Field(default=10, ge=1, le=50)
    max_budget_usd: float = Field(default=0.5, gt=0)
    audit_db: Path = Field(default_factory=_default_audit_db)

    @field_validator("allowed_dirs")
    @classmethod
    def _validate_allowed_dirs(cls, dirs: list[Path]) -> list[Path]:
        resolved = [d.expanduser().resolve() for d in dirs]
        for d in resolved:
            for bad in forbidden_dirs():
                if _overlaps(d, bad):
                    raise ValueError(f"La carpeta {d} no puede estar en la allowlist ({bad}).")
            if d == Path(d.anchor) or d == Path.home().resolve():
                raise ValueError(f"La allowlist no puede incluir {d} entera: es demasiado amplia.")
        return resolved

    @field_validator("audit_db")
    @classmethod
    def _expand_audit_db(cls, path: Path) -> Path:
        return path.expanduser()


def load_config(path: Path | None = None) -> JarvisConfig:
    """Carga la configuración desde ``path`` (o la ruta XDG por defecto)."""
    path = path or default_config_path()
    if not path.exists():
        return JarvisConfig()
    with path.open("rb") as f:
        data = tomllib.load(f)
    return JarvisConfig.model_validate(data)
