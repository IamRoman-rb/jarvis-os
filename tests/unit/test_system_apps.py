from pathlib import Path

import pytest

from jarvis.core.config import JarvisConfig
from jarvis.tools import apps, system
from tests.conftest import FakeRun
from tests.helpers import tool_text


async def test_system_info_reporta_lo_basico(cfg: JarvisConfig) -> None:
    text = tool_text(await system.system_info({}, cfg))
    for key in ("sistema", "cpu", "ram", "disco", "encendido hace"):
        assert key in text


@pytest.mark.parametrize(
    "app_id", ["firefox-esr", "org.gnome.Calculator", "xfce4-terminal.desktop"]
)
def test_validate_app_id_acepta_ids_simples(app_id: str) -> None:
    assert apps.validate_app_id(app_id) == app_id.removesuffix(".desktop")


@pytest.mark.parametrize(
    "app_id", ["", "../../bin/sh", "/usr/bin/firefox", "firefox --new-window", "-x", "a;b", 3]
)
def test_validate_app_id_rechaza_rutas_y_argumentos(app_id: object) -> None:
    with pytest.raises(ValueError):
        apps.validate_app_id(app_id)


@pytest.fixture
def app_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    d = tmp_path / "applications"
    d.mkdir()
    monkeypatch.setattr(apps, "application_dirs", lambda: [d])
    return d


async def test_open_app_lanza_con_gio(cfg: JarvisConfig, fake_run: FakeRun, app_dir: Path) -> None:
    desktop = app_dir / "firefox-esr.desktop"
    desktop.write_text("[Desktop Entry]\n", encoding="utf-8")
    result = await apps.open_app({"app_id": "firefox-esr"}, cfg)
    assert result.get("is_error") is None
    assert fake_run.calls == [["gio", "launch", str(desktop)]]


async def test_open_app_no_instalada(cfg: JarvisConfig, fake_run: FakeRun, app_dir: Path) -> None:
    result = await apps.open_app({"app_id": "no-existe"}, cfg)
    assert result.get("is_error") is True
    assert fake_run.calls == []
