"""Los permisos de las tools (docs/permisos.md): nivel declarado, confirmaciones y auditoría."""

import json
from pathlib import Path
from typing import Any

import pytest

from jarvis.policy import audit as audit_mod
from jarvis.policy.levels import (
    BUILTIN_LEVELS,
    LEVELS,
    MCP_PREFIX,
    UnknownToolError,
    auto_approved,
    level_of,
)
from jarvis.tools.system import SPECS, Gate, describe


class FakeKernel:
    def __init__(self, answer: bool) -> None:
        self.answer = answer
        self.asked: list[tuple[str, int, str]] = []
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        self.calls.append((tool, args))
        return True, "ok"

    async def confirm(self, tool: str, level: int, description: str) -> bool:
        self.asked.append((tool, level, description))
        return self.answer


@pytest.fixture(autouse=True)
def audit_file(tmp_path: Path) -> Path:
    p = tmp_path / "auditoria.jsonl"
    audit_mod.set_path(p)
    yield p
    audit_mod.set_path(None)


def test_cada_tool_tiene_nivel_y_la_tabla_de_permisos_la_lista() -> None:
    assert {s[0] for s in SPECS} == set(LEVELS)
    table = Path("docs/permisos.md").read_text(encoding="utf-8")
    for name, level in {**LEVELS, **BUILTIN_LEVELS}.items():
        assert f"| `{name}` | {level} |" in table, name


def test_solo_el_nivel_1_se_aprueba_solo() -> None:
    assert set(auto_approved()) == {MCP_PREFIX + n for n, lvl in LEVELS.items() if lvl == 1} | {
        n for n, lvl in BUILTIN_LEVELS.items() if lvl == 1
    }
    assert MCP_PREFIX + "a_papelera" not in auto_approved()
    with pytest.raises(UnknownToolError):
        level_of("Bash")
    # De Claude Code, solo leer la web: nada que escriba ni ejecute.
    assert set(BUILTIN_LEVELS) == {"WebSearch", "WebFetch"}
    assert level_of("WebFetch") == 1


async def test_nivel_1_no_pregunta() -> None:
    k = FakeKernel(answer=False)
    assert await Gate(k).run("buscar_web", {"consulta": "rust"}) == (True, "ok")
    assert k.asked == []


async def test_nivel_3_pregunta_y_si_no_no_se_hace(audit_file: Path) -> None:
    k = FakeKernel(answer=False)
    ok, _ = await Gate(k).run("a_papelera", {"ruta": "/Documentos/tesis.txt"})
    assert not ok and k.calls == []
    assert k.asked == [("a_papelera", 3, "Mover a la Papelera: /Documentos/tesis.txt.")]
    lines = audit_file.read_text(encoding="utf-8").splitlines()  # noqa: ASYNC240
    entry = json.loads(lines[-1])
    assert entry["evento"] == "rechazada" and entry["nivel"] == 3


async def test_una_tool_desconocida_se_rechaza_sin_preguntar() -> None:
    k = FakeKernel(answer=True)
    assert not await Gate(k).permit("mcp__otro__Bash", {"command": "rm -rf /"})
    assert k.asked == []


def test_el_comando_se_muestra_entero() -> None:
    cmd = "apt install " + "paquete-" * 40
    assert describe("ejecutar_comando", {"comando": cmd}).endswith(cmd)
