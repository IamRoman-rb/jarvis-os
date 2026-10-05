"""De los puntos de la mano a gestos (sin cámara: testeable).

MediaPipe Hands da 21 puntos por mano (x, y entre 0 y 1 sobre la imagen): 0 es la muñeca; el
pulgar es 1-4, el índice 5-8, el mayor 9-12, el anular 13-16 y el meñique 17-20 (el último de
cada dedo es la punta). Con eso:

- **Señalar** (solo el índice extendido): el puntero sigue la punta del índice.
- **Pinza** (pulgar e índice juntos): un clic.
- **Dos dedos** (índice y mayor extendidos) y mover la mano: la rueda.
- **Palma abierta** y moverla rápido a un costado: deslizar (otro escritorio u otra ventana).
- **Palma abierta quieta** un momento: el menú de inicio.

Los eventos son los mensajes `gesto` del protocolo (ver `protocol.py`).
"""

from __future__ import annotations

import math
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Literal

Point = tuple[float, float]
Pose = Literal["señalar", "pinza", "dos", "palma", "otra"]

WRIST, THUMB_TIP, INDEX_TIP, MIDDLE_MCP = 0, 4, 8, 9
#: (punta, articulación del medio) de índice, mayor, anular y meñique.
FINGERS = ((8, 6), (12, 10), (16, 14), (20, 18))

#: La zona de la imagen que cubre la pantalla entera (la mano no llega a los bordes).
REACH = (0.15, 0.85)
#: Suavizado del puntero (más alto = sigue más rápido, tiembla más).
SMOOTH = 0.35
PINCH_RATIO = 0.33
CLICK_COOLDOWN = 0.4
#: La rueda: un paso cada tanto que se mueve la mano (en proporción a la imagen).
SCROLL_STEP = 0.035
SWIPE_DISTANCE = 0.22
SWIPE_WINDOW = 0.35
SWIPE_COOLDOWN = 1.0
HOLD_STILL = 0.03
HOLD_TIME = 1.2
HOLD_COOLDOWN = 2.0
#: Cuadros seguidos con la misma pose para creerle (un cuadro suelto es ruido).
STABLE_FRAMES = 2


def _dist(a: Point, b: Point) -> float:
    return math.hypot(a[0] - b[0], a[1] - b[1])


def extended(points: list[Point]) -> list[bool]:
    """Índice, mayor, anular y meñique: ¿extendidos? (la punta más lejos de la muñeca que la
    articulación del medio; sirve con la mano girada)."""
    w = points[WRIST]
    return [_dist(points[tip], w) > _dist(points[pip], w) * 1.1 for tip, pip in FINGERS]


def pose(points: list[Point]) -> Pose:
    size = _dist(points[WRIST], points[MIDDLE_MCP]) or 1e-6
    if _dist(points[THUMB_TIP], points[INDEX_TIP]) < PINCH_RATIO * size:
        return "pinza"
    index, middle, ring, pinky = extended(points)
    if index and middle and ring and pinky:
        return "palma"
    if index and middle and not ring and not pinky:
        return "dos"
    if index and not middle and not ring and not pinky:
        return "señalar"
    return "otra"


def _screen(v: float) -> int:
    lo, hi = REACH
    return round(min(max((v - lo) / (hi - lo), 0.0), 1.0) * 1000)


@dataclass
class Tracker:
    """Sigue la mano cuadro a cuadro y decide los gestos. La imagen llega como la ve la
    cámara: se espeja (moverse a la derecha mueve el puntero a la derecha)."""

    pointer: Point | None = None
    _pose: Pose = "otra"
    _candidate: Pose = "otra"
    _count: int = 0
    _last_click: float = -1e9
    _scroll_ref: float | None = None
    _palm: deque[tuple[float, Point]] = field(default_factory=deque)
    _last_swipe: float = -1e9
    _last_hold: float = -1e9
    _sent: tuple[int, int] | None = None

    def _stable(self, p: Pose) -> Pose:
        if p == self._candidate:
            self._count += 1
        else:
            self._candidate, self._count = p, 1
        if self._count >= STABLE_FRAMES:
            return p
        return self._pose

    def feed(self, points: list[Point] | None, now: float) -> list[dict[str, Any]]:
        """Un cuadro (`None`: no se ve ninguna mano) → los gestos que dispara."""
        if points is None or len(points) < 21:
            self._pose, self._candidate, self._count = "otra", "otra", 0
            self._scroll_ref = None
            self._palm.clear()
            return []
        pts = [(1.0 - x, y) for x, y in points]  # espejo
        before = self._pose
        current = self._pose = self._stable(pose(pts))
        events: list[dict[str, Any]] = []
        if current != "dos":
            self._scroll_ref = None
        if current != "palma":
            self._palm.clear()

        if current == "señalar":
            tip = pts[INDEX_TIP]
            if self.pointer is None or before != "señalar":
                self.pointer = tip
            else:
                px, py = self.pointer
                self.pointer = (px + (tip[0] - px) * SMOOTH, py + (tip[1] - py) * SMOOTH)
            xy = (_screen(self.pointer[0]), _screen(self.pointer[1]))
            # Sin mandar cada temblor: solo si se movió algo.
            if (
                self._sent is None
                or max(abs(xy[0] - self._sent[0]), abs(xy[1] - self._sent[1])) >= 3
            ):
                self._sent = xy
                events.append({"t": "gesto", "tipo": "mover", "x": xy[0], "y": xy[1]})
        elif current == "pinza":
            if before != "pinza" and now - self._last_click >= CLICK_COOLDOWN:
                self._last_click = now
                events.append({"t": "gesto", "tipo": "clic"})
        elif current == "dos":
            y = (pts[INDEX_TIP][1] + pts[12][1]) / 2
            if self._scroll_ref is None:
                self._scroll_ref = y
            steps = int((y - self._scroll_ref) / SCROLL_STEP)
            if steps:
                self._scroll_ref += steps * SCROLL_STEP
                ev: dict[str, Any] = {"t": "gesto", "tipo": "desplazar", "pasos": abs(steps)}
                if steps < 0:  # la mano subió
                    ev["arriba"] = True
                events.append(ev)
        elif current == "palma":
            center = pts[MIDDLE_MCP]
            self._palm.append((now, center))
            while self._palm and now - self._palm[0][0] > max(SWIPE_WINDOW, HOLD_TIME):
                self._palm.popleft()
            recent = [p for t, p in self._palm if now - t <= SWIPE_WINDOW]
            dx = recent[-1][0] - recent[0][0] if len(recent) > 1 else 0.0
            if abs(dx) >= SWIPE_DISTANCE and now - self._last_swipe >= SWIPE_COOLDOWN:
                self._last_swipe = now
                self._palm.clear()
                direction = "derecha" if dx > 0 else "izquierda"
                events.append({"t": "gesto", "tipo": "deslizar", "dir": direction})
            elif (
                self._palm
                and now - self._palm[0][0] >= HOLD_TIME * 0.95
                and max(_dist(p, center) for _, p in self._palm) <= HOLD_STILL
                and now - self._last_hold >= HOLD_COOLDOWN
            ):
                self._last_hold = now
                self._palm.clear()
                events.append({"t": "gesto", "tipo": "inicio"})
        return events
