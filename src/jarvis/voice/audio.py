"""Lo que no necesita micrófono ni modelos: el nivel de un pedazo de audio y cuándo terminó de
hablar Roman. Audio PCM de 16 bits, mono, en bytes."""

from __future__ import annotations

import array
import math

RATE = 16_000
#: Pedazos de 80 ms: lo que espera openWakeWord.
FRAME = 1280


def rms(pcm: bytes) -> float:
    """Energía (0..32768) de un pedazo de audio."""
    if len(pcm) < 2:
        return 0.0
    a = array.array("h")
    a.frombytes(pcm[: len(pcm) // 2 * 2])
    return math.sqrt(sum(x * x for x in a) / len(a))


def level(pcm: bytes) -> int:
    """Nivel 0..100 para la esfera (escala logarítmica, como el oído)."""
    r = rms(pcm)
    if r < 50:
        return 0
    # 50 → 0, ~10 000 → 100
    return max(0, min(100, int(math.log10(r / 50) / math.log10(200) * 100)))


class SpeechRecorder:
    """Junta el pedido después de la palabra de activación, hasta un silencio.

    Termina con `silence_ms` de silencio después de haber oído voz, o a los `max_ms`. Si en
    `start_ms` no se oyó nada, termina vacío (falsa alarma).
    """

    def __init__(
        self,
        threshold: float = 500.0,
        silence_ms: int = 900,
        max_ms: int = 10_000,
        start_ms: int = 3_000,
    ) -> None:
        self.threshold = threshold
        self.silence_ms = silence_ms
        self.max_ms = max_ms
        self.start_ms = start_ms
        self.audio = bytearray()
        self.heard = False
        self.quiet_ms = 0
        self.total_ms = 0

    def feed(self, pcm: bytes) -> bool:
        """Agrega un pedazo. `True` = terminó."""
        ms = len(pcm) // 2 * 1000 // RATE
        self.audio += pcm
        self.total_ms += ms
        if rms(pcm) >= self.threshold:
            self.heard = True
            self.quiet_ms = 0
        else:
            self.quiet_ms += ms
        if self.total_ms >= self.max_ms:
            return True
        if not self.heard:
            return self.total_ms >= self.start_ms
        return self.quiet_ms >= self.silence_ms

    def speech(self) -> bytes:
        return bytes(self.audio) if self.heard else b""
