"""El procesado de la voz de JARVIS (ADR 0025): de una voz sintética común a la de un asistente
de película, **sin imitar a ningún actor**. Funciones puras sobre NumPy (float32 mono en
[-1, 1]), con tests; corren una vez por oración, no en tiempo real.

La cadena (`jarvis_chain`), y por qué cada paso:

1. **Tono 1,5 semitonos más grave, sin hacerla lenta** (`pitch_shift`): remuestrear baja el tono
   pero alarga la frase; WSOLA (`time_stretch`) la vuelve a su largo copiando pedazos de ~30 ms
   que encajan (por correlación) en vez de estirar, así no suena "en cámara lenta" como la voz
   vieja (Piper más lenta).
2. **Ecualizador** (`equalize`): pasa-altos a 80 Hz (ruido grave), +2,5 dB de cuerpo a 200 Hz,
   -3 dB de aspereza a 3,5 kHz y +2 dB de "aire" arriba de 9 kHz. Se aplica en frecuencia sobre
   la frase entera (fase cero: no corre los graves respecto de los agudos).
3. **De-esser** (`deess`): baja las eses cuando dominan (subirle el aire las hace silbar).
4. **Compresor** (`compress`): volumen parejo, como una voz de locutor.
5. **Brillo de IA** (`shimmer`): una copia retrasada 4-7 ms que oscila despacio, mezclada muy
   baja: un leve timbre "de máquina" sin tapar las palabras.
6. **Sala chica** (`reverb`): reverberación de Schroeder (4 peines + 2 pasa-todo), poca mezcla.
7. **Normalizar** (`normalize`): pico a -1 dBFS.

Referencias: Verhelst y Roelands (1993) para WSOLA; el "Audio EQ Cookbook" de R. Bristow-Johnson
para las curvas del ecualizador; Schroeder (1962) para la reverberación.
"""

from __future__ import annotations

import numpy as np
from numpy.typing import NDArray

Audio = NDArray[np.float32]

#: Cuánto se baja el tono (semitonos).
PITCH_SEMITONES = -1.5


def to_float(pcm: bytes) -> Audio:
    """PCM de 16 bits → float32 en [-1, 1]."""
    a = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype=np.int16)
    return (a.astype(np.float32) / 32768.0).astype(np.float32)


def to_pcm(x: Audio) -> bytes:
    """float32 → PCM de 16 bits (recortando)."""
    return (np.clip(x, -1.0, 1.0) * 32767.0).astype(np.int16).tobytes()


def resample(x: Audio, factor: float) -> Audio:
    """Cambia el largo por `factor` (2 = el doble de muestras) con interpolación lineal."""
    n = max(1, round(len(x) * factor))
    if len(x) < 2:
        return np.resize(x, n).astype(np.float32)
    pos = np.linspace(0, len(x) - 1, n)
    return np.interp(pos, np.arange(len(x)), x).astype(np.float32)


def time_stretch(x: Audio, rate: int, factor: float, frame_ms: float = 30.0) -> Audio:
    """WSOLA: el mismo tono, `factor` veces más largo (0,9 = 10 % más corto).

    Se arma la salida de a medio cuadro (`hop`). Cada cuadro nuevo se copia de la entrada cerca
    de donde "debería" estar (posición de salida / factor), corrido hasta ±`tol` muestras para
    que continúe lo más parecido posible a lo que ya se escribió (máxima correlación), y se
    suma con una ventana de Hann.
    """
    frame = max(16, int(rate * frame_ms / 1000)) // 2 * 2
    hop = frame // 2
    tol = hop // 2
    if len(x) < frame * 2 or abs(factor - 1.0) < 1e-6:
        return resample(x, factor) if len(x) < frame * 2 else x.copy()
    out_len = round(len(x) * factor)
    window = np.hanning(frame).astype(np.float32)
    padded = np.concatenate([np.zeros(tol, np.float32), x, np.zeros(frame + tol, np.float32)])
    out = np.zeros(out_len + frame, np.float32)
    norm = np.zeros(out_len + frame, np.float32)
    # Lo que "seguiría" naturalmente al último cuadro copiado (para buscar el que mejor encaja).
    natural = padded[tol : tol + frame]
    for start in range(0, out_len, hop):
        ideal = int(start / factor) + tol
        lo, hi = max(0, ideal - tol), min(len(padded) - frame, ideal + tol)
        if start == 0 or hi <= lo:
            best = min(max(ideal, 0), len(padded) - frame)
        else:
            region = padded[lo : hi + frame]
            corr = np.correlate(region, natural[:frame], mode="valid")
            best = lo + int(np.argmax(corr))
        piece = padded[best : best + frame]
        out[start : start + frame] += piece * window
        norm[start : start + frame] += window
        natural = padded[best + hop : best + hop + frame]
        if len(natural) < frame:
            natural = np.pad(natural, (0, frame - len(natural)))
    norm[norm < 1e-3] = 1.0
    return (out / norm)[:out_len].astype(np.float32)


def pitch_shift(x: Audio, rate: int, semitones: float = PITCH_SEMITONES) -> Audio:
    """Cambia el tono `semitones` (negativo = más grave) dejando el largo igual."""
    ratio = 2.0 ** (semitones / 12.0)  # 0,917 para -1,5
    # Remuestrear a 1/ratio veces el largo baja el tono y alarga; WSOLA lo devuelve al largo.
    lower = resample(x, 1.0 / ratio)
    back = time_stretch(lower, rate, ratio)
    return fit(back, len(x))


def fit(x: Audio, n: int) -> Audio:
    """Corta o rellena con silencio hasta `n` muestras."""
    if len(x) >= n:
        return x[:n].astype(np.float32)
    return np.pad(x, (0, n - len(x))).astype(np.float32)


# --- ecualizador -------------------------------------------------------------------------------


def _biquad_gain(
    freqs: NDArray[np.float64], rate: int, kind: str, f0: float, db: float, q: float
) -> NDArray[np.float64]:
    """|H(f)| de un filtro del Audio EQ Cookbook (sin aplicarlo: solo la curva)."""
    a_gain = 10 ** (db / 40)
    w0 = 2 * np.pi * f0 / rate
    alpha = np.sin(w0) / (2 * q)
    cos = np.cos(w0)
    if kind == "highpass":
        b = [(1 + cos) / 2, -(1 + cos), (1 + cos) / 2]
        a = [1 + alpha, -2 * cos, 1 - alpha]
    elif kind == "peak":
        b = [1 + alpha * a_gain, -2 * cos, 1 - alpha * a_gain]
        a = [1 + alpha / a_gain, -2 * cos, 1 - alpha / a_gain]
    elif kind == "highshelf":
        sq = 2 * np.sqrt(a_gain) * alpha
        b = [
            a_gain * ((a_gain + 1) + (a_gain - 1) * cos + sq),
            -2 * a_gain * ((a_gain - 1) + (a_gain + 1) * cos),
            a_gain * ((a_gain + 1) + (a_gain - 1) * cos - sq),
        ]
        a = [
            (a_gain + 1) - (a_gain - 1) * cos + sq,
            2 * ((a_gain - 1) - (a_gain + 1) * cos),
            (a_gain + 1) - (a_gain - 1) * cos - sq,
        ]
    else:
        raise ValueError(kind)
    z = np.exp(-1j * 2 * np.pi * freqs / rate)
    h = (b[0] + b[1] * z + b[2] * z**2) / (a[0] + a[1] * z + a[2] * z**2)
    gain: NDArray[np.float64] = np.abs(h)
    return gain


#: (tipo, frecuencia, dB, Q).
JARVIS_EQ: tuple[tuple[str, float, float, float], ...] = (
    ("highpass", 80.0, 0.0, 0.707),
    ("peak", 200.0, 2.5, 0.9),
    ("peak", 3500.0, -3.0, 1.2),
    ("highshelf", 9000.0, 2.0, 0.707),
)


def equalize(
    x: Audio, rate: int, bands: tuple[tuple[str, float, float, float], ...] = JARVIS_EQ
) -> Audio:
    """Aplica las curvas en frecuencia (fase cero) a la frase entera."""
    if len(x) == 0:
        return x
    n = 1 << max(1, (len(x) - 1).bit_length())
    spectrum = np.fft.rfft(x, n)
    freqs = np.fft.rfftfreq(n, 1 / rate)
    gain = np.ones_like(freqs)
    for kind, f0, db, q in bands:
        if f0 < rate / 2:
            gain *= _biquad_gain(freqs, rate, kind, f0, db, q)
    return np.fft.irfft(spectrum * gain, n)[: len(x)].astype(np.float32)


# --- dinámica ----------------------------------------------------------------------------------


def _stft(x: Audio, size: int, hop: int) -> NDArray[np.complex128]:
    pad = np.pad(x, (size, size + size))
    frames = np.lib.stride_tricks.sliding_window_view(pad, size)[::hop]
    return np.fft.rfft(frames * np.hanning(size), axis=1)


def _istft(spec: NDArray[np.complex128], size: int, hop: int, n: int) -> Audio:
    frames = np.fft.irfft(spec, size, axis=1)
    window = np.hanning(size)
    out = np.zeros(len(frames) * hop + size)
    norm = np.zeros_like(out)
    for i, f in enumerate(frames):
        out[i * hop : i * hop + size] += f * window
        norm[i * hop : i * hop + size] += window**2
    norm[norm < 1e-6] = 1.0
    return (out / norm)[size : size + n].astype(np.float32)


def deess(
    x: Audio, rate: int, low: float = 4500.0, threshold: float = 0.45, depth_db: float = 8.0
) -> Audio:
    """Donde la energía arriba de `low` pasa `threshold` del total (una "s" fuerte), baja esa
    banda hasta `depth_db` dB, en proporción a cuánto se pasó."""
    if len(x) < 64:
        return x
    size, hop = 512, 128
    spec = _stft(x, size, hop)
    freqs = np.fft.rfftfreq(size, 1 / rate)
    high = freqs >= low
    power = np.abs(spec) ** 2
    ratio = power[:, high].sum(axis=1) / (power.sum(axis=1) + 1e-12)
    over = np.clip((ratio - threshold) / (1 - threshold), 0, 1)
    cut = 10 ** (-depth_db * over / 20)
    spec[:, high] *= cut[:, None]
    return _istft(spec, size, hop, len(x))


def compress(
    x: Audio,
    rate: int,
    threshold_db: float = -20.0,
    ratio: float = 3.0,
    attack_ms: float = 5.0,
    release_ms: float = 80.0,
) -> Audio:
    """Compresor por bloques de 5 ms: arriba del umbral, cada `ratio` dB de entrada sale 1."""
    block = max(1, rate // 200)
    n = len(x)
    if n == 0:
        return x
    blocks = int(np.ceil(n / block))
    padded = np.pad(x, (0, blocks * block - n)).reshape(blocks, block)
    level = np.sqrt((padded**2).mean(axis=1) + 1e-12)
    # Seguidor de envolvente: sube rápido (ataque) y baja despacio (relevo).
    att = np.exp(-block / (rate * attack_ms / 1000))
    rel = np.exp(-block / (rate * release_ms / 1000))
    env = np.empty(blocks)
    e = 0.0
    for i, v in enumerate(level):
        coef = att if v > e else rel
        e = coef * e + (1 - coef) * v
        env[i] = e
    db = 20 * np.log10(env + 1e-12)
    gain_db = np.minimum(0.0, (threshold_db - db) * (1 - 1 / ratio))
    gain = 10 ** (gain_db / 20)
    # La ganancia, interpolada entre centros de bloque (sin escalones).
    centers = np.arange(blocks) * block + block / 2
    g = np.interp(np.arange(n), centers, gain)
    return (x * g).astype(np.float32)


def shimmer(x: Audio, rate: int, mix: float = 0.12) -> Audio:
    """Una copia retrasada entre 4 y 7 ms (una oscilación lenta, 0,4 Hz), mezclada baja."""
    n = len(x)
    t = np.arange(n) / rate
    delay = (5.5 + 1.5 * np.sin(2 * np.pi * 0.4 * t)) / 1000 * rate
    pos = np.arange(n) - delay
    copy = np.interp(pos, np.arange(n), x, left=0.0)
    return ((1 - mix / 2) * x + mix * copy).astype(np.float32)


# --- sala --------------------------------------------------------------------------------------


def _comb(x: NDArray[np.float64], delay: int, gain: float) -> NDArray[np.float64]:
    """y[n] = x[n] + g·y[n-D], de a bloques de D muestras (cada bloque depende del anterior)."""
    y = x.copy()
    for start in range(delay, len(x), delay):
        end = min(start + delay, len(x))
        y[start:end] += gain * y[start - delay : end - delay]
    return y


def _allpass(x: NDArray[np.float64], delay: int, gain: float) -> NDArray[np.float64]:
    """y[n] = -g·x[n] + x[n-D] + g·y[n-D] (deja pasar todas las frecuencias, las desordena)."""
    y = -gain * x
    y[delay:] += x[:-delay] if delay < len(x) else 0
    for start in range(delay, len(x), delay):
        end = min(start + delay, len(x))
        y[start:end] += gain * y[start - delay : end - delay]
    return y


def reverb(x: Audio, rate: int, mix: float = 0.1, tail_ms: int = 400) -> Audio:
    """Schroeder: cuatro peines en paralelo (los ecos) y dos pasa-todo en serie (los difunden).
    Retardos de una sala chica, primos entre sí para que los ecos no coincidan."""
    if len(x) == 0:
        return x
    tail = rate * tail_ms // 1000
    dry = np.pad(x.astype(np.float64), (0, tail))
    scale = rate / 44100
    combs = [(1557, 0.77), (1617, 0.75), (1491, 0.73), (1422, 0.71)]
    wet = np.zeros_like(dry)
    for d, g in combs:
        wet += _comb(dry, max(1, int(d * scale)), g) / len(combs)
    for d, g in ((225, 0.5), (556, 0.5)):
        wet = _allpass(wet, max(1, int(d * scale)), g)
    return ((1 - mix) * dry + mix * wet).astype(np.float32)


def normalize(x: Audio, peak_db: float = -1.0) -> Audio:
    peak = float(np.max(np.abs(x))) if len(x) else 0.0
    if peak < 1e-6:
        return x
    return (x * (10 ** (peak_db / 20) / peak)).astype(np.float32)


def jarvis_chain(x: Audio, rate: int) -> Audio:
    """La cadena completa (ver arriba). Devuelve un poco más largo: la cola de la sala."""
    y = pitch_shift(x, rate)
    y = equalize(y, rate)
    y = deess(y, rate)
    y = compress(y, rate)
    y = shimmer(y, rate)
    y = reverb(y, rate, tail_ms=250)
    return normalize(y)


def jarvis_pcm(pcm: bytes, rate: int) -> bytes:
    """Lo mismo, de PCM de 16 bits a PCM de 16 bits."""
    return to_pcm(jarvis_chain(to_float(pcm), rate))
