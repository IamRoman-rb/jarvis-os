"""La cámara del anfitrión (ADR 0022): cuándo avisar presencia y el hub que prende y apaga la
webcam según la app Cámara y "bloquear al irme". Sin cámara ni OpenCV."""

import asyncio
import base64
from typing import Any

from jarvis.camera import CameraHub, Presence, StillSource


def test_presencia_avisa_una_vez_al_irse_y_al_volver() -> None:
    p = Presence(away_seconds=10)
    assert p.feed(True, 0) is None
    assert p.feed(False, 5) is None  # un rato sin cara no alcanza
    assert p.feed(False, 9.9) is None
    assert p.feed(False, 10) is False  # se fue
    assert p.feed(False, 30) is None  # no lo repite
    assert p.feed(True, 31) is True  # volvió
    assert p.feed(True, 32) is None


def test_presencia_una_cara_suelta_reinicia_la_cuenta() -> None:
    p = Presence(away_seconds=10)
    p.feed(False, 0)
    p.feed(True, 8)
    assert p.feed(False, 15) is None
    assert p.feed(False, 18) is False


class FakeSource:
    def __init__(self, faces: list[bool], problem: str | None = None) -> None:
        self.faces = faces
        self.problem = problem
        self.want_face = False
        self.closed = False
        self.reads = 0

    def open(self) -> str | None:
        return self.problem

    def read(self) -> tuple[bytes | None, bool | None]:
        self.reads += 1
        face = None
        if self.want_face:
            face = self.faces.pop(0) if self.faces else False
        return b"\xff\xd8jpeg", face

    def close(self) -> None:
        self.closed = True


class Session:
    def __init__(self) -> None:
        self.sent: list[dict[str, Any]] = []

    async def send(self, msg: dict[str, Any]) -> None:
        self.sent.append(msg)


async def wait_for(cond: Any, seconds: float = 2.0) -> None:
    for _ in range(int(seconds / 0.01)):
        if cond():
            return
        await asyncio.sleep(0.01)
    raise AssertionError("no pasó a tiempo")


async def test_la_vista_manda_cuadros_y_se_apaga_al_cerrar() -> None:
    sources: list[FakeSource] = []

    def make() -> FakeSource:
        sources.append(FakeSource([]))
        return sources[-1]

    hub = CameraHub(asyncio.get_running_loop(), make)
    s = Session()
    hub.set_view(True, s)
    await wait_for(lambda: len(s.sent) >= 2)
    frame = s.sent[0]
    assert frame["t"] == "camara_cuadro"
    assert base64.b64decode(frame["jpeg"]) == b"\xff\xd8jpeg"
    # Sin "bloquear al irme" no se buscan caras: no viaja nada de presencia.
    assert all(m["t"] == "camara_cuadro" for m in s.sent)
    hub.set_view(False)
    await wait_for(lambda: sources[0].closed)
    assert len(sources) == 1


async def test_presencia_sin_vista_no_manda_imagenes() -> None:
    src = FakeSource([True, False, False, False, True])
    hub = CameraHub(asyncio.get_running_loop(), lambda: src)
    hub.presence.away_seconds = 0.0  # cualquier "no hay cara" ya cuenta como irse
    s = Session()
    hub.set_presence(True, s)  # el Presence nuevo hereda el plazo
    await wait_for(lambda: {"t": "presencia", "hay": True} in s.sent, seconds=5)
    assert s.sent[0] == {"t": "presencia", "hay": False}
    assert not any(m["t"] == "camara_cuadro" for m in s.sent)
    hub.disconnected(s)
    await wait_for(lambda: src.closed)
    assert hub.session is None and not hub.detect


async def test_sin_camara_lo_dice() -> None:
    hub = CameraHub(
        asyncio.get_running_loop(),
        lambda: FakeSource([], problem="No encontré una cámara en esta PC."),
    )
    s = Session()
    hub.set_view(True, s)
    await wait_for(lambda: bool(s.sent))
    assert s.sent == [{"t": "camara", "estado": "No encontré una cámara en esta PC."}]


async def test_el_cerebro_simulado_muestra_una_foto_fija() -> None:
    hub = CameraHub(asyncio.get_running_loop(), StillSource)
    s = Session()
    hub.set_view(True, s)
    await wait_for(lambda: bool(s.sent))
    hub.set_view(False)
    jpeg = base64.b64decode(s.sent[0]["jpeg"])
    assert jpeg.startswith(b"\xff\xd8") and len(jpeg) > 1000
