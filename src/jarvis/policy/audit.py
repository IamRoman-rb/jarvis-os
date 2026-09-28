"""Registro de auditoría: cada tool que JARVIS intenta usar, y si se aprobó (JSONL)."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

from jarvis.config import Config

_path: Path | None = None


def set_path(path: Path | None) -> None:
    """Para los tests: dónde escribir (None = la carpeta de logs del usuario)."""
    global _path
    _path = path


def audit_path() -> Path:
    return _path or Config.log_dir() / "auditoria.jsonl"


def audit(event: str, tool: str, args: dict[str, Any], **extra: Any) -> None:
    p = audit_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    entry = {"ts": time.strftime("%Y-%m-%dT%H:%M:%S"), "evento": event, "tool": tool, "args": args}
    entry.update(extra)
    with p.open("a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")
