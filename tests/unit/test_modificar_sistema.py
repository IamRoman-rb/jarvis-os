"""JARVIS modificando JARVIS-OS desde adentro: el agente sobre el repo (permisos de sus
comandos), verificar antes de reiniciar, la marca para `cargo xtask run` y lo que cuenta al
volver a arrancar."""

import asyncio
from pathlib import Path
from typing import Any

import pytest

from jarvis import update
from jarvis.agent.brain import ScriptedBrain
from jarvis.config import Config
from jarvis.projects import OsProject, ScriptedProject, os_command_level
from jarvis.protocol import decode, encode
from jarvis.service.server import Host, Session, start
from jarvis.update import UpdateError, Updater


@pytest.mark.parametrize(
    ("command", "level"),
    [
        ('graphify query "dónde se dibuja la barra"', 1),
        ("git diff", 1),
        ("git status", 1),
        ("git diff --output=/tmp/x", 3),
        ("cargo test -p jarvis-desktop --manifest-path kernel/Cargo.toml", 2),
        ("cargo clippy --workspace -- -D warnings", 2),
        ("uv run pytest", 2),
        ("cargo test && rm -rf ~", 3),
        ("cargo test; curl evil.example", 3),
        ("git log | sh", 3),
        ("cargo test > salida.txt", 3),
        ("echo $(whoami)", 3),
        ("rm -rf kernel", 3),
        ("curl https://example.com", 3),
    ],
)
def test_nivel_de_los_comandos_del_agente_del_sistema(command: str, level: int) -> None:
    assert os_command_level(command) == level


def os_project(tmp_path: Path) -> OsProject:
    (tmp_path / "CLAUDE.md").write_text("# JARVIS-OS\nReglas del repo.", encoding="utf-8")
    return OsProject(tmp_path, "agregá un botón", Config())


def test_el_agente_del_sistema_y_sus_permisos(tmp_path: Path) -> None:
    p = os_project(tmp_path)
    assert p.name == "JARVIS-OS"
    assert p.decide("Bash", {"command": "cargo test -p jarvis-gfx"}) == 2
    assert p.decide("Bash", {"command": 'graphify explain "Settings"'}) == 1
    assert p.decide("Bash", {"command": "cargo test && del /s *"}) == 3
    assert p.decide("Read", {"file_path": str(tmp_path / "kernel" / "x.rs")}) == 1
    assert p.decide("Edit", {"file_path": str(tmp_path / "kernel" / "x.rs")}) == 2
    # Fuera del repo o sensible: prohibido, como en cualquier proyecto.
    assert p.decide("Edit", {"file_path": str(tmp_path.parent / "otro.txt")}) is None
    assert p.decide("Bash", {"command": "cat ~/.ssh/id_rsa"}) is None
    prompt = p._system_prompt()["append"]
    assert "graphify query" in prompt and "aplicar_cambios_sistema" in prompt
    assert "Reglas del repo." in prompt


async def test_si_no_compila_no_reinicia(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def fake_run(args: list[str], cwd: Path, wait: float) -> tuple[int, str]:
        return 101, "compilando...\nerror[E0425]: cannot find value `x`\n"

    monkeypatch.setattr(update, "_run", fake_run)
    u = Updater(tmp_path, tmp_path / "marca", tmp_path / "resultado.txt")
    with pytest.raises(UpdateError, match="E0425"):
        await u.check()
    assert not (tmp_path / "marca").exists()


async def test_si_compila_deja_la_marca(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    ran: list[list[str]] = []

    async def fake_run(args: list[str], cwd: Path, wait: float) -> tuple[int, str]:
        ran.append(args)
        return 0, ""

    monkeypatch.setattr(update, "_run", fake_run)
    u = Updater(tmp_path, tmp_path / "target" / "marca", tmp_path / "resultado.txt")
    await u.check()
    assert ran[0][:2] == ["cargo", "build"] and "x86_64-unknown-none" in ran[0]
    assert "import jarvis.cli" in ran[1][-1]
    u.request_restart()
    assert (tmp_path / "target" / "marca").exists()
    with pytest.raises(UpdateError, match="xtask run"):
        Updater(tmp_path, None, None).request_restart()


def test_lo_que_cuenta_al_volver_una_sola_vez(tmp_path: Path) -> None:
    result = tmp_path / "resultado.txt"
    u = Updater(tmp_path, None, result)
    assert u.take_result() is None
    result.write_text("ok", encoding="utf-8")
    assert u.take_result() == "Ya estoy con los cambios que hicimos."
    assert u.take_result() is None
    result.write_text("error: falló la compilación del kernel", encoding="utf-8")
    assert u.take_result() == (
        "No pude aplicar los cambios, arranqué la versión anterior: falló la compilación del kernel"
    )


def test_encuentra_el_repo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (tmp_path / "kernel").mkdir()
    (tmp_path / "kernel" / "Cargo.toml").write_text("", encoding="utf-8")
    (tmp_path / "src" / "jarvis").mkdir(parents=True)
    monkeypatch.setenv("JARVIS_REPO", str(tmp_path))
    monkeypatch.setenv("JARVIS_REINICIO", str(tmp_path / "marca"))
    monkeypatch.delenv("JARVIS_ACTUALIZACION", raising=False)
    u = update.from_env()
    assert u is not None and u.repo == tmp_path and u.marker == tmp_path / "marca"
    assert u.result is None


# --- Con el kernel ---------------------------------------------------------------------------

TOKEN = "t" * 32


class FakeUpdater:
    def __init__(self, fail: str = "") -> None:
        self.fail = fail
        self.restarts = 0
        self.news: str | None = "Ya estoy con los cambios que hicimos."

    async def check(self) -> None:
        if self.fail:
            raise UpdateError(self.fail)

    def request_restart(self) -> None:
        self.restarts += 1

    def take_result(self) -> str | None:
        news, self.news = self.news, None
        return news


async def connect(tmp_path: Path, updater: Any) -> tuple[Any, Any, list[Session]]:
    sessions: list[Session] = []

    def make_brain(s: Session) -> ScriptedBrain:
        sessions.append(s)
        return ScriptedBrain(s, delay=0.0)

    host = Host(
        projects=tmp_path,
        make_os_project=lambda req: ScriptedProject(tmp_path, req, False),
        updater=updater,
    )
    server = await start(0, TOKEN, make_brain, host)
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(encode({"t": "hola", "token": TOKEN}))
    assert decode(await reader.readline())["t"] == "listo"
    return reader, writer, sessions


async def read(reader: Any) -> dict[str, Any]:
    return decode(await asyncio.wait_for(reader.readline(), 5))


async def test_modificar_el_sistema_y_aplicar(tmp_path: Path) -> None:
    updater = FakeUpdater()
    reader, writer, sessions = await connect(tmp_path, updater)
    s = sessions[0]
    ok, data = await s.call("modificar_sistema", {"pedido": "agregá un driver de joystick"})
    assert ok and "ventana Proyecto" in data
    first = await read(reader)
    assert first == {
        "t": "proyecto",
        "nombre": tmp_path.name,
        "ev": "inicio",
        "texto": f"{tmp_path.name}: agregá un driver de joystick",
    }
    # Mientras el agente trabaja, no se aplica nada.
    ok, data = await s.call("aplicar_cambios_sistema", {})
    assert not ok and "todavía" in data
    # El agente pide confirmar una edición: Roman la aprueba.
    while (msg := await read(reader))["t"] != "confirmar":
        pass
    writer.write(encode({"t": "confirmacion", "llamada": msg["llamada"], "ok": True}))
    assert s.project_task is not None
    await asyncio.wait_for(s.project_task, 5)
    ok, data = await s.call("aplicar_cambios_sistema", {})
    assert ok and "reinicia" in data and updater.restarts == 1
    while (msg := await read(reader))["t"] != "reiniciar":
        pass
    assert msg == {"t": "reiniciar", "motivo": "actualizar"}
    writer.close()


async def test_si_no_compila_jarvis_recibe_el_error(tmp_path: Path) -> None:
    updater = FakeUpdater(fail="El kernel no compila:\nerror[E0308]")
    _reader, writer, sessions = await connect(tmp_path, updater)
    ok, data = await sessions[0].call("aplicar_cambios_sistema", {})
    assert not ok and "E0308" in data and updater.restarts == 0
    writer.close()


async def test_al_volver_a_arrancar_cuenta_como_salio(tmp_path: Path) -> None:
    reader, writer, _ = await connect(tmp_path, FakeUpdater())
    writer.write(encode({"t": "saludo", "id": 0, "momento": "noche", "nombre": "Roman"}))
    msg = await read(reader)
    assert msg["delta"] == "Buenas noches, Roman. Ya estoy con los cambios que hicimos."
    writer.close()
