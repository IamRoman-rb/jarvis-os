"""La parte de la voz que no necesita micrófono: niveles y el fin de un pedido hablado."""

import array
import math

from jarvis.voice.audio import FRAME, RATE, SpeechRecorder, level, rms


def tone(amplitude: int, ms: int) -> bytes:
    n = RATE * ms // 1000
    return array.array(
        "h", (int(amplitude * math.sin(2 * math.pi * 440 * i / RATE)) for i in range(n))
    ).tobytes()


def test_niveles() -> None:
    assert rms(b"") == 0
    assert level(tone(0, 50)) == 0
    assert 0 < level(tone(1000, 50)) < level(tone(20000, 50)) <= 100


def test_termina_con_un_silencio_despues_de_hablar() -> None:
    r = SpeechRecorder(silence_ms=400)
    frame_ms = FRAME * 1000 // RATE
    assert not r.feed(tone(8000, frame_ms))
    for _ in range(5):  # 5 de 80 ms = 400 ms de silencio
        done = r.feed(tone(0, frame_ms))
    assert done and r.speech()


def test_si_no_habla_es_una_falsa_alarma() -> None:
    r = SpeechRecorder(start_ms=240)
    frame_ms = FRAME * 1000 // RATE
    results = [r.feed(tone(0, frame_ms)) for _ in range(3)]
    assert results[-1] and r.speech() == b""
