"""La voz de JARVIS en el anfitrión (docs/investigacion.md §6.3), todo local:

- palabra de activación: "JARVIS". El micrófono está siempre abierto: cada frase (voz hasta un
  silencio) se transcribe, y `voice/listener.py` decide si es para JARVIS: las que empiezan con
  "JARVIS" y, durante una conversación (hasta 1 minuto sin hablarle), todas;
- voz → texto: faster-whisper (modelo `small`, español);
- texto → voz: Kokoro-82M (voz `em_alex`) con el procesado de `voice/dsp.py` (ADR 0025); Piper
  queda de respaldo (si faltan los archivos de Kokoro) y para las voces "cine" y "daniela".

Necesita `uv sync --extra voice` y los modelos (`jarvis voz instalar` los baja, avisando el
tamaño). El audio corre en un hilo aparte; los resultados vuelven al bucle de asyncio.
"""

from __future__ import annotations

import hashlib
import logging
import queue
import re
import threading
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from platformdirs import user_data_path

from jarvis.voice.audio import (
    FRAME,
    RATE,
    Segmenter,
    cinema_effect,
    for_speech,
    is_noise,
    jarvis_effect,
    level,
    send_paced,
)
from jarvis.voice.listener import Event, Listener

log = logging.getLogger("jarvis.voz")

#: Después de hablar, el micrófono todavía recibe el eco de la sala: se ignora un rato.
ECHO_MS = 400
WHISPER_MODEL = "small"
PIPER_BASE = "https://huggingface.co/rhasspy/piper-voices/resolve/main/"
KOKORO_BASE = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/"
#: Los archivos de Kokoro (modelo int8 y estilos de las voces) con su SHA-256. La release no
#: publica hashes: son los de la primera descarga (2026-11-08), así un archivo cambiado después
#: no se usa sin que se note.
KOKORO_FILES: dict[str, tuple[str, int]] = {
    "kokoro-v1.0.int8.onnx": (
        "6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb",
        92_361_271,
    ),
    "voices-v1.0.bin": (
        "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
        28_214_398,
    ),
}


@dataclass(frozen=True)
class VoiceSpec:
    #: "kokoro" o "piper".
    engine: str
    #: La voz de Kokoro (`em_alex`) o el modelo de Piper.
    model: str
    #: El efecto: "dsp" (la cadena de `voice/dsp.py`), "cine", "jarvis" o "" (ninguno).
    effect: str
    #: Kokoro: el ritmo al hablar. Piper: la velocidad de reproducción (más lenta = más grave).
    speed: float
    #: Piper: la carpeta del repositorio de voces. Kokoro: el idioma (para el fonetizador).
    folder: str = ""
    lang: str = "es"


#: Ninguna imita la voz de un actor (clonar la voz de una persona real sin su consentimiento no
#: se hace): son voces sintéticas procesadas para sonar a un asistente de película.
VOICES: dict[str, VoiceSpec] = {
    # Kokoro, castellano, masculina, más grave y con sala (la de fábrica).
    "jarvis": VoiceSpec("kokoro", "em_alex", "dsp", 0.95),
    # Lo mismo en inglés británico (bm_lewis es otra opción).
    "jarvis-en": VoiceSpec("kokoro", "bm_george", "dsp", 0.95, lang="en-gb"),
    # Las de Piper (las de antes de la 0025).
    "cine": VoiceSpec("piper", "es_ES-davefx-medium", "cine", 0.86, "es/es_ES/davefx/medium"),
    "piper": VoiceSpec("piper", "es_ES-davefx-medium", "jarvis", 0.92, "es/es_ES/davefx/medium"),
    "daniela": VoiceSpec("piper", "es_AR-daniela-high", "", 1.0, "es/es_AR/daniela/high"),
}
DEFAULT_VOICE = "jarvis"
#: Si faltan los archivos de Kokoro, esta (Piper).
FALLBACK_VOICE = "cine"


def voice_url(name: str) -> str:
    v = VOICES[name]
    return f"{PIPER_BASE}{v.folder}/{v.model}.onnx"


def kokoro_dir() -> Path:
    return models_dir() / "kokoro"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def models_dir() -> Path:
    return user_data_path("jarvis") / "voz"


class VoiceUnavailableError(RuntimeError):
    """Faltan las bibliotecas o los modelos de voz."""


def check(voice: str = DEFAULT_VOICE) -> str | None:
    """`None` si la voz puede andar; si no, qué falta (para mostrarlo)."""
    spec = VOICES.get(voice, VOICES[DEFAULT_VOICE])
    try:
        import faster_whisper  # noqa: F401
        import sounddevice  # noqa: F401

        if spec.engine == "kokoro":
            import kokoro_onnx  # noqa: F401
        else:
            import piper  # noqa: F401
    except ImportError as e:
        return f"falta {e.name}: corré `uv sync --extra voice`"
    if spec.engine == "kokoro":
        if not all((kokoro_dir() / f).exists() for f in KOKORO_FILES):
            return "faltan los archivos de Kokoro: corré `uv run jarvis voz instalar`"
    elif not (models_dir() / f"{spec.model}.onnx").exists():
        return "faltan los modelos: corré `uv run jarvis voz instalar`"
    return None


def sentences(text: str, min_chars: int = 24) -> list[str]:
    """Parte el texto en oraciones (para empezar a hablar con la primera), juntando las muy
    cortas con su vecina: una por una sonarían entrecortadas."""
    parts = [p.strip() for p in re.split(r"(?<=[.!?;:…])\s+", text) if p.strip()]
    out: list[str] = []
    for p in parts:
        if out and (len(out[-1]) < min_chars or len(p) < min_chars):
            out[-1] = f"{out[-1]} {p}"
        else:
            out.append(p)
    return out


class Voice:
    """Micrófono → texto, y texto → parlantes. Los callbacks se llaman desde el hilo del audio."""

    def __init__(self, voice: str = DEFAULT_VOICE) -> None:
        voice = voice if voice in VOICES else DEFAULT_VOICE
        problem = check(voice)
        if problem and VOICES[voice].engine == "kokoro" and check(FALLBACK_VOICE) is None:
            log.warning("voz %s: %s; uso %s (Piper)", voice, problem, FALLBACK_VOICE)
            voice, problem = FALLBACK_VOICE, None
        if problem:
            raise VoiceUnavailableError(problem)
        from faster_whisper import WhisperModel

        try:
            # Sin red si ya está bajado (si no, lo baja).
            self._stt = WhisperModel(
                WHISPER_MODEL, device="cpu", compute_type="int8", local_files_only=True
            )
        except Exception:
            self._stt = WhisperModel(WHISPER_MODEL, device="cpu", compute_type="int8")
        self.spec = VOICES[voice]
        self._tts: Any
        if self.spec.engine == "kokoro":
            from kokoro_onnx import Kokoro

            self._tts = Kokoro(
                str(kokoro_dir() / "kokoro-v1.0.int8.onnx"), str(kokoro_dir() / "voices-v1.0.bin")
            )
        else:
            from piper import PiperVoice

            self._tts = PiperVoice.load(str(models_dir() / f"{self.spec.model}.onnx"))
        log.info("voz: %s (%s, %s)", voice, self.spec.engine, self.spec.model)
        self._listen_now = threading.Event()
        self._speaking = threading.Event()
        self._hush = threading.Event()
        self._stop = threading.Event()

    # --- oír -------------------------------------------------------------------------------

    def listen_now(self) -> None:
        """Escuchar un pedido sin esperar la palabra de activación (Win+J en JARVIS-OS)."""
        self._listen_now.set()

    def run(self, on_event: Callable[[Event], None]) -> None:
        """Bucle del micrófono (bloquea: correrlo en un hilo). `on_event`: las órdenes y los
        cambios de estado que decide `Listener`."""
        import sounddevice as sd

        seg = Segmenter()
        listener = Listener()
        deaf_until = 0.0  # el eco de lo que acaba de decir

        def emit(events: list[Event]) -> None:
            for ev in events:
                on_event(ev)

        with sd.RawInputStream(samplerate=RATE, channels=1, dtype="int16", blocksize=FRAME) as mic:
            log.info("voz: escuchando el micrófono (%s)", sd.query_devices(kind="input")["name"])
            while not self._stop.is_set():
                data, _ = mic.read(FRAME)
                now = time.monotonic()
                if self._listen_now.is_set():
                    self._listen_now.clear()
                    emit(listener.listen_now(now))
                if self._speaking.is_set():
                    # No escucharse a sí mismo: lo que venía oyendo se descarta, y el eco también.
                    seg = Segmenter()
                    deaf_until = now + ECHO_MS / 1000
                    listener.speaking(deaf_until)
                    continue
                if now < deaf_until:
                    continue
                phrase = seg.feed(bytes(data))
                emit(listener.tick(now))
                if phrase is None:
                    continue
                log.info("voz: frase de %d ms, transcribiendo", len(phrase) * 500 // RATE)
                text = self.transcribe(phrase)
                if not text:
                    continue
                if is_noise(text):
                    log.info("voz: descarto %r (ruido)", text)
                    continue
                log.info("voz: oí %r", text)
                # Con la hora en que terminó la frase (transcribir tarda): una dicha justo antes
                # de que venza la conversación sigue contando.
                emit(listener.phrase(text, now))

    def transcribe(self, pcm: bytes) -> str:
        if not pcm:
            return ""
        import numpy as np

        audio = np.frombuffer(pcm, dtype=np.int16).astype(np.float32) / 32768.0
        segments, _ = self._stt.transcribe(
            audio, language="es", vad_filter=True, initial_prompt="JARVIS, abrí el navegador."
        )
        return "".join(s.text for s in segments).strip()

    # --- hablar ----------------------------------------------------------------------------

    def hush(self) -> None:
        """Corta lo que está diciendo (Roman pidió otra cosa o canceló)."""
        if self._speaking.is_set():
            self._hush.set()

    def _render(self, text: str) -> tuple[int, Iterator[bytes]]:
        """El audio de `text` de a oraciones, con el efecto de la voz elegida: la primera se
        sintetiza enseguida y las demás en otro hilo mientras suena la anterior."""
        spec = self.spec
        if spec.engine == "kokoro":
            from jarvis.voice import dsp

            rate = 24_000

            def one(sentence: str) -> bytes:
                samples, sr = self._tts.create(
                    sentence, voice=spec.model, speed=spec.speed, lang=spec.lang
                )
                x = dsp.to_float(dsp.to_pcm(samples))
                return dsp.to_pcm(dsp.jarvis_chain(x, sr) if spec.effect == "dsp" else x)

            return rate, _ahead(sentences(text), one, self._hush.is_set)
        rate, chunks = self._synthesize(text)
        if spec.effect:
            effect = cinema_effect if spec.effect == "cine" else jarvis_effect
            chunks = [effect(c, rate) for c in chunks]
        # Más lenta = más grave y pausada.
        return int(rate * spec.speed), iter(chunks)

    def speak_pcm(
        self, text: str, on_chunk: Callable[[int, bytes], None], paced: bool = True
    ) -> None:
        """Como `speak`, pero el audio lo reproduce JARVIS-OS (K12): se le manda de a pedazos
        (`on_chunk(frecuencia, pcm)`), al ritmo en que suena (o todo junto, sin `paced`)."""
        text = for_speech(text)
        if not text:
            return
        self._hush.clear()
        self._speaking.set()
        try:
            rate, chunks = self._render(text)
            if paced:
                send_paced(chunks, rate, on_chunk, self._hush.is_set)
            else:
                for chunk in chunks:
                    on_chunk(rate, chunk)
        finally:
            self._hush.clear()
            self._speaking.clear()

    def speak(self, text: str, on_level: Callable[[int], None]) -> None:
        """Dice `text` por los parlantes; `on_level` recibe el nivel cada ~50 ms."""
        import sounddevice as sd

        text = for_speech(text)
        if not text:
            return
        self._hush.clear()
        self._speaking.set()
        try:
            rate, chunks = self._render(text)
            step = rate // 20 * 2  # 50 ms de audio de 16 bits
            with sd.RawOutputStream(samplerate=rate, channels=1, dtype="int16") as out:
                for pcm in chunks:
                    for i in range(0, len(pcm), step):
                        if self._hush.is_set():
                            return
                        piece = pcm[i : i + step]
                        on_level(level(piece))
                        out.write(piece)
        finally:
            on_level(0)
            self._hush.clear()
            self._speaking.clear()

    def _synthesize(self, text: str) -> tuple[int, list[bytes]]:
        voice: Any = self._tts
        if hasattr(voice, "synthesize") and hasattr(voice.config, "sample_rate"):
            chunks = []
            for chunk in voice.synthesize(text):
                data = getattr(chunk, "audio_int16_bytes", chunk)
                chunks.append(bytes(data))
            return int(voice.config.sample_rate), chunks
        return int(voice.config.sample_rate), [b"".join(voice.synthesize_stream_raw(text))]

    def stop(self) -> None:
        self._stop.set()


def _ahead(
    items: list[str], make: Callable[[str], bytes], cancelled: Callable[[], bool]
) -> Iterator[bytes]:
    """`make(item)` para cada uno en otro hilo, mientras quien consume usa los anteriores. (Sin
    tope: si se deja de consumir, el hilo no se queda trabado; `cancelled` corta lo que falta.)"""
    out: queue.Queue[bytes | BaseException | None] = queue.Queue()

    def work() -> None:
        try:
            for item in items:
                if cancelled():
                    break
                out.put(make(item))
        except BaseException as e:  # se relanza del lado de quien consume
            out.put(e)
        out.put(None)

    threading.Thread(target=work, name="jarvis-voz-sintesis", daemon=True).start()
    while (got := out.get()) is not None:
        if isinstance(got, BaseException):
            raise got
        yield got
