from pathlib import Path

import pytest

from jarvis.core.config import JarvisConfig
from jarvis.policy.audit import AuditLog
from jarvis.tools import _common
from jarvis.tools._common import ProcResult


@pytest.fixture
def cfg(tmp_path: Path) -> JarvisConfig:
    """Config con una allowlist temporal: ``Facultad`` y ``Proyectos`` dentro de tmp_path."""
    facultad = tmp_path / "Facultad"
    proyectos = tmp_path / "Proyectos"
    facultad.mkdir()
    proyectos.mkdir()
    return JarvisConfig(
        allowed_dirs=[facultad, proyectos],
        audit_db=tmp_path / "state" / "audit.sqlite3",
    )


@pytest.fixture
def audit(cfg: JarvisConfig) -> AuditLog:
    return AuditLog(cfg.audit_db)


class FakeRun:
    """Reemplaza la ejecución de procesos: registra los comandos y no ejecuta nada."""

    def __init__(self) -> None:
        self.calls: list[list[str]] = []
        self.result = ProcResult(0, "", "")
        self.raises: BaseException | None = None

    async def __call__(self, args: list[str], timeout_s: float = 15.0) -> ProcResult:
        self.calls.append(list(args))
        if self.raises is not None:
            raise self.raises
        return self.result


@pytest.fixture
def fake_run(monkeypatch: pytest.MonkeyPatch) -> FakeRun:
    fake = FakeRun()
    monkeypatch.setattr(_common, "run", fake)
    return fake
