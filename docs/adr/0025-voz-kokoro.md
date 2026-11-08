# ADR 0025: la voz de JARVIS con Kokoro y un procesado propio

- **Estado:** aceptada
- **Fecha:** 2026-11-08
- Reemplaza la voz de fábrica de la investigación inicial (§6.3, Piper).

## Contexto

Roman pidió que la voz se parezca "a la del JARVIS de la película". Esa voz es la de un actor
real: **clonarla o imitarla no se hace** (es la voz de una persona, sin su consentimiento). Lo que
sí se puede es lo que hace que suene a un asistente de película: una voz masculina, grave y
calma, hablada con prolijidad, con un leve timbre artificial y algo de sala.

La voz de antes era Piper (es_ES davefx) reproducida más lenta para que suene más grave: bajaba
el tono pero hacía todo más lento ("cámara lenta"), y Piper suena bastante robótica.

Opciones: Piper (rápida, calidad media), **Kokoro-82M** (Apache-2.0, 82 M de parámetros, de las
mejores voces abiertas de 2025; voces en castellano e inglés británico), XTTS/F5 (clonan voces:
justo lo que no queremos, y son pesadas). Kokoro corre en CPU con `kokoro-onnx` (ONNX Runtime,
que ya estaba por Piper y Whisper).

## Decisión

1. **Motor**: Kokoro-82M int8 con `kokoro-onnx`, voz `em_alex` (castellano, masculina). Con
   `voz = "jarvis-en"`, `bm_george` (inglés británico; `bm_lewis` es la otra opción). Piper
   queda de **respaldo**: si faltan los archivos de Kokoro se usa "cine" sola, con un aviso.
2. **Archivos**: `jarvis voz instalar` baja `kokoro-v1.0.int8.onnx` (88 MB) y
   `voices-v1.0.bin` (27 MB) de la release oficial de kokoro-onnx y verifica su **SHA-256**
   (fijado en `voice/engine.py`; la release no publica hashes, son los de la primera descarga:
   un archivo cambiado después no se usa).
3. **Procesado** (`voice/dsp.py`, NumPy, funciones puras con tests), en este orden: tono −1,5
   semitonos **sin cambiar el largo** (remuestreo + WSOLA), ecualizador (pasa-altos 80 Hz,
   +2,5 dB a 200 Hz, −3 dB a 3,5 kHz, +2 dB arriba de 9 kHz, en frecuencia y con fase cero),
   de-esser, compresor 3:1, brillo de IA (copia retrasada 4–7 ms que oscila, mezclada al 12 %),
   sala de Schroeder (4 peines + 2 pasa-todo) y normalización a −1 dBFS. Tarda ~0,06 s por
   frase: lo caro es la síntesis.
4. **Más rápido para empezar a hablar**: se sintetiza **de a oración** en otro hilo
   (`sentences`, `_ahead`) y la primera suena mientras se arma la segunda; `send_paced` las
   manda a JARVIS-OS con un solo reloj, sin cortes entre oraciones.
5. **Estilo de habla** (el prompt de `agent/prompts.py`): un mayordomo atento, cortés y sereno,
   con humor seco y discreto; nada de frases de películas ni de presentarse como personaje.

## Consecuencias

- (+) Voz mucho más natural, grave sin sonar lenta, y del mismo carácter en castellano e inglés.
- (+) Todo local, sin cuentas ni nube; el procesado está probado sin modelos (tests con tonos).
- (−) Kokoro en CPU sintetiza más o menos a tiempo real (4,4 s de audio en ~4,6 s en la PC de
  Roman): una frase larga tarda en arrancar; por eso se habla de a oración.
- (−) ~120 MB más de modelos (Whisper y Piper siguen).
- (−) El castellano de Kokoro pasa por espeak-ng para la fonética: algún nombre propio o sigla
  puede sonar raro.

Muestras: `uv run jarvis voz decir "texto" --wav archivo.wav` (y `--voz cine` para comparar con
la anterior).
