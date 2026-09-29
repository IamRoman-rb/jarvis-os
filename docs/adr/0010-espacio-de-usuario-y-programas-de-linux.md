# ADR 0010: espacio de usuario y programas de Linux

- **Estado:** propuesta
- **Fecha:** 2026-09-28

## Contexto

Hasta K10 todo lo que corre en JARVIS-OS es parte del kernel: el escritorio, las apps y la red son
tareas en el anillo 0 que comparten memoria. Los programas de Windows y de Linux se descargan e
inspeccionan pero no se ejecutan (ADR 0005, punto 3; regla 18 de CLAUDE.md): "ejecutarlos espera
al espacio de usuario (K11)".

K11 es ese paso: que un programa que no escribimos nosotros corra **aislado** (no puede tocar la
memoria del kernel ni la de otro programa) y hable con el sistema solo por llamadas al sistema.
Para no inventar un formato ni una ABI, la de Linux x86_64: así corren programas compilados para
Linux sin cambios.

## Decisión

1. **Procesos = tareas con su propio espacio de direcciones.** Cada proceso es una tarea del
   planificador (K9) con una PML4 propia. La mitad del kernel (toda entrada de la PML4 salvo la 0)
   se comparte: al arrancar se crean **todas** las entradas de la PML4 del kernel (511 tablas, 2
   MiB), así lo que el kernel mapee después (pilas, dispositivos) se ve en todos los procesos. La
   entrada 0 (los primeros 512 GiB) es del proceso: ahí van el programa, su heap, sus `mmap` y su
   pila. Las páginas del proceso llevan el bit USER; las del kernel no.
2. **Anillo 3 y `syscall`.** La GDT suma los segmentos de usuario; `syscall`/`sysret` (MSR STAR,
   LSTAR, SFMASK) para entrar y salir. Al cambiar de tarea se cambian CR3, la pila del kernel en la
   TSS (`rsp0`), FS (TLS del programa) y el estado de SSE (`fxsave`/`fxrstor`: el kernel no usa
   SSE, los programas sí). Un fallo en el anillo 3 (página inválida, instrucción inválida)
   termina el proceso ("violación de segmento"), no el sistema.
3. **La ABI de Linux en una crate `no_std` testeable** (`kernel/linux`, `jarvis-linux`): el
   cargador de ELF (estáticos y *static-pie*; los que piden un intérprete —bibliotecas
   dinámicas— se rechazan con un mensaje claro), la pila inicial (`argv`, `envp`, vector auxiliar
   con `AT_RANDOM`, `AT_PHDR`…), el mapa de memoria (brk, `mmap` anónimo, `mprotect`, con páginas
   que se asignan al primer uso) y la tabla de descriptores con las llamadas (`read`, `write`,
   `openat`, `getdents64`, `clock_gettime`, `getrandom`, `socket`…). La crate habla con el
   sistema por un trait; los tests usan uno de mentira.
4. **El disco lo sigue manejando el escritorio.** El FAT32 vive en la tarea del escritorio (K2):
   las llamadas de archivos de un proceso se le piden por una cola, como la red (K9), y el
   proceso espera la respuesta. Un archivo se lee entero al abrirlo y se escribe entero al
   cerrarlo (los archivos de JARVIS-OS son chicos). `unlink` manda a /Papelera (regla 13). La
   consola del proceso es la Terminal que lo lanzó: su salida aparece ahí y lo que se tipea es su
   entrada; Ctrl+C lo termina.
5. **Sockets con firewall.** `socket`/`connect`/`send`/`recv` (TCP, IPv4) usan las conexiones
   largas del `Outbox` (ADR 0007), así que pasan por las reglas de `ufw` como la app
   `programas` (regla 20; el log dice qué programa fue). Un programa no puede escuchar puertos (JARVIS-OS no acepta
   conexiones entrantes).
6. **Programas de prueba y un intérprete de JavaScript.** `kernel/usuario/` es un workspace
   aparte con programas en Rust para `x86_64-unknown-linux-musl` (estáticos, con la biblioteca
   estándar entera): se enlazan con `rust-lld` desde cualquier sistema, sin compilador de C. Entre
   ellos, `js`: un intérprete de JavaScript (el motor `boa`) que corre como programa de Linux.
   `cargo xtask` los compila, los pone en el disco de prueba y el puente los sirve en
   `http://paquetes.jarvis/usuario/` (solo lectura, desde `target/usuario/`), para instalarlos
   con `apt`. La regla 15 de CLAUDE.md se actualiza con esa carpeta.

## Qué no entra en K11

Bibliotecas dinámicas, hilos (`clone`), `fork`/`exec` desde un programa, señales de verdad,
memoria compartida y programas de Windows (PE). Brave nativo necesita todo eso más un servidor
gráfico: queda como la meta de este camino, no de este hito.

## Consecuencias

- (+) Aislamiento real: un programa con un bug no tumba el sistema ni lee la memoria del kernel.
- (+) Corren programas de Linux estáticos sin recompilar, y cualquier programa en Rust para musl.
- (−) Cada llamada de archivos cruza a la tarea del escritorio: es lenta comparada con Linux. Se
  acepta: el cuello de botella de hoy es el FAT32, no la cola.
- (−) Un solo núcleo: un programa que calcula sin parar se reparte la CPU con el escritorio por
  turnos de 10 ms (el desalojo de K9 lo permite).
- (−) La ABI de Linux es enorme; se implementa lo que piden los programas que corremos y el resto
  devuelve `ENOSYS` (y se anota en el log, para saber qué falta).
