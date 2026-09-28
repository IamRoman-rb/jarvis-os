"""Une la voz del anfitrión con la sesión del kernel conectada.

- Lo que se oye va al kernel como `oido{texto}`: la consola lo trata como si se hubiera escrito
  (una orden local o un pedido a Claude con `origen: voz`).
- Mientras escucha, `escuchando{activo}`.
- La respuesta a un pedido de voz se dice en voz alta, y el nivel del audio (`voz{nivel}`, ~20
  por segundo) mueve la esfera.
"""

from __future__ import annotations

import asyncio
import logging
import threading
from collections.abc import Callable
from typing import Any, Protocol

log = logging.getLogger("jarvis.voz")


class VoiceLike(Protocol):
    def listen_now(self) -> None: ...

    def speak(self, text: str, on_level: Callable[[int], None]) -> None: ...

    def run(
        self, on_heard: Callable[[str], None], on_listening: Callable[[bool], None]
    ) -> None: ...

    def stop(self) -> None: ...


class Sender(Protocol):
    async def send(self, msg: dict[str, Any]) -> None: ...


class VoiceHub:
    def __init__(self, voice: VoiceLike, loop: asyncio.AbstractEventLoop) -> None:
        self.voice = voice
        self.loop = loop
        self.session: Sender | None = None
        self._speak_lock = asyncio.Lock()

    def _post(self, msg: dict[str, Any]) -> None:
        """Desde el hilo del audio: mandar al kernel conectado."""
        session = self.session
        if session is not None:
            asyncio.run_coroutine_threadsafe(session.send(msg), self.loop)

    def start(self) -> None:
        def run() -> None:
            try:
                self.voice.run(
                    lambda text: self._post({"t": "oido", "texto": text}),
                    lambda on: self._post({"t": "escuchando", "activo": on}),
                )
            except Exception:
                log.exception("la voz se detuvo")

        threading.Thread(target=run, name="jarvis-voz", daemon=True).start()

    def listen_now(self) -> None:
        self.voice.listen_now()

    async def speak(self, text: str) -> None:
        if not text.strip():
            return
        async with self._speak_lock:
            last = -1

            def on_level(n: int) -> None:
                nonlocal last
                if n != last:
                    last = n
                    self._post({"t": "voz", "nivel": n})

            await asyncio.to_thread(self.voice.speak, text, on_level)
