import os
from pathlib import Path

import pytest

from jarvis.core.config import JarvisConfig
from jarvis.tools import files
from jarvis.tools._common import ProcResult
from tests.conftest import FakeRun
from tests.helpers import tool_text


def _touch(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("x", encoding="utf-8")
    return path


def _symlink_or_skip(link: Path, target: Path) -> None:
    try:
        os.symlink(target, link, target_is_directory=target.is_dir())
    except OSError:
        pytest.skip("este sistema no permite crear symlinks (Windows sin modo desarrollador)")


# --- find_files -------------------------------------------------------------------------------


async def test_find_busca_por_substring_sin_importar_mayusculas(cfg: JarvisConfig) -> None:
    fac, pro = cfg.allowed_dirs
    tp = _touch(fac / "Algoritmos" / "TP-Final.pdf")
    _touch(pro / "notas.txt")
    text = tool_text(await files.find_files({"pattern": "tp-final"}, cfg))
    assert str(tp) in text
    assert "notas" not in text


async def test_find_acepta_comodines(cfg: JarvisConfig) -> None:
    fac, pro = cfg.allowed_dirs
    _touch(fac / "a.pdf")
    _touch(pro / "b.PDF")
    _touch(pro / "c.txt")
    text = tool_text(await files.find_files({"pattern": "*.pdf"}, cfg))
    assert "a.pdf" in text and "b.PDF" in text and "c.txt" not in text


async def test_find_saltea_carpetas_ocultas(cfg: JarvisConfig) -> None:
    _touch(cfg.allowed_dirs[0] / ".git" / "config")
    text = tool_text(await files.find_files({"pattern": "config"}, cfg))
    assert "No encontré" in text


async def test_find_limita_resultados(cfg: JarvisConfig) -> None:
    for i in range(files.MAX_RESULTS + 5):
        _touch(cfg.allowed_dirs[0] / f"doc{i:03}.md")
    text = tool_text(await files.find_files({"pattern": "*.md"}, cfg))
    assert text.count(".md") == files.MAX_RESULTS
    assert "hay más resultados" in text


@pytest.mark.parametrize("bad", ["", "   ", "../etc/passwd", "a/b", "x" * 300, 42])
async def test_find_rechaza_patrones_invalidos(cfg: JarvisConfig, bad: object) -> None:
    result = await files.find_files({"pattern": bad}, cfg)
    assert result.get("is_error") is True


async def test_find_ignora_symlinks_que_escapan(cfg: JarvisConfig, tmp_path: Path) -> None:
    secreto = _touch(tmp_path / "afuera" / "secreto.txt")
    _symlink_or_skip(cfg.allowed_dirs[0] / "secreto.txt", secreto)
    _symlink_or_skip(cfg.allowed_dirs[0] / "dir_afuera", secreto.parent)
    text = tool_text(await files.find_files({"pattern": "secreto"}, cfg))
    assert "No encontré" in text


# --- trash_file -------------------------------------------------------------------------------


async def test_trash_usa_gio_con_lista_de_argumentos(cfg: JarvisConfig, fake_run: FakeRun) -> None:
    target = _touch(cfg.allowed_dirs[0] / "viejo; rm -rf ~.txt")
    result = await files.trash_file({"path": str(target)}, cfg)
    assert result.get("is_error") is None
    assert fake_run.calls == [["gio", "trash", "--", str(target)]]


@pytest.mark.parametrize(
    "raw",
    [
        "relativo.txt",
        "{fac}/../afuera.txt",
        "{tmp}/afuera.txt",
        "{fac}",
        "{fac}/no-existe.txt",
        "{fac}/con\nsalto.txt",
        "",
    ],
)
async def test_trash_rechaza_rutas_no_permitidas(
    cfg: JarvisConfig, fake_run: FakeRun, tmp_path: Path, raw: str
) -> None:
    _touch(tmp_path / "afuera.txt")
    path = raw.format(fac=cfg.allowed_dirs[0], tmp=tmp_path)
    result = await files.trash_file({"path": path}, cfg)
    assert result.get("is_error") is True
    assert fake_run.calls == []


async def test_trash_no_sigue_directorios_symlink_hacia_afuera(
    cfg: JarvisConfig, fake_run: FakeRun, tmp_path: Path
) -> None:
    _touch(tmp_path / "afuera" / "importante.txt")
    _symlink_or_skip(cfg.allowed_dirs[0] / "atajo", tmp_path / "afuera")
    path = cfg.allowed_dirs[0] / "atajo" / "importante.txt"
    result = await files.trash_file({"path": str(path)}, cfg)
    assert result.get("is_error") is True
    assert fake_run.calls == []


async def test_trash_informa_si_falta_gio(cfg: JarvisConfig, fake_run: FakeRun) -> None:
    fake_run.raises = FileNotFoundError("gio")
    target = _touch(cfg.allowed_dirs[0] / "a.txt")
    result = await files.trash_file({"path": str(target)}, cfg)
    assert result.get("is_error") is True
    assert "gio" in tool_text(result)


async def test_trash_informa_error_de_gio(cfg: JarvisConfig, fake_run: FakeRun) -> None:
    fake_run.result = ProcResult(1, "", "permiso denegado")
    target = _touch(cfg.allowed_dirs[0] / "a.txt")
    result = await files.trash_file({"path": str(target)}, cfg)
    assert result.get("is_error") is True
    assert "permiso denegado" in tool_text(result)
