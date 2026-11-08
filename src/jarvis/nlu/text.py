"""Del texto a números: normalización y n-gramas de caracteres con "hashing".

Por qué n-gramas de **caracteres** y no palabras: "abrí", "abri", "abrime", "abrir" y "abiri"
(un error de tipeo, o de Whisper) comparten casi todos sus pedazos de 2 a 4 letras ("ab", "abr",
"bri"…), así que la red los ve parecidos aunque nunca haya visto la variante exacta.

Por qué **hashing**: en vez de un diccionario de n-gramas (que crece con cada frase nueva),
cada n-grama va a una de `DIM` casillas según su CRC-32. Dos n-gramas pueden caer en la misma
casilla (una "colisión"), pero con 4096 casillas y unos pocos miles de n-gramas casi no pasa, y
la red no necesita vocabulario. Se usa CRC-32 y no `hash()` porque el de Python cambia en
cada ejecución (y el modelo guardado dejaría de servir).
"""

from __future__ import annotations

import re
import unicodedata
import zlib

import numpy as np

DIM = 4096
NGRAMS = (2, 3, 4)

#: "JARVIS," al principio (y "che"/"hey" antes): no cambia la orden.
_WAKE = re.compile(
    r"^\s*(?:(?:che|hey|ey|oye|oi)\s+)?(?:jarvis|yarvis|jarbis|harvis)\b[\s,:.!¡]*", re.IGNORECASE
)


def strip_wake(text: str) -> str:
    return _WAKE.sub("", text, count=1)


def plain(text: str) -> str:
    """Minúsculas, sin tildes, sin signos (salvo los de rutas y direcciones), espacios simples."""
    t = unicodedata.normalize("NFD", strip_wake(text).lower())
    t = "".join(c for c in t if unicodedata.category(c) != "Mn")
    t = re.sub(r"[^a-z0-9/._\-~ñ ]+", " ", t)
    return " ".join(t.split())


def features(text: str) -> list[str]:
    """Los n-gramas de la frase: de caracteres (por palabra, con bordes) y las palabras."""
    words = plain(text).split()
    out = [f"w:{w}" for w in words]
    for w in words:
        padded = f" {w} "
        for n in NGRAMS:
            out.extend(padded[i : i + n] for i in range(len(padded) - n + 1))
    return out


def _bucket(feature: str) -> int:
    return zlib.crc32(feature.encode("utf-8")) % DIM


def vectorize(texts: list[str]) -> np.ndarray:
    """(N, DIM): cuántas veces aparece cada casilla, suavizado con log(1 + x) y normalizado
    (así una frase larga no "pesa" más que una corta)."""
    x = np.zeros((len(texts), DIM), dtype=np.float32)
    for row, t in enumerate(texts):
        for f in features(t):
            x[row, _bucket(f)] += 1.0
    np.log1p(x, out=x)
    norms = np.linalg.norm(x, axis=1, keepdims=True)
    np.divide(x, norms, out=x, where=norms > 0)
    return x
