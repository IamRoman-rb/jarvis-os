"""La voz de JARVIS en el anfitrión (docs/investigacion.md §6.3), todo local:

- palabra de activación: "JARVIS". El micrófono está siempre abierto: cada frase (voz hasta un
  silencio) se transcribe, y `voice/listener.py` decide si es para JARVIS: las que empiezan con
  "JARVIS" y, durante una conversación (hasta 1 minuto sin hablarle), todas;
- voz → texto: faster-whisper (modelo `small`, español);
- texto → voz: Piper, con la voz es_AR "daniela".

Necesita `uv sync --extra voice` y los modelos (`jarvis voz instalar` los baja, avisando el
tamaño). El audio corre en un hilo aparte; los resultados vuelven al bucle de asyncio.
"""

from __future__ import annotations

import logging
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from platformdirs import user_data_path

from jarvis.voice.audio import (
    FRAME,
    RATE,
    Segmenter,
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
#: Voces: nombre → (modelo de Piper, carpeta en el repositorio, efecto "IA", velocidad).
#: "jarvis" es una voz masculina, grave y pausada con un leve brillo metálico: evoca a un
#: asistente de película sin imitar la voz de ningún actor.
VOICES: dict[str, tuple[str, str, bool, float]] = {
    "jarvis": ("es_ES-davefx-medium", "es/es_ES/davefx/medium", True, 0.92),
    "daniela": ("es_AR-daniela-high", "es/es_AR/daniela/high", False, 1.0),
}
DEFAULT_VOICE = "jarvis"


def voice_url(name: str) -> str:
    model, folder, _, _ = VOICES[name]
    return f"{PIPER_BASE}{folder}/{model}.onnx"


def models_dir() -> Path:
    return user_data_path("jarvis") / "voz"


class VoiceUnavailableError(RuntimeError):
    """Faltan las bibliotecas o los modelos de voz."""


def check(voice: str = DEFAULT_VOICE) -> str | None:
    """`None` si la voz puede andar; si no, qué falta (para mostrarlo)."""
    try:
        import faster_whisper  # noqa: F401
        import piper  # noqa: F401
        import sounddevice  # noqa: F401
    except ImportError as e:
        return f"falta {e.name}: corré `uv sync --extra voice`"
    if not (models_dir() / f"{VOICES.get(voice, VOICES[DEFAULT_VOICE])[0]}.onnx").exists():
        return "faltan los modelos: corré `uv run jarvis voz instalar`"
    return None


class Voice:
    """Micrófono → texto, y texto → parlantes. Los callbacks se llaman desde el hilo del audio."""

    def __init__(self, voice: str = DEFAULT_VOICE) -> None:
        voice = voice if voice in VOICES else DEFAULT_VOICE
        problem = check(voice)
        if problem:
            raise VoiceUnavailableError(problem)
        from faster_whisper import WhisperModel
        from piper import PiperVoice

        try:
            # Sin red si ya está bajado (si no, lo baja).
            self._stt = WhisperModel(
                WHISPER_MODEL, device="cpu", compute_type="int8", local_files_only=True
            )
        except Exception:
            self._stt = WhisperModel(WHISPER_MODEL, device="cpu", compute_type="int8")
        model, _, self._effect, self._speed = VOICES[voice]
        self._tts = PiperVoice.load(str(models_dir() / f"{model}.onnx"))
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

    def _render(self, text: str) -> tuple[int, list[bytes]]:
        """El audio de `text`, con el efecto y la velocidad de la voz elegida."""
        rate, chunks = self._synthesize(text)
        if self._effect:
            chunks = [jarvis_effect(c, rate) for c in chunks]
        # Más lenta = más grave y pausada.
        return int(rate * self._speed), chunks

    def speak_pcm(self, text: str, on_chunk: Callable[[int, bytes], None]) -> None:
        """Como `speak`, pero el audio lo reproduce JARVIS-OS (K12): se le manda de a pedazos
        (`on_chunk(frecuencia, pcm)`), al ritmo en que suena."""
        text = for_speech(text)
        if not text:
            return
        self._hush.clear()
        self._speaking.set()
        try:
            rate, chunks = self._render(text)
            send_paced(b"".join(chunks), rate, on_chunk, self._hush.is_set)
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
