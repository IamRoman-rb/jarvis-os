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


def test_la_palabra_de_activacion() -> None:
    from jarvis.voice.audio import split_wake

    assert split_wake("Jarvis, abrí el monitor.") == "abrí el monitor"
    assert split_wake("CARIS, abrí el monitor.") == "abrí el monitor"
    assert split_wake("Oye Yarbis ¿qué hora es?") == "qué hora es"
    assert split_wake("JARVIS.") == ""
    assert split_wake("mañana llueve") is None
    assert split_wake("vamos a comer") is None
    assert split_wake("Carlos abrí la puerta") is None


def test_el_segmentador_corta_en_el_silencio() -> None:
    from jarvis.voice.audio import Segmenter

    s = Segmenter(silence_ms=400)
    for _ in range(20):
        assert s.feed(tone(100, 80)) is None  # ruido de fondo
    assert s.feed(tone(9000, 80)) is None
    out = None
    for _ in range(6):
        out = out or s.feed(tone(100, 80))
    assert out is not None and len(out) > 0


def test_el_efecto_de_ia_no_cambia_el_largo_ni_satura() -> None:
    from jarvis.voice.audio import jarvis_effect

    pcm = tone(30000, 100)
    out = jarvis_effect(pcm, RATE)
    assert len(out) == len(pcm)
    assert out != pcm
    assert max(abs(x) for x in array.array("h", out)) <= 32767
