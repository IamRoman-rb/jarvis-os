"""Log de auditoría en SQLite: cada tool que Claude pide y qué se decidió."""

import json
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path
from typing import Any

import aiosqlite

_SCHEMA = """
CREATE TABLE IF NOT EXISTS actions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    origin     TEXT NOT NULL,
    tool       TEXT NOT NULL,
    level      INTEGER,
    input_json TEXT NOT NULL,
    event      TEXT NOT NULL
)
"""


class AuditEvent(StrEnum):
    REQUESTED = "requested"
    """Claude pidió la tool (lo registra el hook PreToolUse, antes de cualquier decisión)."""
    AUTO_ALLOWED = "auto_allowed"
    APPROVED = "approved"
    DENIED = "denied"
    """El usuario rechazó la confirmación."""
    BLOCKED = "blocked"
    """La política la rechazó sin preguntar (tool desconocida, nivel 3 desde Android)."""


@dataclass(frozen=True)
class AuditEntry:
    ts: str
    origin: str
    tool: str
    level: int | None
    tool_input: dict[str, Any]
    event: AuditEvent


class AuditLog:
    """Escribe una fila por evento. Abre y cierra la conexión en cada escritura: el volumen es
    bajo y así no queda estado compartido entre corrutinas."""

    def __init__(self, db_path: Path) -> None:
        self.db_path = db_path

    async def record(
        self,
        *,
        origin: str,
        tool: str,
        level: int | None,
        tool_input: dict[str, Any],
        event: AuditEvent,
    ) -> None:
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        async with aiosqlite.connect(self.db_path) as db:
            await db.execute(_SCHEMA)
            await db.execute(
                "INSERT INTO actions (ts, origin, tool, level, input_json, event)"
                " VALUES (?, ?, ?, ?, ?, ?)",
                (
                    datetime.now(UTC).isoformat(),
                    origin,
                    tool,
                    level,
                    json.dumps(tool_input, ensure_ascii=False, default=str),
                    event.value,
                ),
            )
            await db.commit()

    async def entries(self) -> list[AuditEntry]:
        """Todas las filas, en orden. Pensado para tests y para un futuro `jarvis audit`."""
        if not self.db_path.exists():
            return []
        async with aiosqlite.connect(self.db_path) as db:
            await db.execute(_SCHEMA)
            cursor = await db.execute(
                "SELECT ts, origin, tool, level, input_json, event FROM actions ORDER BY id"
            )
            rows = await cursor.fetchall()
        return [
            AuditEntry(
                ts=r[0],
                origin=r[1],
                tool=r[2],
                level=r[3],
                tool_input=json.loads(r[4]),
                event=AuditEvent(r[5]),
            )
            for r in rows
        ]
