"""Lo común a todos los tests."""

from collections.abc import Iterator
from pathlib import Path

import pytest

from jarvis.nlu import telemetry
from jarvis.policy import audit as audit_mod


@pytest.fixture(autouse=True)
def audit_file(tmp_path: Path) -> Iterator[Path]:
    """La auditoría de cada test va a un archivo propio, nunca al `auditoria.jsonl` de Roman."""
    p = tmp_path / "auditoria.jsonl"
    audit_mod.set_path(p)
    yield p
    audit_mod.set_path(None)


@pytest.fixture(autouse=True)
def pedidos_file(tmp_path: Path) -> Iterator[Path]:
    """El registro de pedidos (C0) de cada test va a un archivo propio."""
    p = tmp_path / "pedidos.jsonl"
    telemetry.set_path(p)
    yield p
    telemetry.set_path(None)
