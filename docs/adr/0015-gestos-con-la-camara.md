# ADR 0015: Gestos de la mano con la cámara del anfitrión

- **Estado:** aceptada
- **Fecha:** 2026-10-05

## Contexto

Roman quiere manejar JARVIS-OS con gestos de la mano frente a la cámara de la PC. JARVIS-OS
corre en QEMU y no ve la cámara (no hay driver USB de video, UVC, ni visión por computadora en
el kernel); el anfitrión sí, y ya tiene el cerebro, conectado al kernel (ADR 0008).

## Decisión

1. **El reconocimiento corre en el anfitrión** (`src/jarvis/gestures/`): OpenCV lee la cámara y
   MediaPipe Hands da los 21 puntos de la mano; `classify.py` (lógica pura, con tests) los
   convierte en gestos: señalar (mueve el puntero), pinza (clic), dos dedos (rueda), palma rápida
   a un costado (otro escritorio virtual u otra ventana) y palma quieta (menú de inicio).
   Es un extra: `uv sync --extra gestures`.
2. **Al kernel solo llegan eventos chicos**: `gesto{tipo, ...}` y `camara{estado}`. Las
   imágenes no salen del proceso del cerebro ni se guardan.
3. **La cámara se prende solo con los gestos activados** (Configuración → Mouse y teclado, o
   Win+A): el kernel manda `gestos{activo}` (y lo dice en `hola`). Si el kernel se desconecta,
   se apaga. Si no hay cámara o faltan las bibliotecas, se ve en Configuración.
4. El kernel aplica cada gesto como si fuera el mouse, la rueda o un atajo
   (`Desktop::gesture`), así sirve en cualquier app sin que las apps sepan de gestos.

## Consecuencias

- (+) Sin drivers nuevos en el kernel, y la privacidad queda simple: nada de video sale del
  anfitrión.
- (−) En una PC real sin anfitrión no hay gestos (haría falta UVC por USB y visión en el kernel).
- (−) Señalar con el dedo es menos preciso que un mouse: el puntero va suavizado y la zona útil
  de la imagen cubre toda la pantalla.
