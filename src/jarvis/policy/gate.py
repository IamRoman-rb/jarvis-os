"""Decisión de permisos: el corazón de la política, sin dependencias del SDK.

``agent/brain.py`` conecta esto con el callback ``can_use_tool`` del SDK. Orden de evaluación
del SDK: hooks → reglas deny → reglas ask → modo → reglas allow (``allowed_tools``) →
``can_use_tool``. JARVIS deja ``allowed_tools`` vacío, así que toda tool llega acá.
"""

from dataclasses import dataclass
from typing import Any

from jarvis.policy.audit import AuditEvent, AuditLog
from jarvis.policy.confirm import Confirmer
from jarvis.policy.levels import Level, Origin, UnknownToolError, bare_name, level_of


@dataclass(frozen=True)
class Decision:
    allowed: bool
    reason: str


async def decide(
    tool_name: str,
    tool_input: dict[str, Any],
    *,
    origin: Origin,
    confirmer: Confirmer,
    audit: AuditLog,
) -> Decision:
    """Decide si una tool se ejecuta. Ante la duda, no."""
    try:
        level = level_of(tool_name)
    except UnknownToolError:
        await audit.record(
            origin=origin,
            tool=tool_name,
            level=None,
            tool_input=tool_input,
            event=AuditEvent.BLOCKED,
        )
        return Decision(False, f"La herramienta {tool_name} no está permitida.")

    name = bare_name(tool_name)

    async def _log(event: AuditEvent) -> None:
        await audit.record(
            origin=origin, tool=name, level=int(level), tool_input=tool_input, event=event
        )

    if level is Level.READ_ONLY:
        await _log(AuditEvent.AUTO_ALLOWED)
        return Decision(True, "Solo lectura.")

    if level is Level.CRITICAL and origin is Origin.ANDROID:
        await _log(AuditEvent.BLOCKED)
        return Decision(False, "Las acciones críticas no se pueden pedir desde el celular.")

    try:
        approved = await confirmer(name, tool_input, level, origin)
    except Exception:  # un confirmer roto nunca debe equivaler a un "sí"
        approved = False

    if approved:
        await _log(AuditEvent.APPROVED)
        return Decision(True, "Aprobado por el usuario.")
    await _log(AuditEvent.DENIED)
    return Decision(False, "El usuario rechazó la acción.")
