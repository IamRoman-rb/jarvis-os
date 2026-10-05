"""Qué hacer con cada frase que se oye: cuándo es una orden para JARVIS (sin audio, testeable).

- Fuera de una conversación, solo cuentan las frases que empiezan con "JARVIS". "JARVIS" solo
  deja **armado** (contesta "¿Sí?" y la frase siguiente es la orden, si llega en 8 s).
- Después de una orden empieza una **conversación**: lo que Roman diga es para JARVIS, sin
  repetir "JARVIS", hasta que pase **1 minuto sin hablarle**. El minuto se cuenta desde lo
  último que dijo Roman o desde que JARVIS terminó de hablar (lo que sea más tarde): una
  respuesta larga no se come el tiempo para contestarle.
- "Eso es todo", "nada más", "chau"... terminan la conversación antes.

Los eventos (`Event`) los traduce `service/voicehub.py` al protocolo del kernel.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from jarvis.voice.audio import _plain, split_wake

#: Cuánto espera la orden después de un "JARVIS" solo.
ARMED_MS = 8_000
#: Cuánto dura una conversación sin que Roman le hable.
CONVERSATION_MS = 60_000

#: Frases que terminan la conversación (sin tildes ni signos, como las deja `_plain`).
END_PHRASES = (
    "eso es todo",
    "nada mas",
    "chau",
    "hasta luego",
    "terminamos",
    "ya esta gracias",
    "listo gracias",
    "gracias eso es todo",
)

Kind = Literal["orden", "activo", "si", "fin"]


@dataclass(frozen=True)
class Event:
    #: "orden": un pedido (`text`); "activo": empieza o termina de escuchar sin la palabra de
    #: activación (`on`); "si": contestar "¿Sí?"; "fin": Roman cerró la conversación.
    kind: Kind
    text: str = ""
    on: bool = False


def is_goodbye(text: str) -> bool:
    plain = " ".join(_plain(text).split())
    return plain in END_PHRASES or any(plain.startswith(p + " ") for p in END_PHRASES[:4])


class Listener:
    def __init__(self) -> None:
        #: Hasta cuándo la frase siguiente es la orden (después de "JARVIS" solo o Win+J).
        self.armed_until = 0.0
        #: Hasta cuándo dura la conversación (0 = no hay).
        self.conversation_until = 0.0
        self._active = False

    def _state(self, now: float) -> list[Event]:
        """Avisa si cambió "escuchando sin la palabra de activación"."""
        active = now < self.armed_until or now < self.conversation_until
        if active == self._active:
            return []
        self._active = active
        return [Event("activo", on=active)]

    def in_conversation(self, now: float) -> bool:
        return now < self.conversation_until

    def listen_now(self, now: float) -> list[Event]:
        """Win+J: escuchar la frase siguiente."""
        self.armed_until = now + ARMED_MS / 1000
        return self._state(now)

    def speaking(self, now: float) -> None:
        """JARVIS está hablando: el tiempo para contestarle empieza cuando termine."""
        if self.armed_until:
            self.armed_until = max(self.armed_until, now + ARMED_MS / 1000)
        if self.conversation_until:
            self.conversation_until = max(self.conversation_until, now + CONVERSATION_MS / 1000)

    def tick(self, now: float) -> list[Event]:
        """Venció la espera o la conversación."""
        if self.armed_until and now >= self.armed_until:
            self.armed_until = 0.0
        if self.conversation_until and now >= self.conversation_until:
            self.conversation_until = 0.0
        return self._state(now)

    def _order(self, text: str, now: float) -> list[Event]:
        self.armed_until = 0.0
        self.conversation_until = now + CONVERSATION_MS / 1000
        # Primero "escuchando" y después la orden: la respuesta es lo último que se ve.
        return [*self._state(now), Event("orden", text)]

    def phrase(self, text: str, now: float) -> list[Event]:
        """Una frase transcripta (ya sin ruido)."""
        events = self.tick(now)
        order = split_wake(text)
        if order == "":
            # "JARVIS" solo: "¿Sí?" y la frase siguiente es la orden.
            self.armed_until = now + ARMED_MS / 1000
            return [*events, Event("si"), *self._state(now)]
        if now < self.armed_until or self.in_conversation(now):
            if self.in_conversation(now) and is_goodbye(order or text):
                self.armed_until = self.conversation_until = 0.0
                return [*events, Event("fin"), *self._state(now)]
            return [*events, *self._order(order or text, now)]
        if order is None:
            return events  # no le hablaban a JARVIS
        return [*events, *self._order(order, now)]
