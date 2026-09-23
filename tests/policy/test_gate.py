"""La compuerta de permisos: una tool de nivel 3 nunca corre sin confirmación."""

from jarvis.policy.audit import AuditEvent, AuditLog
from jarvis.policy.gate import decide
from jarvis.policy.levels import Level, Origin
from tests.helpers import FakeConfirmer


async def test_nivel_1_no_pregunta(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=False)
    d = await decide(
        "mcp__system__find_files", {}, origin=Origin.LOCAL, confirmer=confirmer, audit=audit
    )
    assert d.allowed
    assert confirmer.calls == []


async def test_nivel_3_rechazado_si_el_usuario_dice_que_no(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=False)
    d = await decide(
        "mcp__system__trash_file",
        {"path": "/x"},
        origin=Origin.LOCAL,
        confirmer=confirmer,
        audit=audit,
    )
    assert not d.allowed
    assert confirmer.calls == [("trash_file", {"path": "/x"}, Level.CRITICAL, Origin.LOCAL)]
    [entry] = await audit.entries()
    assert entry.event is AuditEvent.DENIED


async def test_nivel_2_y_3_aprobados_si_el_usuario_confirma(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=True)
    for tool in ("open_app", "trash_file"):
        d = await decide(
            f"mcp__system__{tool}", {}, origin=Origin.LOCAL, confirmer=confirmer, audit=audit
        )
        assert d.allowed
    assert [e.event for e in await audit.entries()] == [AuditEvent.APPROVED] * 2


async def test_nivel_3_desde_android_se_bloquea_sin_preguntar(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=True)
    d = await decide(
        "mcp__system__trash_file", {}, origin=Origin.ANDROID, confirmer=confirmer, audit=audit
    )
    assert not d.allowed
    assert confirmer.calls == []
    [entry] = await audit.entries()
    assert entry.event is AuditEvent.BLOCKED


async def test_nivel_2_desde_android_pide_confirmacion(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=True)
    d = await decide(
        "mcp__system__open_app", {}, origin=Origin.ANDROID, confirmer=confirmer, audit=audit
    )
    assert d.allowed
    assert len(confirmer.calls) == 1


async def test_tool_desconocida_se_bloquea(audit: AuditLog) -> None:
    confirmer = FakeConfirmer(answer=True)
    for tool in ("Bash", "mcp__otro__trash_file"):
        d = await decide(tool, {}, origin=Origin.LOCAL, confirmer=confirmer, audit=audit)
        assert not d.allowed
    assert confirmer.calls == []


async def test_confirmer_que_falla_equivale_a_no(audit: AuditLog) -> None:
    async def roto(*_args: object) -> bool:
        raise RuntimeError("se cerró la ventana")

    d = await decide(
        "mcp__system__trash_file", {}, origin=Origin.LOCAL, confirmer=roto, audit=audit
    )
    assert not d.allowed
