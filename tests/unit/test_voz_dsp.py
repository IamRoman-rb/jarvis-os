"""El procesado de la voz (ADR 0025) con señales armadas en el test: tonos puros, ruido y una
"s" sintética. Sin modelos ni parlantes."""

import numpy as np
from numpy.typing import NDArray

from jarvis.voice import dsp

RATE = 24_000


def tone(freq: float, seconds: float = 1.0, amp: float = 0.5) -> NDArray[np.float32]:
    t = np.arange(int(RATE * seconds)) / RATE
    return (amp * np.sin(2 * np.pi * freq * t)).astype(np.float32)


def peak_freq(x: NDArray[np.float32]) -> float:
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    return float(np.fft.rfftfreq(len(x), 1 / RATE)[int(np.argmax(spec))])


def energy(x: NDArray[np.float32]) -> float:
    return float(np.mean(x.astype(np.float64) ** 2))


def test_pcm_ida_y_vuelta() -> None:
    x = tone(440)
    back = dsp.to_float(dsp.to_pcm(x))
    assert np.max(np.abs(back - x)) < 1e-3


def test_wsola_cambia_el_largo_sin_cambiar_el_tono() -> None:
    x = tone(220, 1.0)
    for factor in (0.8, 1.25):
        y = dsp.time_stretch(x, RATE, factor)
        assert len(y) == round(len(x) * factor)
        assert abs(peak_freq(y) - 220) < 4, factor


def test_bajar_el_tono_no_cambia_el_largo() -> None:
    x = tone(220, 1.0)
    y = dsp.pitch_shift(x, RATE, -1.5)
    assert len(y) == len(x)
    expected = 220 * 2 ** (-1.5 / 12)  # ~201,7 Hz
    assert abs(peak_freq(y) - expected) < 4, peak_freq(y)
    # No se come la señal (salvo los bordes).
    assert energy(y) > 0.5 * energy(x)


def test_el_ecualizador_saca_lo_grave_y_respeta_la_voz() -> None:
    rumble, voice = tone(30), tone(1000)
    assert energy(dsp.equalize(rumble, RATE)) < 0.1 * energy(rumble)
    ratio = energy(dsp.equalize(voice, RATE)) / energy(voice)
    assert 0.7 < ratio < 1.4, ratio
    # +2,5 dB a 200 Hz: más cuerpo.
    assert energy(dsp.equalize(tone(200), RATE)) > 1.3 * energy(tone(200))


def test_el_de_esser_baja_las_eses_y_no_la_voz() -> None:
    rng = np.random.default_rng(1)
    hiss = np.fft.irfft(
        np.fft.rfft(rng.standard_normal(RATE)) * (np.fft.rfftfreq(RATE, 1 / RATE) > 5000)
    ).astype(np.float32)
    hiss *= 0.3 / np.max(np.abs(hiss))
    out = dsp.deess(hiss, RATE)
    assert energy(out) < 0.4 * energy(hiss)
    voice = tone(300)
    assert abs(energy(dsp.deess(voice, RATE)) / energy(voice) - 1) < 0.05


def test_el_compresor_empareja_el_volumen() -> None:
    x = np.concatenate([tone(300, 0.5, 0.05), tone(300, 0.5, 0.8)])
    y = dsp.compress(x, RATE)
    half = len(x) // 2
    before = energy(x[half + 2000 :]) / energy(x[2000:half])
    after = energy(y[half + 2000 :]) / energy(y[2000:half])
    assert after < before / 4
    # Lo bajo no se toca.
    assert abs(energy(y[2000:half]) / energy(x[2000:half]) - 1) < 0.05


def test_la_sala_deja_cola_y_el_brillo_no_cambia_el_largo() -> None:
    click = np.zeros(RATE // 2, np.float32)
    click[100] = 1.0
    wet = dsp.reverb(click, RATE, mix=0.5, tail_ms=400)
    assert len(wet) == len(click) + RATE * 400 // 1000
    assert energy(wet[RATE // 10 :]) > 0  # sigue sonando después del golpe
    x = tone(500)
    assert len(dsp.shimmer(x, RATE)) == len(x)


def test_la_cadena_entera() -> None:
    rng = np.random.default_rng(2)
    x = (tone(180, 1.5) + 0.05 * rng.standard_normal(int(RATE * 1.5))).astype(np.float32)
    y = dsp.jarvis_chain(x, RATE)
    assert y.dtype == np.float32
    assert len(x) <= len(y) <= len(x) + RATE // 2
    assert np.all(np.isfinite(y))
    assert abs(np.max(np.abs(y)) - 10 ** (-1 / 20)) < 1e-3
    pcm = dsp.jarvis_pcm(dsp.to_pcm(x), RATE)
    assert len(pcm) == len(y) * 2
    # Vacío y muy corto no rompen.
    assert len(dsp.jarvis_chain(np.zeros(0, np.float32), RATE)) >= 0
    assert np.all(np.isfinite(dsp.jarvis_chain(np.zeros(10, np.float32), RATE)))
