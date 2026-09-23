"""Pruebas contra la API real. No corren por defecto: `uv run pytest -m live`."""

import os

import pytest

from jarvis.agent.brain import Brain
from jarvis.core.config import JarvisConfig
from jarvis.policy.audit import AuditEvent, AuditLog
from tests.helpers import FakeConfirmer

pytestmark = [
    pytest.mark.live,
    pytest.mark.skipif(not os.environ.get("ANTHROPIC_API_KEY"), reason="falta ANTHROPIC_API_KEY"),
]


async def test_pregunta_de_solo_lectura(cfg: JarvisConfig, audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=False)
    brain = Brain(cfg, confirmer=confirmer, audit=audit)
    answer = await brain.ask("¿Cuánta memoria RAM tiene esta máquina? Usá tus herramientas.")
    assert answer
    assert confirmer.calls == []
    tools = {(e.tool, e.event) for e in await audit.entries()}
    assert ("system_info", AuditEvent.REQUESTED) in tools
