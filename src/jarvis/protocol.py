"""Protocolo entre el kernel de JARVIS-OS y el cerebro (ADR 0008).

Un mensaje JSON por renglón, sobre una conexión TCP. Cada mensaje tiene `t` (el tipo):

- kernel → cerebro: `hola{token, idioma, equipo, parlantes, principal, consejo}` (parlantes: la
  frecuencia de su salida de audio, 0 si no tiene; principal: claude, gemini, chatgpt o
  deepseek), `pedido{id, texto, origen}`, `cancelar{id}`, `resultado{llamada, ok, datos}`,
  `confirmacion{llamada, ok}`, `agentes`, `agentes_modo{principal, consejo}`,
  `vincular{agente, metodo, clave?}` (metodo: google o clave), `desvincular{agente}`,
  `saludo{id, momento, nombre}` (al arrancar; momento: manana, tarde o noche; se contesta
  con `texto` y `fin`, como un pedido), `gestos{activo}` (prender o apagar la cámara; también
  viaja en `hola`).
- cerebro → kernel: `listo`, `texto{id, delta}`, `fin{id}`, `error{id, msg}`,
  `accion{llamada, tool, args}`, `confirmar{llamada, nivel, descripcion}`, `oido{texto}`,
  `voz{nivel}`, `agente{id, nombre, vinculado, metodo, detalle, estado}` (detalle: la
  cuenta o los últimos 4 caracteres de la clave, nunca la clave), `audio{tasa, pcm}` /
  `audio{fin}` (la voz para los parlantes del kernel, PCM mono de 16 bits en base64),
  `callar`, `proyecto{...}`, `camara{estado}`, `gesto{tipo: mover{x, y} (0..1000) | clic |
  desplazar{pasos, arriba?} | deslizar{dir} | inicio}`.

El kernel solo entiende JSON simple: objetos, textos, números enteros y booleanos.
"""

from __future__ import annotations

import json
from typing import Any

# Un renglón más largo no es un mensaje válido (un archivo leído entra de sobra).
MAX_LINE = 1024 * 1024

Message = dict[str, Any]


class ProtocolError(ValueError):
    """Un renglón que no es un mensaje válido."""


def encode(msg: Message) -> bytes:
    """El mensaje como un renglón (UTF-8, sin saltos adentro)."""
    return json.dumps(msg, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"


def decode(line: bytes) -> Message:
    """Un renglón recibido → mensaje. Rechaza lo que no sea un objeto con `t`."""
    if len(line) > MAX_LINE:
        raise ProtocolError("renglón demasiado largo")
    try:
        msg = json.loads(line.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise ProtocolError(f"no es JSON: {e}") from e
    if not isinstance(msg, dict) or not isinstance(msg.get("t"), str):
        raise ProtocolError("falta el tipo `t`")
    return msg
