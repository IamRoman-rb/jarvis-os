# ADR 0005 — Terminal propia, paquetes de JARVIS-OS y programas de otros sistemas

- **Estado:** aceptada
- **Fecha:** 2026-09-24

## Contexto

Roman pidió:

1. una **terminal para escribir comandos como en Linux**;
2. poder **descargar programas** "como en Linux, con gestores de paquetes", **y también archivos
   `.exe` de Windows**;
3. que el navegador muestre las páginas **con sus estilos** (en Google "no se ve ningún estilo,
   apenas los links").

El kernel de JARVIS-OS sigue siendo de un solo hilo y **sin espacio de usuario**: no hay ring 3,
llamadas al sistema, procesos ni cargador de programas (roadmap: K9). Todo lo que corre es código
del propio kernel.

## Decisión

### 1. Terminal y shell propias (`desktop/src/term/`, `apps/terminal.rs`)

Una shell parecida a `bash` (`jsh`) escrita en el crate del escritorio (`no_std`, testeable en el
host): tuberías, redirecciones, `&&`/`||`/`;`, comillas, variables, `$(…)`, comodines, alias,
historial, Tab para completar y colores ANSI. Los comandos (`ls`, `cd`, `cat`, `grep`, `sed`,
`find`, `cp`, `mv`, `rm`, `wget`, `curl`, `ps`, `kill`, `df`, `free`, `uname`…) están
implementados ahí mismo sobre el FAT32 propio. Hay un `/proc` virtual (`cpuinfo`, `meminfo`…).

Los comandos de red no pueden bloquear (el escritorio es un solo hilo): piden la descarga por el
`Outbox` y la shell queda en espera hasta que llega la respuesta, y ahí sigue con el resto de la
tubería y de la línea.

`rm`, `apt remove` y las sobreescrituras de `cp`/`mv`/`wget` **mueven a la Papelera** (la misma
regla que la app Archivos y el cerebro): nada se borra para siempre sin preguntar.

### 2. `apt` con paquetes de JARVIS-OS

- Los **programas** de JARVIS-OS son **scripts de `jsh`** (en `/Programas/bin`, que está en el
  `PATH`). También hay paquetes de datos (fondos de pantalla, documentos).
- El **repositorio** es la carpeta `kernel/paquetes/` del proyecto: un índice
  (`nombre|versión|descripción|dependencias|tamaño`) y un manifiesto por paquete
  (`origen -> /destino [bmp]`). `apt update/install/remove/upgrade/list/search/show` funcionan como
  en Debian, con dependencias y versiones. La base de datos local está en `/Sistema/paquetes/`.
- El repositorio lo sirve el **puente del anfitrión** en `http://paquetes.jarvis/` (solo lectura,
  nombres simples, sin salir de esa carpeta). Los nombres `.jarvis` van directo al puente, sin DNS.

Por qué no un repositorio en internet: el repositorio del proyecto es privado, y servirlo desde la
carpeta del proyecto hace que agregar un paquete sea agregar archivos y un renglón al índice.

### 3. Programas de Windows (`.exe`) y de Linux (ELF, `.deb`): se descargan e inspeccionan, no se ejecutan

Se pueden **descargar** (navegador → `/Descargas`, o `wget`), y `file`, `strings`, `xxd` y el
intento de ejecutarlos (`./programa.exe`, `wine programa.exe`, doble clic en Archivos) muestran
qué son: formato PE o ELF, arquitectura, si es de consola o con ventanas, un instalador conocido
(Inno Setup, NSIS), las DLL que importa. Y explican por qué **todavía no pueden correr**:

- Un `.exe` espera la API de Windows (Win32: `KERNEL32.dll`, `USER32.dll`…). Ejecutarlo requiere
  espacio de usuario, un cargador de PE y reimplementar esa API, que es lo que hace Wine en Linux
  (décadas de trabajo).
- Un programa de Linux espera las llamadas al sistema de Linux y su biblioteca de C (`glibc`).
  Esto es más alcanzable: con espacio de usuario (K9), un cargador de ELF y un subconjunto de
  llamadas al sistema, los programas estáticos simples podrían correr.

No se simula la ejecución: sería engañoso.

### 4. Navegador con CSS e imágenes

- **CSS propio** (`web/css.rs`): selectores (etiqueta, clase, id, descendiente, hijo), cascada con
  especificidad e `!important`, variables `var(--x)`, `@media` para pantallas anchas. Se aplican
  colores, fondos, tamaños, negrita, alineación, `display`, `list-style`, `text-transform`, etc.
  No hay maquetación de cajas: los hijos de un `display: flex` en fila (e `inline-block`) van en la
  misma línea; lo demás, uno debajo del otro.
- **Formularios** con GET (la caja de búsqueda de Google, DuckDuckGo, Brave Search…). Los POST no
  (el puente solo acepta GET; ADR 0004). Google ya no muestra resultados sin JavaScript: una
  búsqueda en Google se hace con el buscador elegido en la Configuración (DuckDuckGo, Brave
  Search, Bing o Wikipedia).
- **Imágenes**: el kernel solo decodifica BMP. El navegador pide las imágenes con la cabecera
  `X-Jarvis-Imagen: bmp` y **el puente las convierte** (PNG, JPEG, GIF, WebP, ICO → BMP de 24
  bits, achicadas a 900 px como máximo). Sigue siendo un GET; el puente agrega una conversión.

## Consecuencias

- Se puede trabajar en JARVIS-OS como en una terminal de Linux, instalar y desinstalar programas,
  y escribir programas propios (scripts) sin salir del sistema.
- Los `.exe` y los programas de Linux quedan para después de K9 (espacio de usuario). La terminal
  y la ayuda lo dicen claramente.
- El puente hace dos cosas más (repositorio y conversión de imágenes), siempre solo con GET y solo
  en 127.0.0.1 (regla 15 de CLAUDE.md). Cuando haya TLS y un decodificador PNG/JPEG en el kernel,
  la conversión se va; el repositorio podría pasar a un servidor en internet.
- Las páginas se ven con sus colores y tipografías relativas, pero no con su diseño en columnas;
  el modo lectura (F9) sigue disponible para las que quedan desordenadas.
