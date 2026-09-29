# ADR 0003 — Kernel propio en Rust

- **Estado:** aceptada
- **Fecha:** 2026-09-23
- **Reemplaza:** [ADR 0001](0001-base-debian-python.md) (base Debian 13 + XFCE)

## Contexto

La investigación inicial recomendaba construir JARVIS-OS sobre Debian + XFCE, con JARVIS como
servicio. Roman decidió que el proyecto sea un **sistema operativo completamente nuevo, con kernel
propio**, con una interfaz inspirada en su imagen de referencia: fondo oscuro, esfera de
partículas en el centro, reloj arriba a la derecha y el asistente como parte del sistema, no como
una app más.

Se evaluaron dos caminos: un shell propio sobre el kernel Linux (como ChromeOS o SteamOS) o un
kernel propio. Se eligió el **kernel propio**, sabiendo el costo: cada pieza de hardware necesita un
driver escrito por nosotros, y durante mucho tiempo el sistema va a correr en QEMU, no como sistema
de uso diario.

## Decisión

1. **Kernel propio en Rust** (`kernel/`), para x86_64 con arranque UEFI. Se eligió Rust sobre C
   porque el compilador ataja los errores de memoria que en un kernel cuelgan la máquina sin
   explicación, y porque el ecosistema de `rust-osdev` (crates `x86_64`, `bootloader`) está maduro.
2. **Bootloader:** crate `bootloader` 0.11 (UEFI). Pasa la CPU a 64 bits, mapea el kernel, pide el
   framebuffer al firmware y salta a `kernel_main`. Escribir nuestro propio bootloader no aporta
   al objetivo y se puede hacer más adelante.
3. **Nightly fijado** (`kernel/rust-toolchain.toml`): lo requiere `bootloader`. Fijar la fecha hace
   que el build sea reproducible.
4. **La lógica va en crates `no_std` testeables en el host** (`gfx`, y las que vengan). El binario
   del kernel solo conecta hardware con esa lógica. Así la mayor parte del código tiene tests
   comunes (`cargo test`), y el kernel completo se prueba arrancándolo en QEMU (`cargo xtask test`).
5. **El cerebro (Python + Claude) sigue en el host.** Un kernel nuevo no va a correr Python, TLS ni
   el Agent SDK en años. El código de la Fase 1a (agente, permisos de 3 niveles, auditoría) se
   mantiene, y el kernel le habla por un puente: primero el puerto serie (hito K4), después la red
   (K7). Las reglas de seguridad del cerebro siguen vigentes.
6. **Se descartan** XFCE, live-build, Calamares, Btrfs como decisión de base y la VM Debian de
   desarrollo. El diseño visual (`design/stitch/`) se mantiene y ahora se implementa en el propio kernel.

## Consecuencias

- (+) Todo lo que se ve y todo lo que corre es nuestro. Máximo aprendizaje: memoria, interrupciones,
  drivers, scheduling y sistemas de archivos.
- (+) El diseño HUD se implementa sin las restricciones de un escritorio existente.
- (−) Sin navegador, sin apps de terceros y con soporte de hardware mínimo por mucho tiempo.
  JARVIS-OS no reemplaza al sistema de uso diario en el corto plazo.
- (−) El cerebro depende de otra máquina (el host) hasta que el kernel tenga red y un camino
  propio hacia la API.
- (−) Lo que no esté en QEMU (Wi-Fi, GPU real, USB) requiere drivers específicos: es el hito K10.
