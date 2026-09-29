# ADR 0001 — Base Debian 13 y Python como lenguaje principal

- **Estado:** reemplazada por el [ADR 0003](0003-kernel-propio-rust.md) (kernel propio en Rust). Lo de Python sigue vigente para el cerebro.
- **Fecha:** 2026-09-22

## Contexto

JARVIS-OS necesita una base de sistema operativo de escritorio mantenible por una sola persona,
y un lenguaje para el demonio del asistente que tenga buen soporte para el SDK de Claude, audio
local y D-Bus. Análisis completo en [investigacion.md](../investigacion.md) §2, §11 y §12.

## Decisión

1. **Base:** Debian 13 "trixie" (amd64), remasterizada con `live-build` e instalador Calamares
   (fase 4). No se construye un SO desde cero (Yocto/Buildroot) ni se usa Cubic para la versión
   final. Durante el desarrollo, JARVIS se instala como servicio sobre un Debian 13 o
   Ubuntu 26.04 estándar.
2. **Escritorio:** XFCE en sesión X11. **Filesystem del instalado:** Btrfs con snapshots.
3. **Lenguaje principal:** Python ≥ 3.12, gestionado con `uv`. Bash solo como pegamento
   (live-build, instaladores, Termux). Kotlin opcional en la fase 3b.

## Consecuencias

- (+) Base estable y reproducible; el árbol de live-build se versiona en git.
- (+) El Claude Agent SDK, faster-whisper, openWakeWord y Piper son de primera clase en Python.
- (+) Un único runtime de lógica: menos herramientas que mantener.
- (−) Python arranca más lento y usa más memoria que Rust; si la latencia de audio lo exige,
  se puede reescribir solo ese camino crítico más adelante (requiere un ADR nuevo).
- (−) Mantener un ISO propio implica regenerarlo con cada actualización de seguridad; por eso
  el ISO se posterga a la fase 4.
