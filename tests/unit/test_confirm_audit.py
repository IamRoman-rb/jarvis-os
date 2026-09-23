import pytest

from jarvis.policy.audit import AuditEvent, AuditLog
from jarvis.policy.confirm import TerminalConfirmer
from jarvis.policy.levels import Level, Origin


def _confirmer(answer: str, shown: list[str]) -> TerminalConfirmer:
    return TerminalConfirmer(read=lambda _prompt: answer, write=shown.append)


@pytest.mark.parametrize(
    ("level", "answer", "expected"),
    [
        (Level.REVERSIBLE, "s", True),
        (Level.REVERSIBLE, "SI", True),
        (Level.REVERSIBLE, "", False),
        (Level.REVERSIBLE, "n", False),
        (Level.CRITICAL, "s", False),  # nivel 3 exige escribir "si" completo
        (Level.CRITICAL, "si", True),
        (Level.CRITICAL, "sí", True),
        (Level.CRITICAL, "dale", False),
    ],
)
async def test_terminal_confirmer(level: Level, answer: str, expected: bool) -> None:
    shown: list[str] = []
    assert await _confirmer(answer, shown)("trash_file", {}, level, Origin.LOCAL) is expected


async def test_terminal_confirmer_muestra_argumentos_completos() -> None:
    shown: list[str] = []
    ruta = "/home/roman/Facultad/" + "x" * 500 + ".txt"
    await _confirmer("n", shown)("trash_file", {"path": ruta}, Level.CRITICAL, Origin.LOCAL)
    assert ruta in shown[0]


async def test_terminal_confirmer_eof_es_no() -> None:
    def _eof(_prompt: str) -> str:
        raise EOFError

    confirmer = TerminalConfirmer(read=_eof, write=lambda _s: None)
    assert await confirmer("open_app", {}, Level.REVERSIBLE, Origin.LOCAL) is False


async def test_audit_guarda_y_lee(audit: AuditLog) -> None:
    await audit.record(
        origin="local",
        tool="trash_file",
        level=3,
        tool_input={"path": "/home/roman/Facultad/ñandú.txt"},
        event=AuditEvent.DENIED,
    )
    [entry] = await audit.entries()
    assert entry.tool == "trash_file"
    assert entry.level == 3
    assert entry.tool_input == {"path": "/home/roman/Facultad/ñandú.txt"}
    assert entry.event is AuditEvent.DENIED
