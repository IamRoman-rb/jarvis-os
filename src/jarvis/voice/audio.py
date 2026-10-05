"""Lo que no necesita micrófono ni modelos: el nivel de un pedazo de audio y cuándo terminó de
hablar Roman. Audio PCM de 16 bits, mono, en bytes."""

from __future__ import annotations

import array
import math
import time
from collections.abc import Callable

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

    #: Cuántos pedazos recuerda para estimar el ruido (~6 s de 80 ms). Se cuentan todos, también
    #: los de voz: así un ruido constante (música, una aspiradora) termina subiendo el umbral.
    NOISE_FRAMES = 75

    def __init__(self, silence_ms: int = 700, max_ms: int = 8_000, min_ms: int = 160) -> None:
        self.silence_ms = silence_ms
        self.max_ms = max_ms
        #: Menos voz que esto es un golpe o una tecla: no vale la pena transcribirlo.
        self.min_ms = min_ms
        self.loud_ms = 0
        self.noise = 40.0
        self.ambient: list[float] = []
        self.current: bytearray | None = None
        self.pre = bytearray()
        self.quiet_ms = 0
        self.total_ms = 0

    def threshold(self) -> float:
        # Micrófonos con poca ganancia dan ~50 de ruido (ventilador, sala) y ~150-600 hablando a
        # distancia normal: con 4 veces un ruido promedio (que cada golpe o tecla infla) el umbral
        # quedaba arriba de la voz y JARVIS no oía nada.
        return max(120.0, self.noise * 3)

    def _track_noise(self, energy: float) -> None:
        """El ruido es el percentil 25 de lo reciente: ni los golpes sueltos ni las palabras (que
        casi nunca ocupan las tres cuartas partes de 6 s) lo suben."""
        self.ambient.append(energy)
        del self.ambient[: -self.NOISE_FRAMES]
        self.noise = sorted(self.ambient)[len(self.ambient) // 4]

    def feed(self, pcm: bytes) -> bytes | None:
        """Agrega un pedazo; devuelve una frase completa cuando termina."""
        ms = len(pcm) // 2 * 1000 // RATE
        energy = rms(pcm)
        loud = energy >= self.threshold()
        self._track_noise(energy)
        if self.current is None:
            if not loud:
                self.pre = (self.pre + pcm)[-RATE // 2 * 2 :]  # medio segundo antes
                return None
            self.current = bytearray(self.pre)
            self.quiet_ms = 0
            self.total_ms = 0
            self.loud_ms = 0
        self.current += pcm
        self.total_ms += ms
        if loud:
            self.loud_ms += ms
        self.quiet_ms = 0 if loud else self.quiet_ms + ms
        if self.quiet_ms >= self.silence_ms or self.total_ms >= self.max_ms:
            phrase = bytes(self.current)
            self.current = None
            self.pre = bytearray()
            return phrase if self.loud_ms >= self.min_ms else None
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


def cinema_effect(pcm: bytes, rate: int) -> bytes:
    """La voz "cine": un asistente de película, propio (no imita a ningún actor). Al brillo
    metálico de `jarvis_effect` le suma calidez (un filtro que baja los agudos ásperos) y una
    sala chica (dos reflejos suaves, a 37 y 61 ms), y la deja en un volumen parejo. La
    velocidad más lenta (ver `engine.VOICES`) la hace más grave y calma."""
    a = array.array("h")
    a.frombytes(jarvis_effect(pcm, rate)[: len(pcm) // 2 * 2])
    n = len(a)
    warm = [0.0] * n
    prev = 0.0
    for i in range(n):  # pasa-bajos de un polo: más cálida
        prev += (a[i] - prev) * 0.55
        warm[i] = prev
    d1, d2 = rate * 37 // 1000, rate * 61 // 1000
    out = array.array("h", bytes(n * 2))
    for i in range(n):
        v = warm[i] * 0.8
        if i >= d1:
            v += warm[i - d1] * 0.18
        if i >= d2:
            v += warm[i - d2] * 0.1
        # Limitador suave: no satura en los picos.
        v = 32767 * math.tanh(v / 32767)
        out[i] = int(v)
    return out.tobytes()


#: Lo que Whisper "oye" en el ruido o el silencio (lo aprendió de subtítulos de videos).
HALLUCINATIONS = (
    "suscribete",
    "suscribanse",
    "gracias por ver",
    "gracias por mirar",
    "gracias por su atencion",
    "subtitulos realizados",
    "subtitulado por",
    "amara org",
    "no olvides suscribirte",
    "dale like",
)


def is_noise(text: str) -> bool:
    """Una transcripción que no es un pedido: vacía, solo signos, o una alucinación típica."""
    plain = " ".join(_plain(text).split())
    if sum(c.isalpha() for c in plain) < 2:
        return True
    if plain in ("musica", "aplausos", "risas", "silencio", "gracias"):
        return True
    return any(h in plain for h in HALLUCINATIONS)


def for_speech(text: str) -> str:
    """El texto como para decirlo: sin markdown, sin emojis ni símbolos, las direcciones como
    "el enlace" y los saltos de línea como pausas."""
    import re

    t = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    t = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", t)  # [texto](url) → texto
    t = re.sub(r"https?://\S+", "el enlace", t)
    t = re.sub(r"[*_`#>|~]+", "", t)
    t = re.sub(r"^\s*[-•]\s+", "", t, flags=re.MULTILINE)
    # Solo letras, números y la puntuación que Piper sabe leer (fuera emojis y símbolos).
    t = "".join(c if c.isalnum() or c.isspace() or c in ".,;:!?¡¿()'\"-/%$°" else " " for c in t)
    t = re.sub(r"\s*\n\s*", ". ", t.strip())
    t = re.sub(r"\.(\s*\.)+", ".", t)
    return re.sub(r"[ \t]+", " ", t).strip()


def send_paced(
    pcm: bytes,
    rate: int,
    send: Callable[[int, bytes], None],
    stop: Callable[[], bool],
    piece_ms: int = 200,
    lead_ms: int = 300,
    sleep: Callable[[float], None] = time.sleep,
    clock: Callable[[], float] = time.monotonic,
) -> bool:
    """Manda `pcm` (mono, 16 bits, a `rate` Hz) de a pedazos de `piece_ms`, al ritmo en que suena.

    JARVIS-OS reproduce la voz (K12): no hace falta mandarle todo de golpe, y mandarlo al ritmo
    del audio (con `lead_ms` de ventaja, para que nunca le falte) tiene dos ventajas: callar
    (`stop`) corta enseguida, y quien llama sabe cuándo terminó de sonar (el micrófono no se
    escucha a sí mismo mientras tanto). Devuelve `False` si se cortó.
    """
    step = max(2, rate * piece_ms // 1000 * 2)
    start = clock()
    sent_ms = 0.0
    for i in range(0, len(pcm), step):
        ahead = sent_ms - (clock() - start) * 1000
        if ahead > lead_ms:
            sleep((ahead - lead_ms) / 1000)
        if stop():
            return False
        piece = pcm[i : i + step]
        send(rate, piece)
        sent_ms += len(piece) // 2 * 1000 / rate
    # Esperar a que termine de sonar.
    while (remaining := sent_ms - (clock() - start) * 1000) > 0:
        if stop():
            return False
        sleep(min(0.05, remaining / 1000))
    return True
