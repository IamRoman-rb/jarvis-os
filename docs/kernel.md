# Kernel de JARVIS-OS

Kernel propio en Rust para x86_64 con arranque UEFI. Decisión y motivos: [ADR 0003](adr/0003-kernel-propio-rust.md).

| JARVIS hablando | Archivos |
|---|---|
| ![JARVIS hablando](img/k1-hablando.png) | ![Gestor de archivos](img/k2-archivos.png) |

- **JARVIS**: la esfera gira en tiempo real y, cuando JARVIS habla, late con las sílabas, le corren
  ondas y brilla más mientras el mensaje se escribe letra por letra. Por ahora "habla" al arrancar
  y con **Espacio** (frases de demo): todavía no hay audio ni conexión con Claude.
- **Archivos** (**Tab**, o clic en la carpeta de la barra): gestor de archivos sobre un **disco
  virtual persistente** con **FAT32 escrito desde cero**. Crear carpetas (F7) y archivos (F6),
  renombrar (F2), mandar a la Papelera (Supr), vaciarla; teclado y mouse.

## Cómo correrlo

Requisitos: [rustup](https://rustup.rs) y [QEMU](https://www.qemu.org). En Windows también hace
falta el compilador de C++ de Visual Studio (Build Tools). El toolchain nightly correcto se instala
solo la primera vez, porque está fijado en `kernel/rust-toolchain.toml`.

```bash
cd kernel
cargo xtask run          # compila, arma la imagen UEFI y abre QEMU con el disco persistente
cargo xtask test         # sin ventana, de punta a punta (lo usa la CI)
cargo xtask screenshot   # capturas: reposo, hablando, Archivos y un diálogo
cargo xtask disk --reset # vuelve el disco a su contenido inicial (kernel/rootfs/)
cargo xtask vdi          # target/jarvis-os.vdi para bootear en VirtualBox (VM con EFI)
cargo test               # tests en el host: FAT32 (contra fatfs), escritorio y gráficos
```

- **El disco** es `kernel/target/disco.img` (64 MiB, FAT32, etiqueta `JARVIS`). Se crea la primera
  vez con el contenido de `kernel/rootfs/` y después **no se toca**: lo que hagas queda guardado
  entre reinicios. Se puede abrir desde Windows con 7-Zip para ver lo que creaste.
- **El mouse**: hacé clic en la ventana de QEMU para que lo "capture"; Ctrl+Alt+G lo libera.
- Los logs del kernel (puerto serie) salen en la terminal donde corriste `cargo xtask run`.
- En Windows, `xtask` usa la aceleración por hardware (WHPX) si está disponible.
- Para compilar mientras tenés QEMU abierto (la imagen queda bloqueada), usá otra carpeta de
  salida: `CARGO_TARGET_DIR=target/otra cargo xtask test`.

### Teclas

| Tecla | JARVIS | Archivos |
|---|---|---|
| Tab | abrir Archivos | volver a JARVIS |
| Espacio / Enter | JARVIS dice una frase | Enter: abrir carpeta |
| ↑ ↓ Inicio Fin RePág AvPág | | moverse por la lista |
| Retroceso / ← | | subir una carpeta / atrás en el historial |
| F7 / F6 | | nueva carpeta / nuevo archivo de texto |
| F2 | | renombrar |
| Supr | | mandar a la Papelera (adentro de la Papelera: borrar definitivo) |
| Esc | | cerrar el diálogo, o volver a JARVIS |

El teclado usa la distribución de EE. UU.: los nombres con acentos se leen bien, pero no se
pueden tipear todavía.

## Arquitectura

```
firmware UEFI (OVMF en QEMU)
  └─ bootloader (crate bootloader 0.11): modo 64 bits, tablas de páginas, framebuffer,
     mapeo de toda la memoria física
       └─ kernel_main (kernel/kernel/src/main.rs) — solo hardware
            ├─ serial.rs      COM1: logs al host
            ├─ gdt.rs         GDT + TSS (stack de emergencia para el doble fallo)
            ├─ interrupts.rs  IDT: excepciones, timer (IRQ0), teclado (IRQ1), mouse (IRQ12)
            ├─ pit.rs         PIT: despierta al bucle (250 Hz) y calibra el TSC
            ├─ time.rs        reloj en ms con el TSC
            ├─ queue.rs       cola de bytes sin locks (interrupción → bucle)
            ├─ keyboard.rs    teclado PS/2 → eventos del escritorio
            ├─ mouse.rs       mouse PS/2 (puerto auxiliar del 8042)
            ├─ allocator.rs   heap de 64 MiB (alloc: Vec, String)
            ├─ pci.rs         enumeración del bus PCI
            ├─ virtio_blk.rs  driver de disco virtio-blk (DMA, virtqueue, polling)
            ├─ rtc.rs         reloj CMOS → fecha y hora
            └─ bucle          hlt → eventos → Desktop::render → Desktop::present
                 └─ jarvis-desktop  modos, app Archivos, cursor
                      ├─ jarvis-fs   FAT32 sobre el disco
                      └─ jarvis-gfx  dibujo: HUD, esfera, texto, figuras
```

| Crate | Qué es | Cómo se prueba |
|---|---|---|
| `gfx` (`jarvis-gfx`) | Dibujo: canvas con recorte, paleta, texto, figuras, trigonometría en punto fijo, esfera, asistente, escena del HUD con doble buffer. | `cargo test`: 35 tests |
| `fs` (`jarvis-fs`) | FAT32 propio: montaje, FAT (dos copias), nombres largos, lectura, escritura, carpetas, renombrar, mover, borrar. Sobre un trait `BlockDevice`. | 17 tests, 11 de ellos **cruzados contra `fatfs`**: cada uno lee lo que escribe el otro, y el espacio libre se cuenta sobre la FAT cruda |
| `desktop` (`jarvis-desktop`) | Escritorio: eventos, decodificador del mouse, modos, app Archivos (estado, vista, diálogos), cursor. | 16 tests: la app manejada con teclas y clics sobre un disco en memoria, verificado después con `fatfs` |
| `kernel` (`jarvis-kernel`) | El binario sin sistema operativo debajo. Solo hardware → eventos, bloques y píxeles. | `cargo xtask test` en QEMU |
| `xtask` | Imagen booteable, disco FAT32, QEMU (serie + monitor), test de punta a punta, capturas. | Se usa en cada `cargo xtask` |

## Lo que se aprendió (y por qué el código es así)

### K2: disco, FAT32 y Archivos
- **DMA y direcciones físicas**: el disco virtio lee y escribe la memoria por su cuenta, con
  direcciones **físicas**. El heap está en una región física contigua mapeada linealmente, así que
  ahí física = virtual − offset. El stack no cumple eso, por eso el driver copia a un **buffer
  intermedio** propio antes de cada pedido.
- **Virtqueues**: en vez de imitar registros de un disco real, kernel y dispositivo comparten
  anillos en memoria. Un pedido son 3 descriptores (cabecera, datos, estado); el kernel lo publica,
  "toca el timbre" y espera (por *polling*) a que aparezca en el anillo de usados. Hay `fence`
  entre escribir el anillo y su índice: sin eso, el dispositivo podría ver el índice antes que el dato.
- **FAT32 es una lista enlazada en disco**, y cada error ahí corrompe el disco. Por eso hay una
  implementación de referencia en los tests (`fatfs`) y un contador independiente de clusters
  libres. Las operaciones que pueden fallar a la mitad se ordenan para no perder datos: al mover,
  primero se crea en el destino y después se borra del origen; si un renombrado no entra, se
  restaura el nombre viejo.
- **Nombres largos (LFN)**: se guardan en entradas extra de 13 caracteres UTF-16, en orden inverso
  y con un checksum del nombre corto. Cada archivo con nombre largo necesita además un alias 8.3
  único (`NOTASD~1.TXT`), que se ve en el inspector.
- **Borrar = mover a la Papelera**, la misma regla de seguridad que el cerebro de JARVIS. El borrado
  definitivo existe solo adentro de la Papelera y siempre pide confirmación.
- **Cursor por software**: se dibuja sobre la pantalla después de copiar el frame, restaurando
  antes lo que tapaba. Así nunca queda en el buffer donde se compone la imagen.
- **Mouse PS/2**: paquetes de 3 bytes; si se pierde uno, todo queda corrido. Por eso el
  decodificador se resincroniza con el bit 3, que siempre está en el primer byte.
- **El kernel como capa fina**: toda la interfaz vive en `jarvis-desktop` y se prueba sin QEMU. El
  test de "render incremental == redibujar todo" cubre también la ventana de Archivos.

### K0–K1: arranque, tiempo y gráficos
- **Sin `std`**: no hay sistema operativo que provea archivos, hilos ni memoria. El heap lo arma el
  propio kernel sobre la RAM libre que informa el bootloader.
- **Punto flotante por software** (target `x86_64-unknown-none`): por frame, todo es **punto fijo
  Q14** con una **tabla de senos** calculada al compilar. Los `f32` solo se usan al inicializar.
- **No medir el tiempo contando interrupciones**: en QEMU emulado se perdía un tercio y el reloj
  del kernel iba al 68 %. El tiempo sale del **TSC** calibrado contra el PIT, como en Linux.
- **Manejadores de interrupción mínimos** y colas **sin locks**: un manejador que espera un lock
  tomado por el bucle principal traba el kernel para siempre.
- **Doble buffer + rectángulos sucios + recorte**, con un test que exige que el render incremental
  sea idéntico, byte a byte, a dibujar todo de cero.

## Roadmap

El orden cambió: el gestor de archivos (disco, FAT32, mouse, ventanas) se adelantó a pedido.

| Hito | Qué se logra | Qué se aprende |
|---|---|---|
| **K0** ✅ | Arranca en QEMU y dibuja el HUD | Arranque UEFI, framebuffer, E/S por puertos, `no_std` |
| **K1** ✅ | Interrupciones, timer + TSC, teclado, heap. Esfera animada y pulsaciones al hablar | Interrupciones, PIC, calibración de tiempo, concurrencia sin locks |
| **K2** ✅ | **Archivos**: PCI, virtio-blk, FAT32 propio, mouse PS/2, escritorio con modos y ventana | Drivers con DMA, sistemas de archivos, UI dirigida por eventos |
| K3 | Paginación propia (tablas de páginas del kernel, no las del bootloader) | Memoria virtual, allocators de frames |
| K4 | **Puente con el cerebro**: escribís en el HUD → puerto serie → `jarvis` en el host → Claude → respuesta (y la esfera pulsa con ella). JARVIS podría usar el disco con las tools de la Fase 1a | Protocolos, el sistema "piensa" |
| K5 | Multitarea: scheduler y tareas del kernel. Disco por interrupciones en vez de polling | Cambio de contexto, sincronización |
| K6 | Editor de texto y visor de imágenes; teclado latinoamericano | Apps sobre el escritorio |
| K7 | Red: virtio-net + TCP/IP (smoltcp). El puente pasa a red | Drivers de red, pila TCP/IP |
| K8 | Espacio de usuario: ring 3, syscalls, cargador ELF | Aislamiento, ABI |
| K9 | Audio (virtio-sound/HDA) → voz real; la envolvente de la esfera sale del audio | Drivers de audio |
| K10 | Hardware real: AHCI/NVMe, USB, GPU básica, arranque en la PC | Drivers reales |

Recursos: [Writing an OS in Rust](https://os.phil-opp.com), la [wiki de OSDev](https://wiki.osdev.org),
la especificación de virtio y la especificación "Microsoft FAT32 File System".
