"""Proyectos: resolver el nombre, límites de rutas y el nivel de cada herramienta del agente."""

from pathlib import Path

import pytest

from jarvis.projects import ProjectError, decide, list_projects, path_allowed, resolve_project


@pytest.fixture
def root(tmp_path: Path) -> Path:
    for name in ["jarvis-os", "web-seguros", "sistemaSeguros", ".oculto"]:
        (tmp_path / name).mkdir()
    return tmp_path


def test_lista_y_resuelve(root: Path) -> None:
    assert list_projects(root) == ["jarvis-os", "sistemaSeguros", "web-seguros"]
    assert resolve_project(root, "JARVIS-OS").name == "jarvis-os"
    assert resolve_project(root, "jarvis").name == "jarvis-os"
    with pytest.raises(ProjectError, match="puede ser"):
        resolve_project(root, "seguros")
    for bad in ["../x", ".oculto", "", "no-existe"]:
        with pytest.raises(ProjectError):
            resolve_project(root, bad)


def test_rutas_del_agente(root: Path) -> None:
    p = root / "jarvis-os"
    assert path_allowed(p, "src/main.rs")
    assert path_allowed(p, str(p / "README.md"))
    assert not path_allowed(p, "../web-seguros/secreto.txt")
    assert not path_allowed(p, str(Path.home() / ".ssh" / "id_ed25519"))


def test_niveles_del_agente(root: Path) -> None:
    p = root / "jarvis-os"
    assert decide(p, "Read", {"file_path": "src/lib.rs"}) == 1
    assert decide(p, "Edit", {"file_path": "src/lib.rs"}) == 2
    assert decide(p, "Bash", {"command": "cargo test"}) == 3
    assert decide(p, "Edit", {"file_path": "../otro/x"}) is None
    assert decide(p, "Bash", {"command": "cat ~/.ssh/id_rsa"}) is None
