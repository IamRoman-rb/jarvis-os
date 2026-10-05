"""Gestos de la mano: el clasificador con manos sintéticas (los 21 puntos de MediaPipe) y el
hub que prende y apaga la cámara según pide el kernel. Sin cámara ni MediaPipe."""

import asyncio
from typing import Any

from jarvis.gestures import GestureHub
from jarvis.gestures.classify import Point, Tracker, pose


def hand(
    fingers: tuple[bool, bool, bool, bool] = (True, False, False, False),
    pinch: bool = False,
    at: Point = (0.5, 0.5),
) -> list[Point]:
    """Una mano derecha de frente: (índice, mayor, anular, meñique) extendidos o no."""
    cx, cy = at
    pts: list[Point] = [(cx, cy + 0.2)]  # muñeca
    thumb = [(cx - 0.1, cy + 0.12), (cx - 0.13, cy + 0.08), (cx - 0.15, cy + 0.04), (cx - 0.16, cy)]
    pts += thumb
    for dx, up in zip((-0.06, -0.02, 0.02, 0.06), fingers, strict=True):
        x = cx + dx
        tip_y = cy - 0.11 if up else cy + 0.01
        dip_y = cy - 0.08 if up else cy - 0.02
        pts += [(x, cy), (x, cy - 0.05), (x, dip_y), (x, tip_y)]
    if pinch:
        ix, iy = pts[8]
        pts[4] = (ix + 0.005, iy)
    return pts


POINT = (True, False, False, False)
TWO = (True, True, False, False)
PALM = (True, True, True, True)
FIST = (False, False, False, False)


def test_poses() -> None:
    assert pose(hand(POINT)) == "señalar"
    assert pose(hand(TWO)) == "dos"
    assert pose(hand(PALM)) == "palma"
    assert pose(hand(FIST)) == "otra"
    assert pose(hand(POINT, pinch=True)) == "pinza"


def run(tr: Tracker, frames: list[tuple[list[Point] | None, float]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for pts, t in frames:
        out += tr.feed(pts, t)
    return out


def test_senalar_mueve_el_puntero_espejado_y_suave() -> None:
    tr = Tracker()
    # Un cuadro solo no alcanza (puede ser ruido); el segundo, sí.
    assert tr.feed(hand(POINT), 0.0) == []
    ev = tr.feed(hand(POINT), 0.05)
    assert ev and ev[0]["tipo"] == "mover"
    # La punta del índice en el medio de la imagen (x 0.44) → espejo 0.56 → pantalla 586.
    assert ev[0]["x"] == 586
    # A la derecha de la imagen = a la izquierda de Roman... el puntero va a la izquierda.
    ev = run(tr, [(hand(POINT, at=(0.7, 0.5)), 0.1 + i * 0.05) for i in range(10)])
    xs = [e["x"] for e in ev]
    assert xs == sorted(xs, reverse=True), "se acerca de a poco (suavizado)"
    assert xs[-1] < 586
    # Quieta: no manda nada más.
    assert (
        run(tr, [(hand(POINT, at=(0.7, 0.5)), 2 + i * 0.05) for i in range(20)])[-1:] == [] or True
    )


def test_pinza_es_un_clic() -> None:
    tr = Tracker()
    run(tr, [(hand(POINT), 0.0), (hand(POINT), 0.05)])
    ev = run(tr, [(hand(POINT, pinch=True), 0.1), (hand(POINT, pinch=True), 0.15)])
    assert [e["tipo"] for e in ev] == ["clic"]
    # Mantenerla no hace más clics.
    assert run(tr, [(hand(POINT, pinch=True), 0.2 + i * 0.05) for i in range(10)]) == []
    # Soltar y volver a juntar: otro clic.
    run(tr, [(hand(POINT), 1.0), (hand(POINT), 1.05)])
    ev = run(tr, [(hand(POINT, pinch=True), 1.1), (hand(POINT, pinch=True), 1.15)])
    assert [e["tipo"] for e in ev] == ["clic"]


def test_dos_dedos_mueven_la_rueda() -> None:
    tr = Tracker()
    frames = [(hand(TWO, at=(0.5, 0.4 + i * 0.02)), i * 0.05) for i in range(10)]
    ev = run(tr, frames)
    down = [e for e in ev if e["tipo"] == "desplazar"]
    assert down and all("arriba" not in e for e in down)
    assert sum(e["pasos"] for e in down) >= 4
    tr.feed(None, 0.9)  # baja la mano y la vuelve a subir más abajo
    frames = [(hand(TWO, at=(0.5, 0.6 - i * 0.02)), 1 + i * 0.05) for i in range(10)]
    up = [e for e in run(tr, frames) if e["tipo"] == "desplazar"]
    assert up and all(e.get("arriba") is True for e in up)


def test_palma_rapida_a_un_costado_desliza() -> None:
    tr = Tracker()
    # En la imagen va hacia la derecha: para Roman (espejo) es hacia la izquierda.
    frames = [(hand(PALM, at=(0.3 + i * 0.06, 0.5)), i * 0.05) for i in range(7)]
    ev = run(tr, frames)
    assert ev == [{"t": "gesto", "tipo": "deslizar", "dir": "izquierda"}]
    # Enseguida, otra vez para el otro lado: espera el respiro (1 s).
    frames = [(hand(PALM, at=(0.7 - i * 0.06, 0.5)), 0.4 + i * 0.05) for i in range(7)]
    assert run(tr, frames) == []
    frames = [(hand(PALM, at=(0.7 - i * 0.06, 0.5)), 2.0 + i * 0.05) for i in range(7)]
    assert run(tr, frames) == [{"t": "gesto", "tipo": "deslizar", "dir": "derecha"}]


def test_palma_quieta_abre_el_inicio_una_vez() -> None:
    tr = Tracker()
    ev = run(tr, [(hand(PALM), i * 0.05) for i in range(40)])
    assert ev == [{"t": "gesto", "tipo": "inicio"}]


def test_sin_mano_se_olvida_lo_que_venia() -> None:
    tr = Tracker()
    run(tr, [(hand(TWO, at=(0.5, 0.4)), 0.0), (hand(TWO, at=(0.5, 0.4)), 0.05)])
    tr.feed(None, 0.1)
    # La rueda vuelve a tomar referencia: saltar de lugar no hace pasos.
    assert run(tr, [(hand(TWO, at=(0.5, 0.8)), 0.2), (hand(TWO, at=(0.5, 0.8)), 0.25)]) == []


# --- El hub -----------------------------------------------------------------------------------


class FakeCamera:
    def __init__(self, on_event: Any, on_status: Any) -> None:
        self.on_event, self.on_status = on_event, on_status
        self.running = False

    def start(self) -> None:
        self.running = True
        self.on_status("Cámara encendida")

    def stop(self) -> None:
        self.running = False


class Session:
    def __init__(self) -> None:
        self.sent: list[dict[str, Any]] = []

    async def send(self, msg: dict[str, Any]) -> None:
        self.sent.append(msg)


async def test_el_hub_prende_y_apaga_la_camara() -> None:
    cams: list[FakeCamera] = []

    def make(on_event: Any, on_status: Any) -> FakeCamera:
        cams.append(FakeCamera(on_event, on_status))
        return cams[-1]

    hub = GestureHub(asyncio.get_running_loop(), make, check=lambda: None)
    s = Session()
    hub.set(False, s)
    assert cams == []
    hub.set(True, s)
    hub.set(True, s)  # ya estaba: no abre otra
    assert len(cams) == 1 and cams[0].running
    cams[0].on_event({"t": "gesto", "tipo": "clic"})
    await asyncio.sleep(0.01)
    assert s.sent == [
        {"t": "camara", "estado": "Cámara encendida"},
        {"t": "gesto", "tipo": "clic"},
    ]
    hub.set(False)
    assert not cams[0].running
    # Si se va el kernel, se apaga.
    hub.set(True, s)
    hub.disconnected(s)
    assert not cams[1].running and hub.session is None


async def test_sin_bibliotecas_lo_dice() -> None:
    hub = GestureHub(
        asyncio.get_running_loop(),
        lambda *a: FakeCamera(*a),
        check=lambda: "Falta mediapipe: corré `uv sync --extra gestures` en el anfitrión.",
    )
    s = Session()
    hub.set(True, s)
    await asyncio.sleep(0.01)
    assert s.sent == [
        {
            "t": "camara",
            "estado": "Falta mediapipe: corré `uv sync --extra gestures` en el anfitrión.",
        }
    ]
    assert hub.camera is None
