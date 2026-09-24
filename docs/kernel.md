# Kernel de JARVIS-OS

Kernel propio en Rust para x86_64 con arranque UEFI. Decisión y motivos: [ADR 0003](adr/0003-kernel-propio-rust.md).

![JARVIS-OS en QEMU — hito K0](img/k0.png)

## Cómo correrlo

Requisitos: [rustup](https://rustup.rs) y [QEMU](https://www.qemu.org). En Windows también hace
falta el compilador de C++ de Visual Studio (Build Tools). El toolchain nightly correcto se instala
solo la primera vez, porque está fijado en `kernel/rust-toolchain.toml`.

```bash
cd kernel
cargo xtask run          # compila, arma la imagen UEFI y abre QEMU
cargo xtask test         # arranca sin ventana y verifica JARVIS_BOOT_OK (lo usa la CI)
cargo xtask screenshot   # guarda la pantalla en target/jarvis-os.png
cargo xtask vdi          # target/jarvis-os.vdi para bootear en VirtualBox (VM con EFI)
cargo test -p jarvis-gfx # tests de la lógica de dibujo, en el host
```

La salida del puerto serie (logs del kernel) aparece en la terminal donde corriste `cargo xtask run`.

## Arquitectura

```
firmware UEFI (OVMF en QEMU)
  └─ bootloader (crate bootloader 0.11): modo 64 bits, tablas de páginas, framebuffer
       └─ kernel_main (kernel/kernel/src/main.rs)
            ├─ serial.rs   COM1: logs al host
            ├─ rtc.rs      reloj CMOS → fecha y hora (UTC → hora de Argentina)
            └─ jarvis-gfx  HUD: fondo, esfera de partículas, reloj, paneles, mensaje
```

| Crate | Qué es | Cómo se prueba |
|---|---|---|
| `gfx` (`jarvis-gfx`) | Dibujo puro sobre un buffer: canvas, paleta, texto, figuras, esfera, fecha en castellano, composición del HUD. `no_std`. | `cargo test -p jarvis-gfx` en el host |
| `kernel` (`jarvis-kernel`) | El binario que corre sin sistema operativo debajo. Solo conecta hardware con la lógica. | Arrancándolo en QEMU (`cargo xtask test`) |
| `xtask` | Herramienta del host: build de la imagen, QEMU, capturas, VirtualBox. | Se usa en cada `cargo xtask` |

Detalles que conviene saber:

- **Sin `std`**: no hay sistema operativo que provea archivos, hilos ni memoria dinámica. Hasta
  K2 no hay heap: todo vive en el stack o en `static` (por eso existe `StrBuf`).
- **Punto flotante por software**: el target `x86_64-unknown-none` no usa SSE hasta que el kernel lo
  active (K1). La esfera se calcula con `libm` una sola vez al arrancar.
- **Resolución**: el kernel no pide un tamaño. El bootloader deja el modo del firmware (1280×800 en
  QEMU, la nativa del monitor en una PC real).
- **Hora**: el RTC guarda UTC y el kernel le resta 3 horas (`UTC_OFFSET_HOURS`).

## Roadmap

| Hito | Qué se logra | Qué se aprende |
|---|---|---|
| **K0** ✅ | Arranca en QEMU y dibuja el HUD: esfera, reloj real, paneles, mensaje | Arranque UEFI, framebuffer, E/S por puertos, `no_std` |
| K1 | GDT, IDT y excepciones; timer; teclado PS/2. Esfera animada y reloj en vivo | Interrupciones, PIC/APIC, manejo de fallos |
| K2 | Paginación propia, heap (`alloc`), doble buffer sin parpadeo | Memoria virtual, allocators |
| K3 | Mouse, compositor y widgets (ventanas del HUD) | Eventos, dibujo incremental |
| K4 | **Puente con el cerebro**: escribís en el HUD → puerto serie → `jarvis` en el host → Claude → respuesta en pantalla | Protocolos, el sistema "piensa" |
| K5 | Multitarea: scheduler y tareas del kernel | Cambio de contexto, sincronización |
| K6 | Disco (virtio-blk/AHCI) y sistema de archivos | Drivers de bloque, FAT/propio |
| K7 | Red: virtio-net + TCP/IP (smoltcp). El puente pasa a red | Drivers de red, pila TCP/IP |
| K8 | Espacio de usuario: ring 3, syscalls, cargador ELF | Aislamiento, ABI |
| K9 | Audio (virtio-sound/HDA) → voz | Drivers de audio, DMA |
| K10 | Hardware real: USB, GPU básica, arranque en la PC | Drivers reales |

Recursos: [Writing an OS in Rust](https://os.phil-opp.com) (sigue un camino muy parecido a K0–K5)
y la [wiki de OSDev](https://wiki.osdev.org).
