"""Une la voz del anfitrión con la sesión del kernel conectada.

- Lo que se oye va al kernel como `oido{texto}`: la consola lo trata como si se hubiera escrito
  (una orden local o un pedido a Claude con `origen: voz`).
- Mientras escucha sin la palabra de activación (después de "JARVIS" solo, Win+J o durante una
  conversación: ver `voice/listener.py`), `escuchando{activo}`.
- La respuesta a un pedido de voz se dice en voz alta, y el nivel del audio (`voz{nivel}`, ~20
  por segundo) mueve la esfera.
- Si el kernel tiene parlantes (`hola{parlantes}`, K12), la voz la reproduce **JARVIS-OS**: se le
  manda el audio (`audio{tasa, pcm}` en base64, y `audio{fin}` al final) y el kernel mueve la
  esfera con lo que suena. `callar` le dice que corte.
"""

from __future__ import annotations

import asyncio
import base64
import logging
import threading
from collections.abc import Callable
from typing import Any, Protocol

from jarvis.voice.listener import Event

log = logging.getLogger("jarvis.voz")


class VoiceLike(Protocol):
    def listen_now(self) -> None: ...

    def speak(self, text: str, on_level: Callable[[int], None]) -> None: ...

    def speak_pcm(self, text: str, on_chunk: Callable[[int, bytes], None]) -> None: ...

    def hush(self) -> None: ...

    def run(self, on_event: Callable[[Event], None]) -> None: ...

    def stop(self) -> None: ...


class Sender(Protocol):
    async def send(self, msg: dict[str, Any]) -> None: ...


class VoiceHub:
    def __init__(self, voice: VoiceLike, loop: asyncio.AbstractEventLoop) -> None:
        self.voice = voice
        self.loop = loop
        self.session: Sender | None = None
        #: El kernel conectado reproduce el audio él mismo (tiene parlantes, K12).
        self.kernel_audio = False
        self._speak_lock = asyncio.Lock()

    def _post(self, msg: dict[str, Any]) -> None:
        """Desde el hilo del audio: mandar al kernel conectado."""
        session = self.session
        if session is not None:
            asyncio.run_coroutine_threadsafe(session.send(msg), self.loop)

    def start(self) -> None:
        def run() -> None:
            try:
                self.voice.run(self.on_event)
            except Exception:
                log.exception("la voz se detuvo")

        threading.Thread(target=run, name="jarvis-voz", daemon=True).start()

    def on_event(self, ev: Event) -> None:
        """Desde el hilo del audio: lo que decidió `Listener`."""
        if ev.kind == "orden":
            self._post({"t": "oido", "texto": ev.text})
        elif ev.kind == "activo":
            self._post({"t": "escuchando", "activo": ev.on})
        elif ev.kind == "si":
            # "JARVIS" solo: contesta, así Roman sabe que lo escuchó.
            asyncio.run_coroutine_threadsafe(self.speak("¿Sí?"), self.loop)
        elif ev.kind == "fin":
            asyncio.run_coroutine_threadsafe(self.speak("Cuando quieras."), self.loop)

    def listen_now(self) -> None:
        self.voice.listen_now()

    def hush(self) -> None:
        """Deja de hablar (llegó otro pedido, o Roman canceló)."""
        self.voice.hush()
        if self.kernel_audio:
            self._post({"t": "callar"})

    async def speak(self, text: str) -> None:
        if not text.strip():
            return
        async with self._speak_lock:
            if self.kernel_audio and self.session is not None:

                def on_chunk(rate: int, pcm: bytes) -> None:
                    self._post({"t": "audio", "tasa": rate, "pcm": base64.b64encode(pcm).decode()})

                await asyncio.to_thread(self.voice.speak_pcm, text, on_chunk)
                self._post({"t": "audio", "fin": True})
                return
            last = -1

            def on_level(n: int) -> None:
                nonlocal last
                if n != last:
                    last = n
                    self._post({"t": "voz", "nivel": n})

            await asyncio.to_thread(self.voice.speak, text, on_level)
