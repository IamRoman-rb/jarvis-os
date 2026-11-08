"""El registro por pedido (C0): saber qué pasa cuando JARVIS "no entiende".

Una línea JSON por pedido en `pedidos.jsonl` (la carpeta de logs del usuario): de dónde vino
(texto o voz), cuánto tardó (la primera palabra y el total), qué agente contestó y si hubo que
cambiar de agente, qué tools se usaron, qué habría hecho el NLU (y si lo resolvió local), el
uso de tokens si el proveedor lo informa, y el error si lo hubo.

**El texto del pedido no se guarda**, salvo que Roman lo active (`diagnostico = true` en
`config.toml`): por defecto queda solo su largo y un resumen (SHA-256 recortado) que sirve
para ver repetidos sin leerlos. Igual que la auditoría, el archivo rota al pasar 5 MB.
"""

from __future__ import annotations

import hashlib
import json
import time
from pathlib import Path
from typing import Any

from jarvis.config import Config
from jarvis.policy.audit import mask_secrets

MAX_BYTES = 5 * 1024 * 1024
MAX_TEXT = 300
_path: Path | None = None


def set_path(path: Path | None) -> None:
    """Para los tests: dónde escribir (None = la carpeta de logs del usuario)."""
    global _path
    _path = path


def path() -> Path:
    return _path or Config.log_dir() / "pedidos.jsonl"


def fingerprint(text: str) -> str:
    return hashlib.sha256(" ".join(text.lower().split()).encode("utf-8")).hexdigest()[:12]


def record(text: str, keep_text: bool, **fields: Any) -> dict[str, Any]:
    """Anota un pedido. Devuelve lo anotado (para los tests)."""
    entry: dict[str, Any] = {
        "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "largo": len(text),
        "huella": fingerprint(text),
    }
    if keep_text:
        entry["texto"] = mask_secrets(text)[:MAX_TEXT]
    entry.update(fields)
    p = path()
    try:
        p.parent.mkdir(parents=True, exist_ok=True)
        if p.exists() and p.stat().st_size > MAX_BYTES:
            p.replace(p.with_name(p.stem + ".1.jsonl"))
        with p.open("a", encoding="utf-8") as f:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")
    except OSError:
        pass  # el registro nunca corta una respuesta
    return entry


def read(p: Path | None = None) -> list[dict[str, Any]]:
    src = p or path()
    if not src.exists():
        return []
    out: list[dict[str, Any]] = []
    for line in src.read_text(encoding="utf-8").splitlines():
        try:
            v = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(v, dict):
            out.append(v)
    return out


def summary(entries: list[dict[str, Any]]) -> str:
    """El informe de `jarvis nlu informe`: cuántos pedidos se resolvieron (o se habrían
    resuelto) sin el modelo de lenguaje, y cuántos tokens usó el modelo en los demás, **medidos**
    donde el proveedor los informa."""
    n = len(entries)
    if n == 0:
        return "Todavía no hay pedidos registrados (pedidos.jsonl está vacío)."
    local = sum(1 for e in entries if e.get("resuelto_local"))
    would = sum(
        1
        for e in entries
        if not e.get("resuelto_local")
        and isinstance(e.get("nlu"), dict)
        and e["nlu"].get("accion") in ("tool", "local")
    )
    agree = disagree = 0
    for e in entries:
        nlu = e.get("nlu")
        if not isinstance(nlu, dict) or nlu.get("accion") != "tool" or e.get("resuelto_local"):
            continue
        if nlu.get("tool") in (e.get("tools") or []):
            agree += 1
        else:
            disagree += 1
    agents: dict[str, int] = {}
    for e in entries:
        a = str(e.get("agente", "?"))
        agents[a] = agents.get(a, 0) + 1
    with_usage = [e for e in entries if isinstance(e.get("uso"), dict)]
    tin = sum(int(e["uso"].get("input_tokens", 0)) for e in with_usage)
    tout = sum(int(e["uso"].get("output_tokens", 0)) for e in with_usage)
    lines = [
        f"Pedidos: {n}",
        f"Resueltos local (modo primero): {local} ({local / n:.1%})",
        f"Se habrían resuelto local (modo sombra): {would} ({would / n:.1%})",
        f"El NLU coincidió con la tool que usó el modelo: {agree} de {agree + disagree}",
        "Contestó: " + ", ".join(f"{k} {v}" for k, v in sorted(agents.items())),
        f"Cambios de agente: {sum(int(e.get('cambios', 0)) for e in entries)}",
    ]
    if with_usage:
        lines.append(
            f"Tokens medidos ({len(with_usage)} pedidos con uso informado): "
            f"entrada {tin}, salida {tout}; promedio {tin // len(with_usage)} + "
            f"{tout // len(with_usage)} por pedido al modelo"
        )
    else:
        lines.append("Tokens: ningún proveedor informó uso todavía (no se estima nada).")
    return "\n".join(lines)
