"""Gestos de la mano frente a la cámara del anfitrión para manejar JARVIS-OS.

- `classify`: de los puntos de la mano a gestos (sin cámara, con tests).
- `camera`: la cámara (OpenCV + MediaPipe), en un hilo.
- `GestureHub`: la prende y la apaga según pida el kernel (`gestos{activo}`) y le manda
  `camara{estado}` y `gesto{...}`.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Callable
from typing import Any, Protocol

log = logging.getLogger("jarvis.gestos")


class Sender(Protocol):
    async def send(self, msg: dict[str, Any]) -> None: ...


class CameraLike(Protocol):
    def start(self) -> None: ...

    def stop(self) -> None: ...


#: (on_event, on_status) → la cámara. Los tests pasan una de mentira.
MakeCamera = Callable[[Callable[[dict[str, Any]], None], Callable[[str], None]], CameraLike]


def _real_camera(
    on_event: Callable[[dict[str, Any]], None], on_status: Callable[[str], None]
) -> CameraLike:
    from jarvis.gestures.camera import Camera

    return Camera(on_event, on_status)


def _check() -> str | None:
    from jarvis.gestures.camera import check

    return check()


class GestureHub:
    def __init__(
        self,
        loop: asyncio.AbstractEventLoop,
        make_camera: MakeCamera = _real_camera,
        check: Callable[[], str | None] = _check,
    ) -> None:
        self.loop = loop
        self.make_camera = make_camera
        self.check = check
        self.session: Sender | None = None
        self.camera: CameraLike | None = None

    def _post(self, msg: dict[str, Any]) -> None:
        """Desde el hilo de la cámara (o desde el bucle): al kernel conectado."""
        session = self.session
        if session is not None:
            asyncio.run_coroutine_threadsafe(session.send(msg), self.loop)

    def set(self, on: bool, session: Sender | None = None) -> None:
        """Prender o apagar la cámara (lo pide el kernel)."""
        if session is not None:
            self.session = session
        if not on:
            if self.camera is not None:
                self.camera.stop()
                self.camera = None
            return
        if self.camera is not None:
            return
        problem = self.check()
        if problem:
            self._post({"t": "camara", "estado": problem})
            return
        self.camera = self.make_camera(
            self._post, lambda text: self._post({"t": "camara", "estado": text})
        )
        self.camera.start()

    def disconnected(self, session: Sender) -> None:
        """Se fue el kernel: sin nadie a quien mandarle gestos, la cámara se apaga."""
        if self.session is session:
            self.session = None
            self.set(False)
