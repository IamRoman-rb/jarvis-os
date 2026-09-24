# Kernel de JARVIS-OS

Kernel propio en Rust para x86_64 con arranque UEFI. Decisión y motivos: [ADR 0003](adr/0003-kernel-propio-rust.md).
Red y navegador: [ADR 0004](adr/0004-red-y-navegador-propio.md).

| Escritorio | Ventanas |
|---|---|
| ![JARVIS](img/k3-escritorio.png) | ![Archivos y monitor](img/k3-monitor.png) |
| **Navegador (HTTPS)** | **Alt+Tab** |
| ![Navegador](img/k3-web.png) | ![Alt+Tab](img/k3-alt-tab.png) |

- **JARVIS**: la esfera gira en tiempo real y late cuando JARVIS habla. El reloj usa una fuente
  vectorial propia (nítida a cualquier tamaño).
- **Ventanas como en Windows**: barra de título con minimizar, maximizar y cerrar; arrastrar para
  mover, esquina para cambiar el tamaño, doble clic para maximizar. Alt+Tab, Win+D, Win+flechas,
  Alt+F4, menú de inicio, vista de tareas y pantalla de bloqueo.
- **Barra de arriba a la izquierda** (lanzador y barra de tareas): inicio, consola de JARVIS
  (micrófono), monitor del sistema, captura de pantalla, Archivos, música, navegador y JARVIS.
  Un puntito marca las apps abiertas; la que tiene el foco se resalta.
- **Panel de estado** (abajo a la izquierda) con gráficos en vivo de CPU, memoria, disco y red.
  Clic: abre el monitor. "Control de misión": la vista de tareas.
- **Apps**: Archivos (FAT32 propio), Monitor del sistema, Consola JARVIS, Editor de texto, Música
  (parlante de la PC), Visor de imágenes (BMP) y Navegador.
- **Red propia**: driver virtio-net, TCP/IP (smoltcp), DHCP y DNS. El navegador baja páginas
  `http://` directo y `https://` por un puente en el anfitrión (el kernel todavía no tiene TLS).

## Cómo correrlo

Requisitos: [rustup](https://rustup.rs) y [QEMU](https://www.qemu.org). En Windows también hace
falta el compilador de C++ de Visual Studio (Build Tools). El toolchain nightly correcto se instala
solo la primera vez, porque está fijado en `kernel/rust-toolchain.toml`.

```bash
cd kernel
cargo xtask run          # QEMU con ventana, disco persistente, red, sonido y el puente HTTPS
cargo xtask test         # sin ventana, de punta a punta (lo usa la CI)
cargo xtask screenshot   # capturas del escritorio, las apps y los menús (en target/)
cargo xtask disk --reset # vuelve el disco a su contenido inicial (kernel/rootfs/)
cargo xtask vdi          # target/jarvis-os.vdi para bootear en VirtualBox (VM con EFI)
cargo test               # tests en el host: FAT32, red, escritorio y gráficos
```

- **El disco** es `kernel/target/disco.img` (64 MiB, FAT32, etiqueta `JARVIS`). Se crea la primera
  vez con el contenido de `kernel/rootfs/` y después **no se toca**: lo que hagas queda guardado
  entre reinicios. Se puede abrir desde Windows con 7-Zip para ver lo que creaste.
- **Mouse y teclado**: hacé clic en la ventana de QEMU para que los "capture" (Ctrl+Alt+G los
  libera). Mientras están capturados, la tecla Windows y Alt+Tab van a JARVIS-OS y no a Windows.
  Si Windows igual se queda con alguna combinación (Ctrl+Alt+Supr siempre es de Windows), usá el
  ícono de inicio, el menú de inicio o "Control de misión".
- **Sonido**: el parlante de la PC suena por DirectSound en Windows. En Linux, definí
  `QEMU_AUDIO=pa` (o `alsa`) antes de `cargo xtask run`.
- **Red**: QEMU da una red privada con DHCP (JARVIS-OS queda en 10.0.2.15) y sale a internet por
  la computadora anfitriona. El puente HTTPS escucha solo en 127.0.0.1:8118 mientras dura `run`.
- Los logs del kernel (puerto serie) salen en la terminal donde corriste `cargo xtask run`.
- En Windows, `xtask` usa la aceleración por hardware (WHPX) si está disponible.
- Para compilar mientras tenés QEMU abierto (la imagen queda bloqueada), usá otra carpeta de
  salida: `CARGO_TARGET_DIR=target/otra cargo xtask test`.

### Atajos de teclado (como en Windows)

| Atajo | Qué hace |
|---|---|
| Win (sola) | menú de inicio: escribí para buscar apps o una dirección web |
| Alt+Tab / Alt+Shift+Tab | cambiar de ventana (con Alt apretado se ve el selector) |
| Win+D | mostrar el escritorio (JARVIS); otra vez, volver |
| Win+M | minimizar todo |
| Win+E | Archivos |
| Win+R | Consola de JARVIS ("Ejecutar") |
| Win+X / Ctrl+Shift+Esc | Monitor del sistema |
| Win+Tab | vista de tareas ("Control de misión") |
| Win+↑ / Win+↓ | maximizar / restaurar o minimizar |
| Win+← / Win+→ | acoplar a la mitad izquierda / derecha |
| Win+1 … Win+8 | el ícono número N de la barra |
| Win+L | bloquear |
| Alt+F4 | cerrar la ventana; sin ventanas, apagar o reiniciar |
| F11 | maximizar la ventana |
| Impr Pant / Win+Shift+S | captura de pantalla a /Imágenes (BMP) |

En el escritorio (sin ventana con foco): Espacio o clic en la esfera hace hablar a JARVIS, Tab
abre Archivos.

| Archivos | |
|---|---|
| Enter / doble clic | abrir la carpeta, o el archivo (texto → editor, BMP → visor, HTML → navegador) |
| Retroceso / Alt+↑ | subir una carpeta · Alt+← atrás |
| F7 o Ctrl+Shift+N / F6 | nueva carpeta / nuevo archivo de texto |
| F2 | renombrar |
| Ctrl+C / Ctrl+X / Ctrl+V | copiar / cortar / pegar (también carpetas enteras) |
| Supr | a la Papelera (adentro de la Papelera: borrar definitivo, con confirmación) |
| RESTAURAR (en la Papelera) | vuelve a la carpeta de donde vino |
| Clic en NOMBRE / TAMAÑO / MODIFICADO | ordenar por esa columna |
| Letras | ir al primer elemento que empieza así |
| F5 | recargar |

| Navegador | |
|---|---|
| Ctrl+L / F6 / clic en la barra | escribir una dirección o una búsqueda; Enter va |
| Tab / Shift+Tab, Enter | recorrer los enlaces y abrir el elegido (o clic) |
| Alt+← / Retroceso · Alt+→ | atrás · adelante |
| F5 / Ctrl+R · Ctrl+H | recargar · inicio |
| Flechas, RePág, AvPág, Espacio, rueda | moverse por la página |

| Editor | Consola JARVIS | Música |
|---|---|---|
| Ctrl+S guarda (si es nuevo, pide la ruta) | `ayuda` lista las órdenes | ↑ ↓ elegir, Enter reproducir |
| Ctrl+Inicio / Ctrl+Fin | `abrir`, `ir`, `buscar`, `ls`, `cat`, `editar` | Esc detener |
| Cerrar con cambios avisa una vez | `hora`, `estado`, `red`, `captura`, `apagar` | |

El teclado usa la distribución de EE. UU.: los nombres con acentos se leen bien, pero todavía no
se pueden tipear.

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
            ├─ keyboard.rs    teclado PS/2 → teclas y modificadores (Alt, Ctrl, Win…)
            ├─ mouse.rs       mouse PS/2 con rueda (puerto auxiliar del 8042)
            ├─ allocator.rs   heap de 256 MiB (alloc: Vec, String)
            ├─ pci.rs         enumeración del bus PCI
            ├─ virtio_blk.rs  driver de disco virtio-blk (DMA, virtqueue, polling)
            ├─ virtio_net.rs  driver de placa de red virtio-net (dos virtqueues, polling)
            ├─ speaker.rs     parlante de la PC (canal 2 del PIT)
            ├─ power.rs       apagar (ACPI de QEMU) y reiniciar (8042)
            ├─ cpu.rs         nombre de la CPU (cpuid)
            ├─ rtc.rs         reloj CMOS → fecha y hora
            └─ bucle          hlt → red → eventos → render → present → pedidos → estadísticas
                 ├─ jarvis-net      TCP/IP (smoltcp), DHCP, DNS, descargas HTTP
                 └─ jarvis-desktop  ventanas, atajos, apps, barra, panel, menús
                      ├─ jarvis-fs   FAT32 sobre el disco (con caché de sectores)
                      └─ jarvis-gfx  dibujo: HUD, esfera, texto, fuente vectorial, figuras
```

| Crate | Qué es | Cómo se prueba |
|---|---|---|
| `gfx` (`jarvis-gfx`) | Dibujo: canvas con recorte y `blit`, paleta, texto, **fuente vectorial**, figuras, íconos, trigonometría en punto fijo, esfera, asistente, HUD. | `cargo test`: 37 tests |
| `fs` (`jarvis-fs`) | FAT32 propio: montaje, FAT (dos copias), nombres largos, lectura, escritura, carpetas, renombrar, mover, **copiar**, borrar, **caché de sectores**. Sobre un trait `BlockDevice`. | 22 tests, 14 de ellos **cruzados contra `fatfs`**: cada uno lee lo que escribe el otro, y el espacio libre se cuenta sobre la FAT cruda |
| `desktop` (`jarvis-desktop`) | Escritorio: gestor de ventanas, atajos, barra, panel de estado, menús, composición; apps (Archivos, Monitor, Consola, Editor, Música, Visor, Navegador); URL, HTTP y HTML. | 65 tests: el escritorio manejado con teclas y clics sobre un disco en memoria, verificado con `fatfs`; incluye "render incremental == redibujar todo" |
| `net` (`jarvis-net`) | Red: smoltcp, DHCP, DNS (con respaldo), descargas HTTP con redirecciones, HTTPS por el puente. | 3 tests de punta a punta en memoria (placa "loopback" + servidor HTTP de juguete) |
| `kernel` (`jarvis-kernel`) | El binario sin sistema operativo debajo. Solo hardware → eventos, bloques y píxeles. | `cargo xtask test` en QEMU |
| `xtask` | Imagen booteable, disco FAT32, QEMU (serie + monitor + red + audio), puente HTTPS, test de punta a punta, capturas. | Se usa en cada `cargo xtask` |

## Lo que se aprendió (y por qué el código es así)

### K3: ventanas, red y navegador
- **Composición por ventana**: cada ventana tiene su propio buffer y solo se redibuja cuando su
  app cambia. En cada frame se juntan las zonas que cambiaron (la esfera siempre, el reloj una vez
  por minuto, una ventana que se movió…), se restauran desde el fondo y se pintan las capas de
  atrás hacia adelante con **recorte**. Mover una ventana cuesta copiar dos rectángulos, no
  redibujar su contenido. Si una ventana maximizada tapa la esfera, la esfera ni se anima.
- **Qué cambió = comparar antes y después**: en vez de que cada acción avise qué redibujar, el
  escritorio saca una foto de la geometría de las ventanas antes de cada evento y la compara con
  la de después. Así no se puede olvidar un caso (y el test de render incremental lo verifica).
- **Modificadores como eventos**: Alt+Tab necesita saber cuándo se *suelta* Alt, y la tecla
  Windows sola abre el menú solo si no se usó en un atajo. Por eso el teclado manda un evento
  cada vez que cambian Shift, Ctrl, Alt o Win, y el escritorio lleva la cuenta.
- **Red por capas**: el driver (virtio-net) solo mueve tramas Ethernet; smoltcp arma IP, TCP,
  DHCP y DNS; `jarvis-net` hace las descargas; el navegador solo ve "pedí esta URL" y "llegó
  esta respuesta" (por el `Outbox`). Cada capa se prueba sola: la red, con una placa "loopback".
- **DNS con respaldo**: en la máquina donde se desarrolló, el DNS no resolvía algunos nombres
  (tampoco desde Windows: el problema era de esa red). Por eso hay DNS públicos de respaldo y,
  si igual falla, la página se pide por el puente.
- **Fuente vectorial**: la hora era la fuente bitmap agrandada ×2 (bordes borrosos). Ahora cada
  dígito son trazos que se rasterizan al tamaño exacto, midiendo la distancia de cada píxel al
  trazo, con 1 px de suavizado. Se rasteriza una vez y queda en caché.
- **Uso de CPU**: es el tiempo que el bucle **no** pasó dormido en `hlt`, medido con el TSC.

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

El orden cambió dos veces a pedido: el gestor de archivos (K2) y el escritorio con red (K3) se
adelantaron.

| Hito | Qué se logra | Qué se aprende |
|---|---|---|
| **K0** ✅ | Arranca en QEMU y dibuja el HUD | Arranque UEFI, framebuffer, E/S por puertos, `no_std` |
| **K1** ✅ | Interrupciones, timer + TSC, teclado, heap. Esfera animada y pulsaciones al hablar | Interrupciones, PIC, calibración de tiempo, concurrencia sin locks |
| **K2** ✅ | **Archivos**: PCI, virtio-blk, FAT32 propio, mouse PS/2 | Drivers con DMA, sistemas de archivos, UI dirigida por eventos |
| **K3** ✅ | **Escritorio y red**: ventanas y atajos como Windows, monitor, apps, virtio-net + TCP/IP, navegador | Composición, gestores de ventanas, redes, HTTP/HTML |
| K4 | **Puente con el cerebro**: la consola de JARVIS le habla a Claude (por la red, al `jarvis` del anfitrión) y la esfera pulsa con la respuesta | Protocolos, el sistema "piensa" |
| K5 | Paginación propia (tablas de páginas del kernel, no las del bootloader) | Memoria virtual, allocators de frames |
| K6 | Multitarea: scheduler y tareas del kernel. Disco y red por interrupciones | Cambio de contexto, sincronización |
| K7 | **TLS en el kernel** (sin puente) y teclado latinoamericano | Criptografía, certificados |
| K8 | Espacio de usuario: ring 3, syscalls, cargador ELF. Recién ahí se puede pensar en portar un navegador más completo | Aislamiento, ABI |
| K9 | Audio (virtio-sound/HDA) → voz real; la envolvente de la esfera sale del audio | Drivers de audio |
| K10 | Hardware real: placas de red Intel/Realtek, AHCI/NVMe, USB, ACPI, arranque en la PC | Drivers reales |

Recursos: [Writing an OS in Rust](https://os.phil-opp.com), la [wiki de OSDev](https://wiki.osdev.org),
la especificación de virtio y la especificación "Microsoft FAT32 File System".
