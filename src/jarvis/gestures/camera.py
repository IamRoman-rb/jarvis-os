"""La cámara del anfitrión → gestos (OpenCV para leerla, MediaPipe Hands para la mano).

Corre en un hilo y solo mientras los gestos están activados (Configuración o Win+A de JARVIS-OS).
Las imágenes no salen de este proceso ni se guardan: al kernel solo le llegan los gestos.

Necesita `uv sync --extra gestures` (mediapipe y opencv-python).
"""

from __future__ import annotations

import logging
import platform
import threading
import time
from collections.abc import Callable
from typing import Any

from jarvis.gestures.classify import Tracker

log = logging.getLogger("jarvis.gestos")

#: Cuadros por segundo que se analizan (más no mejora y gasta CPU).
FPS = 20


def check() -> str | None:
    """`None` si se puede usar la cámara; si no, qué falta (para mostrarlo)."""
    try:
        import cv2  # noqa: F401
        import mediapipe  # noqa: F401
    except ImportError as e:
        return f"Falta {e.name}: corré `uv sync --extra gestures` en el anfitrión."
    return None


class Camera:
    """`on_event(gesto)` y `on_status(texto)` se llaman desde el hilo de la cámara."""

    def __init__(
        self,
        on_event: Callable[[dict[str, Any]], None],
        on_status: Callable[[str], None],
        index: int = 0,
    ) -> None:
        self.on_event = on_event
        self.on_status = on_status
        self.index = index
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        if self._thread is not None and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, name="jarvis-gestos", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()

    def _run(self) -> None:
        try:
            self._loop()
        except Exception as e:  # la cámara no puede tirar abajo al cerebro
            log.exception("los gestos se detuvieron")
            self.on_status(f"Los gestos se detuvieron: {e}")

    def _loop(self) -> None:
        import cv2
        import mediapipe as mp

        # En Windows, DirectShow abre la cámara bastante más rápido que el backend por defecto.
        api = cv2.CAP_DSHOW if platform.system() == "Windows" else cv2.CAP_ANY
        cap = cv2.VideoCapture(self.index, api)
        if not cap.isOpened():
            self.on_status("No encontré una cámara en esta PC.")
            return
        hands = mp.solutions.hands.Hands(
            max_num_hands=1,
            model_complexity=0,
            min_detection_confidence=0.6,
            min_tracking_confidence=0.5,
        )
        tracker = Tracker()
        self.on_status("Cámara encendida: mové la mano frente a ella.")
        log.info("gestos: cámara %d encendida", self.index)
        try:
            while not self._stop.is_set():
                started = time.monotonic()
                ok, frame = cap.read()
                if not ok:
                    self.on_status("La cámara dejó de mandar imagen.")
                    return
                result = hands.process(cv2.cvtColor(frame, cv2.COLOR_BGR2RGB))
                points = None
                if result.multi_hand_landmarks:
                    points = [(p.x, p.y) for p in result.multi_hand_landmarks[0].landmark]
                for ev in tracker.feed(points, time.monotonic()):
                    self.on_event(ev)
                time.sleep(max(0.0, 1 / FPS - (time.monotonic() - started)))
        finally:
            hands.close()
            cap.release()
            self.on_status("Cámara apagada.")
            log.info("gestos: cámara apagada")
