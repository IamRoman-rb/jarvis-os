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


#: Cómo suele escribir Whisper "JARVIS" (con y sin tilde, y confusiones comunes).
WAKE_WORDS = ("jarvis", "yarvis", "jarbis", "yarbis", "charvis", "jervis", "harvis", "jarviz")


def _plain(text: str) -> str:
    import unicodedata

    t = unicodedata.normalize("NFD", text.lower())
    t = "".join(c for c in t if unicodedata.category(c) != "Mn")
    return "".join(c if c.isalnum() else " " for c in t)


def _distance(a: str, b: str) -> int:
    """Distancia de edición (Levenshtein)."""
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def is_wake(word: str) -> bool:
    """ "jarvis" y cómo lo escribe Whisper cuando lo entiende mal ("caris", "yarbis"…)."""
    return word in WAKE_WORDS or (4 <= len(word) <= 7 and _distance(word, "jarvis") <= 2)


def split_wake(text: str) -> str | None:
    """Si `text` empieza con "JARVIS" (en las primeras 2 palabras), lo que sigue (puede ser "");
    si no lo nombra, `None`. Conserva el texto original de la orden."""
    words = text.split()
    plain = _plain(text).split()
    for i, w in enumerate(plain[:2]):
        if is_wake(w):
            # La misma posición en el texto original (las palabras coinciden una a una salvo
            # la puntuación suelta, que se descarta).
            orig = [x for x in words if _plain(x).strip()]
            rest = " ".join(orig[i + 1 :])
            return rest.strip(" ,.;:!¡?¿")
    return None


class Segmenter:
    """Corta el audio continuo en frases: empieza con voz y termina con un silencio. El umbral
    se adapta al ruido del ambiente."""

    def __init__(self, silence_ms: int = 700, max_ms: int = 8_000) -> None:
        self.silence_ms = silence_ms
        self.max_ms = max_ms
        self.noise = 200.0
        self.current: bytearray | None = None
        self.pre = bytearray()
        self.quiet_ms = 0
        self.total_ms = 0

    def threshold(self) -> float:
        # Micrófonos con poca ganancia dan ~5 de ruido y ~150-600 hablando a distancia normal.
        return max(120.0, self.noise * 4)

    def feed(self, pcm: bytes) -> bytes | None:
        """Agrega un pedazo; devuelve una frase completa cuando termina."""
        ms = len(pcm) // 2 * 1000 // RATE
        energy = rms(pcm)
        loud = energy >= self.threshold()
        if self.current is None:
            if not loud:
                self.noise = self.noise * 0.95 + energy * 0.05
                self.pre = (self.pre + pcm)[-RATE // 2 * 2 :]  # medio segundo antes
                return None
            self.current = bytearray(self.pre)
            self.quiet_ms = 0
            self.total_ms = 0
        self.current += pcm
        self.total_ms += ms
        self.quiet_ms = 0 if loud else self.quiet_ms + ms
        if self.quiet_ms >= self.silence_ms or self.total_ms >= self.max_ms:
            phrase = bytes(self.current)
            self.current = None
            self.pre = bytearray()
            return phrase
        return None


def jarvis_effect(pcm: bytes, rate: int) -> bytes:
    """El toque de IA de la voz "jarvis": un eco muy corto (8 ms) que le da un brillo metálico
    sin tapar las palabras. (El tono más grave sale de reproducirla un poco más lenta.)"""
    a = array.array("h")
    a.frombytes(pcm[: len(pcm) // 2 * 2])
    d = rate * 8 // 1000
    out = array.array("h", a)
    for i in range(d, len(a)):
        out[i] = max(-32768, min(32767, int(a[i] * 0.75 + a[i - d] * 0.35)))
    return out.tobytes()
