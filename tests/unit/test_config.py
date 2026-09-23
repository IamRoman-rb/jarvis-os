from pathlib import Path

import pytest
from pydantic import ValidationError

from jarvis.core.config import JarvisConfig, load_config


@pytest.fixture
def fake_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))  # Path.home() en Windows
    return home


def test_sin_archivo_usa_defaults(tmp_path: Path) -> None:
    cfg = load_config(tmp_path / "no-existe.toml")
    assert cfg.model == "claude-sonnet-5"
    assert [d.name for d in cfg.allowed_dirs] == ["Facultad", "Proyectos"]
    assert cfg.max_budget_usd > 0


def test_lee_toml_y_expande_home(tmp_path: Path, fake_home: Path) -> None:
    path = tmp_path / "config.toml"
    path.write_text(
        'model = "claude-opus-5-5"\nallowed_dirs = ["~/Materias"]\nmax_turns = 5\n',
        encoding="utf-8",
    )
    cfg = load_config(path)
    assert cfg.model == "claude-opus-5-5"
    assert cfg.allowed_dirs == [(fake_home / "Materias").resolve()]
    assert cfg.max_turns == 5


@pytest.mark.parametrize("bad", ["~/.ssh", "~/.gnupg/claves", "~", "/"])
def test_rechaza_carpetas_prohibidas_o_demasiado_amplias(fake_home: Path, bad: str) -> None:
    with pytest.raises(ValidationError):
        JarvisConfig(allowed_dirs=[Path(bad)])


def test_rechaza_claves_desconocidas(tmp_path: Path) -> None:
    path = tmp_path / "config.toml"
    path.write_text('modelo = "x"\n', encoding="utf-8")
    with pytest.raises(ValidationError):
        load_config(path)
