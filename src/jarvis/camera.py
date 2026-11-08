"""La webcam del anfitrión para JARVIS-OS (ADR 0022): la vista de la app Cámara y "bloquear al
irme".

- **Vista**: mientras la app Cámara está abierta (el kernel manda `camara_vista{activo}`), se leen
  cuadros, se achican a 320x240, se comprimen en JPEG y se mandan (`camara_cuadro`), unos 10 por
  segundo. Nada se guarda: las fotos y los videos los guarda JARVIS-OS en su disco, si Roman
  los pide.
- **Presencia**: con "bloquear al irme" activado (`presencia_activa{activo}`), se busca una cara
  cada medio segundo con el clasificador Haar que trae OpenCV (sin bajar modelos). Si no hay nadie
  durante `AWAY_SECONDS`, se avisa `presencia{hay: false}`; cuando vuelve, `presencia{hay:
  true}`. Al kernel solo le llega eso, nunca la imagen.
- La cámara se abre solo si hace falta para alguna de las dos cosas, y se cierra apenas no.

Desbloquear con la cara **no** se hace: con una webcam común (sin infrarrojo) una foto alcanza
para engañarla (lo advierten Howdy y Visage). Por eso solo se usa para *bloquear*.
"""

from __future__ import annotations

import asyncio
import base64
import logging
import platform
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any, Protocol

log = logging.getLogger("jarvis.camara")

#: Cuadros por segundo de la vista.
VIEW_FPS = 10
#: Cada cuánto se busca una cara (en segundos).
FACE_EVERY = 0.5
#: Cuánto tiempo sin nadie para avisar que se fue.
AWAY_SECONDS = 60.0
WIDTH, HEIGHT = 320, 240


class Sender(Protocol):
    async def send(self, msg: dict[str, Any]) -> None: ...


class Source(Protocol):
    """De dónde salen los cuadros (la webcam de verdad, o una de mentira en los tests)."""

    def open(self) -> str | None:
        """`None` si abrió; si no, el motivo."""
        ...

    def read(self) -> tuple[bytes | None, bool | None]:
        """(JPEG del cuadro, ¿hay una cara?) — la cara solo si se pidió (`want_face`)."""
        ...

    def close(self) -> None: ...

    want_face: bool


class Presence:
    """Cuándo avisar que alguien se fue o volvió (lógica pura, con tests)."""

    def __init__(self, away_seconds: float = AWAY_SECONDS) -> None:
        self.away_seconds = away_seconds
        self.last_seen: float | None = None
        self.here = True

    def feed(self, face: bool, now: float) -> bool | None:
        """Un resultado nuevo. Devuelve `False` cuando se fue, `True` cuando volvió, o `None`."""
        if self.last_seen is None:
            self.last_seen = now
        if face:
            self.last_seen = now
            if not self.here:
                self.here = True
                return True
            return None
        if self.here and now - self.last_seen >= self.away_seconds:
            self.here = False
            return False
        return None


class OpenCvSource:
    """La webcam con OpenCV (lo mismo que usan los gestos: `uv sync --extra gestures`)."""

    def __init__(self, index: int = 0) -> None:
        self.index = index
        self.want_face = False
        self._cap: Any = None
        self._faces: Any = None

    def open(self) -> str | None:
        try:
            import cv2
        except ImportError:
            return "Falta OpenCV: corré `uv sync --extra gestures` en el anfitrión."
        api = cv2.CAP_DSHOW if platform.system() == "Windows" else cv2.CAP_ANY
        self._cap = cv2.VideoCapture(self.index, api)
        if not self._cap.isOpened():
            return "No encontré una cámara en esta PC (o la están usando los gestos)."
        self._faces = cv2.CascadeClassifier(
            cv2.data.haarcascades + "haarcascade_frontalface_default.xml"
        )
        return None

    def read(self) -> tuple[bytes | None, bool | None]:
        import cv2

        ok, frame = self._cap.read()
        if not ok:
            return None, None
        small = cv2.resize(frame, (WIDTH, HEIGHT))
        face = None
        if self.want_face and self._faces is not None:
            gray = cv2.cvtColor(small, cv2.COLOR_BGR2GRAY)
            found = self._faces.detectMultiScale(gray, scaleFactor=1.2, minNeighbors=5)
            face = len(found) > 0
        ok, jpeg = cv2.imencode(".jpg", small, [cv2.IMWRITE_JPEG_QUALITY, 70])
        return (bytes(jpeg) if ok else None), face

    def close(self) -> None:
        if self._cap is not None:
            self._cap.release()
            self._cap = None


class StillSource:
    """Una "cámara" que siempre muestra la misma foto, con alguien adelante: la del cerebro
    simulado (`jarvis serve --simulado`), para las pruebas de punta a punta y las capturas."""

    #: Un JPEG del repo (el de las pruebas del decodificador del kernel).
    DEFAULT = Path(__file__).resolve().parents[2] / "kernel/paquetes/pruebas/aurora-progresivo.jpg"

    def __init__(self, path: Path = DEFAULT) -> None:
        self.path = path
        self.want_face = False
        self._jpeg = b""

    def open(self) -> str | None:
        try:
            self._jpeg = self.path.read_bytes()
        except OSError:
            return f"No encontré la imagen de prueba ({self.path.name})."
        return None

    def read(self) -> tuple[bytes | None, bool | None]:
        return self._jpeg, (True if self.want_face else None)

    def close(self) -> None:
        pass


class CameraHub:
    """Prende la cámara cuando la vista o la presencia la necesitan; manda lo que corresponde."""

    def __init__(
        self,
        loop: asyncio.AbstractEventLoop,
        make_source: Callable[[], Source] = OpenCvSource,
        presence: Presence | None = None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self.loop = loop
        self.make_source = make_source
        self.presence = presence or Presence()
        self.clock = clock
        self.session: Sender | None = None
        self.view = False
        self.detect = False
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def _post(self, msg: dict[str, Any]) -> None:
        session = self.session
        if session is not None:
            asyncio.run_coroutine_threadsafe(session.send(msg), self.loop)

    def set_view(self, on: bool, session: Sender | None = None) -> None:
        if session is not None:
            self.session = session
        self.view = on
        self._update()

    def set_presence(self, on: bool, session: Sender | None = None) -> None:
        if session is not None:
            self.session = session
        self.detect = on
        self.presence = Presence(self.presence.away_seconds)
        self._update()

    def disconnected(self, session: Sender) -> None:
        if self.session is session:
            self.session = None
            self.view = self.detect = False
            self._update()

    def _update(self) -> None:
        need = self.view or self.detect
        running = self._thread is not None and self._thread.is_alive()
        if need and not running:
            self._stop.clear()
            self._thread = threading.Thread(target=self._run, name="jarvis-camara", daemon=True)
            self._thread.start()
        elif not need and running:
            self._stop.set()

    def _run(self) -> None:
        source = self.make_source()
        problem = source.open()
        if problem:
            self._post({"t": "camara", "estado": problem})
            return
        log.info("cámara encendida (vista=%s, presencia=%s)", self.view, self.detect)
        last_face = 0.0
        try:
            while not self._stop.is_set():
                started = self.clock()
                source.want_face = self.detect and started - last_face >= FACE_EVERY
                jpeg, face = source.read()
                if jpeg is None:
                    self._post({"t": "camara", "estado": "La cámara dejó de mandar imagen."})
                    return
                if self.view:
                    self._post({"t": "camara_cuadro", "jpeg": base64.b64encode(jpeg).decode()})
                if face is not None:
                    last_face = started
                    change = self.presence.feed(face, started)
                    if change is not None:
                        log.info("presencia: %s", "volvió" if change else "se fue")
                        self._post({"t": "presencia", "hay": change})
                wait = 1 / VIEW_FPS if self.view else FACE_EVERY
                self._stop.wait(max(0.0, wait - (self.clock() - started)))
        finally:
            source.close()
            log.info("cámara apagada")
