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
    def __init__(self, answer: bool, result: tuple[bool, str] = (True, "ok")) -> None:
        self.answer = answer
        self.result = result
        self.asked: list[tuple[str, int, str]] = []
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def call(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        self.calls.append((tool, args))
        return self.result

    async def confirm(self, tool: str, level: int, description: str) -> bool:
        self.asked.append((tool, level, description))
        return self.answer


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


def entries(audit_file: Path) -> list[dict[str, Any]]:
    lines = audit_file.read_text(encoding="utf-8").splitlines()
    return [json.loads(line) for line in lines]


async def test_la_auditoria_anota_como_termino_cada_accion(audit_file: Path) -> None:
    k = FakeKernel(answer=True, result=(True, "hola mundo"))
    assert await Gate(k).run("escribir_archivo", {"ruta": "/a.txt", "contenido": "hola"})
    k.result = (False, "No existe /Nada")
    assert await Gate(k).execute("leer_archivo", {"ruta": "/Nada"}) == (False, "No existe /Nada")

    log = entries(audit_file)
    assert [(e["evento"], e["tool"]) for e in log] == [
        ("aprobada", "escribir_archivo"),
        ("ejecutada", "escribir_archivo"),
        ("ejecutada", "leer_archivo"),
    ]
    done, failed = log[1], log[2]
    assert done["ok"] is True and done["largo"] == len("hola mundo") and done["ms"] >= 0
    assert "error" not in done
    # El resultado no se guarda (puede ser un archivo de Roman); el error sí, para saber qué pasó.
    assert "hola mundo" not in json.dumps(done)
    assert failed["ok"] is False and failed["error"] == "No existe /Nada"


def test_la_auditoria_no_guarda_datos_privados_ni_secretos() -> None:
    texto = "x" * 900
    got = audit_mod.redact(
        {
            "ruta": "/Documentos/diario.txt",
            "contenido": "lo que escribí hoy",
            "dato": "mi DNI es 40.123.456",
            "comando": "export JARVIS_CEREBRO_TOKEN=abc123 && curl -H 'Authorization: Bearer "
            "zzz' x " + "y" * 600,
            "pregunta": "¿sirve sk-ant-api03-AbC_d-123? " + texto,
        }
    )
    assert got["ruta"] == "/Documentos/diario.txt"
    assert got["contenido"] == "<18 caracteres>"
    assert got["dato"] == "<20 caracteres>"
    assert "abc123" not in got["comando"] and "zzz" not in got["comando"]
    assert "JARVIS_CEREBRO_TOKEN=***" in got["comando"] and "Bearer ***" in got["comando"]
    assert got["comando"].endswith("y" * 600), "el comando no se recorta"
    assert "sk-ant" not in got["pregunta"]
    # "¿sirve ***? " (12) + 900 = 912 caracteres: quedan 500 y la cuenta de lo recortado.
    assert got["pregunta"].startswith("¿sirve ***? ") and got["pregunta"].endswith("…(+412)")


def test_la_auditoria_rota_al_llenarse(audit_file: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(audit_mod, "MAX_BYTES", 300)
    for i in range(10):
        audit_mod.audit("aprobada", "buscar_web", {"consulta": f"pedido {i}"}, nivel=1)
    old = audit_file.with_suffix(".1.jsonl")
    assert old.exists() and audit_file.exists()
    assert audit_file.stat().st_size < 300 + 200
    # Lo último está en el archivo nuevo; no se pierde nada reciente.
    assert entries(audit_file)[-1]["args"] == {"consulta": "pedido 9"}
