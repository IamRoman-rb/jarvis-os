"""Registro de auditoría (JSONL): cada tool que JARVIS intenta usar, si se aprobó y cómo terminó.

Eventos: `intento` (lo pidió el modelo), `aprobada`/`rechazada` (la decisión del Gate) y
`ejecutada` (con `ok`, `ms` y `largo` de la respuesta). Ver docs/permisos.md → Auditoría.

El registro sirve para saber qué pasó, no para guardar los datos de Roman: el contenido de los
archivos y de los recuerdos se anota solo por su largo, los secretos se tapan y los textos
largos se recortan. El archivo rota al pasar `MAX_BYTES` (queda un único `auditoria.1.jsonl`).
"""

from __future__ import annotations

import json
import re
import time
from pathlib import Path
from typing import Any

from jarvis.config import Config

_path: Path | None = None

#: Al pasar este tamaño, el archivo pasa a `auditoria.1.jsonl` (los tests lo achican).
MAX_BYTES = 5 * 1024 * 1024
#: Campos con datos de Roman: se anota cuánto miden, no qué dicen.
PRIVATE_FIELDS = frozenset({"contenido", "dato"})
#: Campos que no se recortan: Roman tiene que poder ver el comando entero que aprobó.
FULL_FIELDS = frozenset({"comando"})
MAX_TEXT = 500

_SECRETS = [
    # Claves de la API de Anthropic.
    (re.compile(r"sk-ant-[A-Za-z0-9_\-]+"), "***"),
    # "Authorization: Bearer xxx".
    (re.compile(r"(?i)\b(bearer\s+)\S+"), r"\1***"),
    # TOKEN=xxx, api_key: xxx, JARVIS_CEREBRO_TOKEN=xxx, password="xxx"...
    (
        re.compile(
            r"(?i)\b(\w*(?:token|api[_-]?key|password|passwd|secret|contraseña)\w*)"
            r"(\s*[=:]\s*)(\"[^\"]*\"|'[^']*'|\S+)"
        ),
        r"\1\2***",
    ),
]


def set_path(path: Path | None) -> None:
    """Para los tests: dónde escribir (None = la carpeta de logs del usuario)."""
    global _path
    _path = path


def audit_path() -> Path:
    return _path or Config.log_dir() / "auditoria.jsonl"


def mask_secrets(text: str) -> str:
    """El texto con los secretos reconocibles tapados con `***`."""
    for pattern, repl in _SECRETS:
        text = pattern.sub(repl, text)
    return text


def _clean(value: Any, field: str = "") -> Any:
    if isinstance(value, dict):
        return {k: _clean(v, str(k)) for k, v in value.items()}
    if isinstance(value, list | tuple):
        return [_clean(v, field) for v in value]
    if not isinstance(value, str):
        return value
    if field in PRIVATE_FIELDS:
        return f"<{len(value)} caracteres>"
    value = mask_secrets(value)
    if field not in FULL_FIELDS and len(value) > MAX_TEXT:
        return f"{value[:MAX_TEXT]}…(+{len(value) - MAX_TEXT})"
    return value


def redact(args: dict[str, Any]) -> dict[str, Any]:
    """Los argumentos como se anotan: sin datos privados, sin secretos y sin textos enormes."""
    cleaned: dict[str, Any] = _clean(args)
    return cleaned


def _rotate(p: Path) -> None:
    try:
        if p.stat().st_size >= MAX_BYTES:
            p.replace(p.with_suffix(".1.jsonl"))
    except FileNotFoundError:
        pass


def audit(event: str, tool: str, args: dict[str, Any], **extra: Any) -> None:
    p = audit_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    _rotate(p)
    entry: dict[str, Any] = {
        "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "evento": event,
        "tool": tool,
        "args": redact(args),
    }
    entry.update(_clean(extra))
    with p.open("a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")
