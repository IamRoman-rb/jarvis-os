"""La voz de JARVIS en el anfitrión (docs/investigacion.md §6.3), todo local:

- palabra de activación: "JARVIS". El micrófono está siempre abierto: cada frase (voz hasta un
  silencio) se transcribe, y solo las que empiezan con "JARVIS" son órdenes. "JARVIS" solo deja
  escuchando la frase siguiente;
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

from jarvis.voice.audio import FRAME, RATE, Segmenter, level, split_wake

log = logging.getLogger("jarvis.voz")

#: Cuánto espera la orden después de un "JARVIS" solo.
ARMED_MS = 8_000
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
        from piper import PiperVoice

        try:
            # Sin red si ya está bajado (si no, lo baja).
            self._stt = WhisperModel(
                WHISPER_MODEL, device="cpu", compute_type="int8", local_files_only=True
            )
        except Exception:
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
        import sounddevice as sd

        seg = Segmenter()
        armed_until = 0.0  # después de "JARVIS" solo (o Win+J), la frase siguiente es la orden
        with sd.RawInputStream(samplerate=RATE, channels=1, dtype="int16", blocksize=FRAME) as mic:
            log.info("voz: escuchando el micrófono (%s)", sd.query_devices(kind="input")["name"])
            while not self._stop.is_set():
                data, _ = mic.read(FRAME)
                now = time.monotonic()
                if self._listen_now.is_set():
                    self._listen_now.clear()
                    armed_until = now + ARMED_MS / 1000
                    on_listening(True)
                if self._speaking.is_set():
                    continue  # no escucharse a sí mismo
                phrase = seg.feed(bytes(data))
                if armed_until and now > armed_until:
                    armed_until = 0.0
                    on_listening(False)
                if phrase is None:
                    continue
                text = self.transcribe(phrase)
                if not text:
                    continue
                log.info("voz: oí %r", text)
                if armed_until:
                    armed_until = 0.0
                    on_listening(False)
                    order = split_wake(text)
                    on_heard(order if order else text)
                    continue
                order = split_wake(text)
                if order is None:
                    continue  # no le hablaban a JARVIS
                if order:
                    on_heard(order)
                else:
                    armed_until = time.monotonic() + ARMED_MS / 1000
                    on_listening(True)

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
