"""La voz de JARVIS en el anfitrión (docs/investigacion.md §6.3), todo local:

- palabra de activación: openWakeWord ("hey jarvis");
- voz → texto: faster-whisper (modelo `small`, español);
- texto → voz: Piper, con la voz es_AR "daniela".

Necesita `uv sync --extra voice` y los modelos (`jarvis voz instalar` los baja, avisando el
tamaño). El audio corre en un hilo aparte; los resultados vuelven al bucle de asyncio.
"""

from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

from platformdirs import user_data_path

from jarvis.voice.audio import FRAME, RATE, SpeechRecorder, level

log = logging.getLogger("jarvis.voz")

WAKE_MODEL = "hey_jarvis"
WAKE_THRESHOLD = 0.5
WHISPER_MODEL = "small"
PIPER_VOICE = "es_AR-daniela-high"
PIPER_URL = (
    "https://huggingface.co/rhasspy/piper-voices/resolve/main/es/es_AR/daniela/high/"
    f"{PIPER_VOICE}.onnx"
)


def models_dir() -> Path:
    return user_data_path("jarvis") / "voz"


class VoiceUnavailableError(RuntimeError):
    """Faltan las bibliotecas o los modelos de voz."""


def check() -> str | None:
    """`None` si la voz puede andar; si no, qué falta (para mostrarlo)."""
    try:
        import faster_whisper  # noqa: F401
        import openwakeword  # noqa: F401
        import piper  # noqa: F401
        import sounddevice  # noqa: F401
    except ImportError as e:
        return f"falta {e.name}: corré `uv sync --extra voice`"
    if not (models_dir() / f"{PIPER_VOICE}.onnx").exists():
        return "faltan los modelos: corré `uv run jarvis voz instalar`"
    return None


class Voice:
    """Micrófono → texto, y texto → parlantes. Los callbacks se llaman desde el hilo del audio."""

    def __init__(self) -> None:
        problem = check()
        if problem:
            raise VoiceUnavailableError(problem)
        from faster_whisper import WhisperModel
        from openwakeword.model import Model
        from piper import PiperVoice

        self._wake = Model(wakeword_models=[WAKE_MODEL], inference_framework="onnx")
        self._stt = WhisperModel(WHISPER_MODEL, device="cpu", compute_type="int8")
        self._tts = PiperVoice.load(str(models_dir() / f"{PIPER_VOICE}.onnx"))
        self._listen_now = threading.Event()
        self._speaking = threading.Event()
        self._stop = threading.Event()

    # --- oír -------------------------------------------------------------------------------

    def listen_now(self) -> None:
        """Escuchar un pedido sin esperar la palabra de activación (Win+J en JARVIS-OS)."""
        self._listen_now.set()

    def run(self, on_heard: Callable[[str], None], on_listening: Callable[[bool], None]) -> None:
        """Bucle del micrófono (bloquea: correrlo en un hilo)."""
        import numpy as np
        import sounddevice as sd

        with sd.RawInputStream(samplerate=RATE, channels=1, dtype="int16", blocksize=FRAME) as mic:
            recorder: SpeechRecorder | None = None
            while not self._stop.is_set():
                data, _ = mic.read(FRAME)
                pcm = bytes(data)
                if self._speaking.is_set():
                    continue  # no escucharse a sí mismo
                if recorder is None:
                    frame = np.frombuffer(pcm, dtype=np.int16)
                    woke = self._wake.predict(frame).get(WAKE_MODEL, 0.0) >= WAKE_THRESHOLD
                    if woke or self._listen_now.is_set():
                        self._listen_now.clear()
                        self._wake.reset()
                        recorder = SpeechRecorder()
                        on_listening(True)
                    continue
                if recorder.feed(pcm):
                    on_listening(False)
                    text = self.transcribe(recorder.speech())
                    recorder = None
                    if text:
                        on_heard(text)

    def transcribe(self, pcm: bytes) -> str:
        if not pcm:
            return ""
        import numpy as np

        audio = np.frombuffer(pcm, dtype=np.int16).astype(np.float32) / 32768.0
        segments, _ = self._stt.transcribe(audio, language="es", vad_filter=True)
        return "".join(s.text for s in segments).strip()

    # --- hablar ----------------------------------------------------------------------------

    def speak(self, text: str, on_level: Callable[[int], None]) -> None:
        """Dice `text` por los parlantes; `on_level` recibe el nivel cada ~50 ms."""
        import sounddevice as sd

        self._speaking.set()
        try:
            rate, chunks = self._synthesize(text)
            step = rate // 20 * 2  # 50 ms de audio de 16 bits
            with sd.RawOutputStream(samplerate=rate, channels=1, dtype="int16") as out:
                for pcm in chunks:
                    for i in range(0, len(pcm), step):
                        piece = pcm[i : i + step]
                        on_level(level(piece))
                        out.write(piece)
        finally:
            on_level(0)
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
