"""Genera los paquetes de audio y video de ejemplo (K12): `musica/` y `videos/`.

    python generar_multimedia.py        (necesita Pillow para los cuadros del video)

Todo es sintético (no hay grabaciones con derechos de nadie):

- musica/tema.wav: el "Tema de JARVIS", 20 s de acordes y arpegios, en IMA ADPCM (4 bits por
  muestra). El codificador de acá es el mismo algoritmo que decodifica jarvis-audio (adpcm.rs).
- musica/escala.wav: una escala en PCM de 16 bits, para comparar.
- videos/demo.avi: 8 s de 320x240 a 15 cuadros por segundo, MJPEG (cada cuadro, un JPEG) con
  audio PCM. Cada segundo suena un "bip" y la pantalla destella: sirve para ver que imagen y
  sonido van juntos.
"""

from __future__ import annotations

import io
import math
import struct
from pathlib import Path

HERE = Path(__file__).parent
RATE = 22050

STEP = [
    7,
    8,
    9,
    10,
    11,
    12,
    13,
    14,
    16,
    17,
    19,
    21,
    23,
    25,
    28,
    31,
    34,
    37,
    41,
    45,
    50,
    55,
    60,
    66,
    73,
    80,
    88,
    97,
    107,
    118,
    130,
    143,
    157,
    173,
    190,
    209,
    230,
    253,
    279,
    307,
    337,
    371,
    408,
    449,
    494,
    544,
    598,
    658,
    724,
    796,
    876,
    963,
    1060,
    1166,
    1282,
    1411,
    1552,
    1707,
    1878,
    2066,
    2272,
    2499,
    2749,
    3024,
    3327,
    3660,
    4026,
    4428,
    4871,
    5358,
    5894,
    6484,
    7132,
    7845,
    8630,
    9493,
    10442,
    11487,
    12635,
    13899,
    15289,
    16818,
    18500,
    20350,
    22385,
    24623,
    27086,
    29794,
    32767,
]
INDEX = [-1, -1, -1, -1, 2, 4, 6, 8]


def note(freq: float, secs: float, amp: float = 0.35) -> list[float]:
    """Una nota con dos armónicos y una envolvente (sube rápido, decae, se apaga)."""
    n = int(RATE * secs)
    out = []
    for i in range(n):
        t = i / RATE
        fade = min(1.0, (n - i) / (RATE * 0.03))
        env = min(1.0, t / 0.01) * (0.6 + 0.4 * math.exp(-t * 6)) * fade
        w = math.sin(2 * math.pi * freq * t)
        w += 0.3 * math.sin(4 * math.pi * freq * t) + 0.15 * math.sin(6 * math.pi * freq * t)
        out.append(amp * env * w / 1.45)
    return out


def mix(*tracks: list[float]) -> list[float]:
    n = max(len(t) for t in tracks)
    return [sum(t[i] for t in tracks if i < len(t)) for i in range(n)]


def to16(samples: list[float]) -> list[int]:
    return [max(-32768, min(32767, int(s * 32767))) for s in samples]


def freq(midi: int) -> float:
    return 440.0 * 2 ** ((midi - 69) / 12)


def theme() -> list[int]:
    """Acordes de fondo con un arpegio encima: Am, F, C y G, dos vueltas."""
    chords = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]] * 2
    out: list[float] = []
    for chord in chords:
        pad = mix(*[note(freq(m), 2.5, 0.12) for m in chord])
        arp: list[float] = []
        for k in range(8):
            m = chord[k % 3] + 12 * (1 + (k // 3) % 2)
            arp += note(freq(m), 0.3125, 0.18)
        out += mix(pad, arp)
    return to16(out)


def scale() -> list[int]:
    out: list[float] = []
    for m in [60, 62, 64, 65, 67, 69, 71, 72]:
        out += note(freq(m), 0.4)
    return to16(out)


def adpcm(samples: list[int], block_align: int = 512) -> bytes:
    """IMA ADPCM de un canal, en bloques (como WAV_FORMAT_IMA_ADPCM de Microsoft)."""
    per = (block_align - 4) * 2 + 1
    out = bytearray()
    index = 0
    for start in range(0, len(samples), per):
        block = samples[start : start + per] + [0] * max(0, start + per - len(samples))
        pred = block[0]
        out += struct.pack("<hBB", pred, index, 0)
        nibbles = []
        for s in block[1:]:
            step = STEP[index]
            diff = s - pred
            code = 0
            if diff < 0:
                code = 8
                diff = -diff
            if diff >= step:
                code |= 4
                diff -= step
            if diff >= step >> 1:
                code |= 2
                diff -= step >> 1
            if diff >= step >> 2:
                code |= 1
            # Lo mismo que hace el decodificador.
            d = step >> 3
            if code & 4:
                d += step
            if code & 2:
                d += step >> 1
            if code & 1:
                d += step >> 2
            pred = max(-32768, min(32767, pred - d if code & 8 else pred + d))
            index = max(0, min(88, index + INDEX[code & 7]))
            nibbles.append(code)
        for k in range(0, len(nibbles), 2):
            out.append(nibbles[k] | (nibbles[k + 1] << 4))
    return bytes(out)


def wav(fmt: bytes, data: bytes) -> bytes:
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt
    body += b"data" + struct.pack("<I", len(data)) + data + (b"\0" if len(data) % 2 else b"")
    return b"RIFF" + struct.pack("<I", len(body)) + body


def pcm_fmt(rate: int, channels: int = 1) -> bytes:
    return struct.pack("<HHIIHH", 1, channels, rate, rate * channels * 2, channels * 2, 16)


def adpcm_fmt(rate: int, block_align: int = 512) -> bytes:
    per = (block_align - 4) * 2 + 1
    byte_rate = rate * block_align // per
    return struct.pack("<HHIIHHHH", 0x11, 1, rate, byte_rate, block_align, 4, 2, per)


def chunk(fourcc: bytes, data: bytes) -> bytes:
    return fourcc + struct.pack("<I", len(data)) + data + (b"\0" if len(data) % 2 else b"")


def lst(kind: bytes, *parts: bytes) -> bytes:
    return chunk(b"LIST", kind + b"".join(parts))


def video() -> bytes:
    from PIL import Image, ImageDraw

    w, h, fps, secs = 320, 240, 15, 8
    frames = []
    for i in range(fps * secs):
        t = i / fps
        flash = (t % 1.0) < 0.12
        img = Image.new("RGB", (w, h), (40, 70, 110) if flash else (6, 10, 20))
        d = ImageDraw.Draw(img)
        # Un arco que gira (como el HUD) y el reloj del video.
        a = (t * 90) % 360
        d.arc((80, 40, 240, 200), a, a + 120, fill=(0, 229, 255), width=6)
        d.arc((95, 55, 225, 185), -a * 1.5, -a * 1.5 + 60, fill=(120, 160, 255), width=3)
        d.text((118, 110), f"K12  {t:4.1f} s", fill=(230, 240, 255))
        d.text((104, 212), "JARVIS-OS · MJPEG + PCM", fill=(150, 170, 200))
        buf = io.BytesIO()
        img.save(buf, "JPEG", quality=70)
        frames.append(buf.getvalue())
    # El audio: un bip de 80 ms al principio de cada segundo, un pedazo por cuadro.
    per_frame = RATE // fps
    samples = []
    for k in range(per_frame * len(frames)):
        tt = k / RATE
        beep = (tt % 1.0) < 0.08
        samples.append(int(9000 * math.sin(2 * math.pi * 880 * tt)) if beep else 0)
    audio = [
        struct.pack(f"<{per_frame}h", *samples[f * per_frame : (f + 1) * per_frame])
        for f in range(len(frames))
    ]
    avih = struct.pack("<14I", 1_000_000 // fps, 0, 0, 0x10, len(frames), 0, 2, 0, w, h, 0, 0, 0, 0)
    vstrh = b"vidsMJPG" + struct.pack("<10I", 0, 0, 0, 1, fps, 0, len(frames), 0, 0, 0) + bytes(8)
    vstrf = struct.pack("<IiiHH4sI4I", 40, w, h, 1, 24, b"MJPG", w * h * 3, 0, 0, 0, 0)
    astrh = b"auds" + bytes(16) + struct.pack("<II", 1, RATE) + bytes(24)
    movi = b"".join(chunk(b"00dc", f) + chunk(b"01wb", a) for f, a in zip(frames, audio))
    hdrl = lst(
        b"hdrl",
        chunk(b"avih", avih),
        lst(b"strl", chunk(b"strh", vstrh), chunk(b"strf", vstrf)),
        lst(b"strl", chunk(b"strh", astrh), chunk(b"strf", pcm_fmt(RATE))),
    )
    return chunk(b"RIFF", b"AVI " + hdrl + lst(b"movi", movi))


def main() -> None:
    (HERE / "musica").mkdir(exist_ok=True)
    (HERE / "videos").mkdir(exist_ok=True)
    (HERE / "musica" / "tema.wav").write_bytes(wav(adpcm_fmt(RATE), adpcm(theme())))
    s = scale()
    (HERE / "musica" / "escala.wav").write_bytes(wav(pcm_fmt(RATE), struct.pack(f"<{len(s)}h", *s)))
    (HERE / "videos" / "demo.avi").write_bytes(video())
    for p in ["musica/tema.wav", "musica/escala.wav", "videos/demo.avi"]:
        print(p, (HERE / p).stat().st_size, "bytes")


if __name__ == "__main__":
    main()
