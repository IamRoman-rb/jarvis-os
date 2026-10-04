# Kernel de JARVIS-OS

Kernel propio en Rust para x86_64 con arranque UEFI. Decisión y motivos: [ADR 0003](adr/0003-kernel-propio-rust.md).
Red y navegador: [ADR 0004](adr/0004-red-y-navegador-propio.md). Terminal, paquetes y programas de
otros sistemas: [ADR 0005](adr/0005-terminal-paquetes-y-programas.md). Motor web, firewall, snap,
winget e idiomas: [ADR 0006](adr/0006-motor-web-firewall-tiendas-e-idiomas.md). Conexiones largas,
Brave remoto, sincronización e ISO: [ADR 0007](adr/0007-brave-remoto-y-sincronizacion.md).

**Brave en JARVIS-OS**: YouTube con JavaScript, en una ventana maximizada con la barra de arriba
(íconos, ventanas abiertas, CPU, memoria, disco, red, IP y hora):

![Brave](img/k6-brave.png)

| Wikipedia en el navegador | YouTube (sin JavaScript) |
|---|---|
| ![Wikipedia](img/k5-wikipedia.png) | ![YouTube](img/k5-youtube.png) |
| **snap y ufw en la terminal** | **Firewall en la Configuración** |
| ![snap y ufw](img/k5-snap-ufw.png) | ![Firewall](img/k5-firewall.png) |
| **Monitor 1 (extender)** | **Monitor 2, con el Monitor del sistema** |
| ![Pantalla 1](img/k6-pantalla1.png) | ![Pantalla 2](img/k6-pantalla2.png) |
| **Tema claro** | **Monitor con temperatura** |
| ![Tema claro](img/k6-claro.png) | ![Monitor](img/k6-monitor.png) |
| **Cerrar sesión** | |
| ![Sesión](img/k6-sesion.png) | |
| **Escritorio** | **Terminal y `apt`** |
| ![JARVIS](img/k4-escritorio.png) | ![Terminal](img/k4-terminal.png) |

- **Brave**: el navegador de verdad (JavaScript, YouTube, cualquier sitio). Corre en el
  anfitrión sin ventana y JARVIS-OS lo muestra en una ventana propia, con pestañas, barra de
  dirección, atrás/adelante, mouse, rueda y teclado. Se instala con `cargo xtask brave
  --instalar`. El navegador propio de K3–K5 queda como "Navegador simple".
- **JARVIS con Claude** (K7, ADR 0008): la consola resuelve sus órdenes locales y lo demás se lo
  pregunta a Claude (`jarvis serve` en el anfitrión, con el login de Claude Code); la respuesta
  aparece a medida que llega y la esfera habla. JARVIS también **actúa**: abre apps y páginas,
  lee, crea y mueve archivos, cierra ventanas, manda a la Papelera o ejecuta comandos, con los
  3 niveles de `docs/permisos.md`: lo que cambia algo pide permiso en un diálogo que muestra la
  acción completa, y el nivel 3 solo se aprueba con un clic.
  "Abrí jarvis-os y seguí con lo que estábamos": JARVIS abre un **agente de código** (el de Claude
  Code) en esa carpeta del anfitrión, retoma la última conversación y muestra su avance en la
  ventana **Proyecto**; cada edición (nivel 2) y cada comando (nivel 3) se confirman.
  **Por voz**: "JARVIS, …" (o Win+J; "JARVIS" solo contesta "¿Sí?" y espera la orden) con el micrófono del anfitrión; lo que entiende se trata
  como si se hubiera escrito, y la respuesta se dice en voz alta mientras la esfera se mueve con
  el audio real (`uv sync --extra voice` y `uv run jarvis voz instalar`, ~590 MB de modelos). El micrófono queda siempre abierto: cada frase se transcribe local y solo las que empiezan con "JARVIS" son órdenes; nada sale de la PC.
- **Paginación propia** (K8, `paging.rs` + crate `jarvis-mem`): al arrancar, el kernel deja las
  tablas de páginas del bootloader y arma las suyas con su propio allocator de marcos físicos.
  El código del kernel queda de solo lectura y los datos sin permiso de ejecución (**W^X**,
  verificado en cada arranque); la RAM se mapea con páginas de 2 MiB (o de 1 GiB si la CPU las
  tiene): 29 tablas en vez de las ~1050 del bootloader. El Monitor muestra la RAM física libre
  ([captura](img/k8-monitor.png)).
- **Multitarea** (K9, `task.rs` + crate `jarvis-task`): el kernel tiene **tareas** con su propia
  pila y un planificador con prioridades y desalojo. El escritorio, la red y la tarea ociosa
  corren por separado: la pila TCP/IP ya no espera a que termine un cuadro, y cuando nadie tiene
  nada que hacer la CPU duerme. El **disco y la red avisan por interrupción** (su línea PCI): la
  tarea que espera un sector se duerme hasta que llega, en vez de dar vueltas. `ps` en la
  Terminal muestra las tareas del kernel con su tiempo de CPU.
- **Micrófono** (driver `virtio_sound.rs`, sobre `virtio_modern.rs`): QEMU agrega una placa
  virtio-sound conectada al micrófono del anfitrión; el kernel configura su entrada (PCM de 16 bits
  a 16 kHz) y recibe audio en buffers de 20 ms. Configuración → **Micrófono** muestra si se
  detectó, el nivel en vivo y el estado de la voz de JARVIS.
- **Sonido** (K12): la misma placa virtio-sound tiene salida a los parlantes del anfitrión. Un
  mezclador suma lo que suena: la app **Música** (las partituras con un sintetizador, y los WAV
  de `/Música`), el **Visor**, que ahora también reproduce **videos** AVI (MJPEG con audio), y la
  **voz de JARVIS**, que el cerebro manda como audio y el kernel reproduce: la esfera se mueve
  con lo que está sonando. `apt install musica videos` trae ejemplos. Sin placa de sonido,
  Música vuelve al parlante de la PC.
- **Sincronización entre máquinas**: la carpeta `/Sincronizado` se copia sola entre dos (o más)
  JARVIS, aunque estén en redes distintas. En Configuración → Sincronización se genera un código
  en una y se escribe en la otra; las dos se conectan a un **relé** (`cargo xtask relay`) que solo
  ve bytes cifrados. Si las dos cambian el mismo archivo, gana el cambio más nuevo y el otro queda
  como `nombre (conflicto de PC2).ext`; un borrado remoto va a la Papelera.
- **ISO**: `cargo xtask iso` arma `target/jarvis-os.iso`, que arranca como CD (UEFI). Sin disco,
  arranca en **modo en vivo**: un FAT32 en RAM de 48 MiB (lo que se guarda se pierde al apagar).
- **Varios monitores**: con la placa virtio-gpu (QEMU la trae con una salida por monitor de la
  PC), extender, duplicar o usar uno solo (Win+P o Configuración → Pantallas); el segundo a la
  derecha o abajo, y cuál es el principal. Win+Shift+←/→ lleva una ventana al otro monitor;
  maximizar, acoplar y las distribuciones usan el monitor de la ventana.
- **Selección como en Windows**: en Archivos, varios a la vez (Shift+flechas, Ctrl+clic,
  Shift+clic, Ctrl+E) para mover a la Papelera, copiar o cortar; en el Editor, Shift+flechas,
  arrastrar con el mouse, doble clic en una palabra y Ctrl+E, con un portapapeles del sistema.
- **Distribuciones de ventanas**: Win+Z con seis plantillas, cuartos con Win+flechas, mosaico,
  cascada, y acoplar arrastrando contra un borde o una esquina.
- **Personalización en capas** (como Windows o KDE), en la Configuración:
  - **Apariencia**: tema HUD oscuro, claro o de alto contraste, y ocho colores de acento.
  - **Tipografía**: texto grande, títulos en negrita, y la letra de la terminal y del editor.
  - **Ventanas**: animación (ninguna, desvanecer, zoom, deslizar) y su velocidad, botones del
    título a la izquierda o a la derecha, qué hace el doble clic, acoplar al arrastrar contra un
    borde, arrastrar solo el contorno, y que el foco siga al mouse.
  - **Barra y cursor**: barra de arriba sí/no, qué gráficos muestra, segundos en el reloj y
    cursor grande.
- **Energía**: apagar, reiniciar, **suspender** (pantalla negra, nada se dibuja; una tecla o el
  mouse despiertan, con PIN si hay) y **cerrar sesión** (cierra las apps; si el Editor tiene
  cambios sin guardar, no sigue). Desde el menú Inicio, Win+X, Alt+F4 en el escritorio o el botón
  de la punta derecha de la barra de arriba.
- **Temperatura** de la CPU en el Monitor, la barra de arriba y el panel de estado, leída del
  sensor térmico de Intel. En QEMU dice "sin sensor": las máquinas virtuales no lo emulan.
- **JARVIS**: la esfera gira en tiempo real y late cuando JARVIS habla. El reloj usa una fuente
  vectorial propia (nítida a cualquier tamaño), en 24 o 12 horas.
- **Ventanas como en Windows**: barra de título con minimizar, maximizar y cerrar; arrastrar para
  mover, esquina para cambiar el tamaño, doble clic para maximizar. Alt+Tab, Win+D, Win+flechas,
  escritorios virtuales, enlaces rápidos (Win+X), configuración rápida (Win+A), notificaciones con
  calendario (Win+N), menú de la ventana (Alt+Espacio) y más: ver la tabla de abajo o F1.
- **Barra de arriba**: al maximizar una ventana ocupa toda la pantalla, y arriba aparece una barra
  con los íconos, las ventanas abiertas (clic para traerla o minimizarla), gráficos en vivo de
  CPU, memoria, disco y red (clic: Monitor), la IP y la hora (clic: notificaciones).
- **Transiciones**: las ventanas aparecen, se cierran, se minimizan, vuelven, se maximizan y se
  acoplan con una animación corta (se apagan con "Animaciones" en la Configuración).
- **Terminal** (Ctrl+Alt+T): shell `jsh` parecida a bash, con tuberías, redirecciones, variables,
  comodines, historial, Tab y colores; ~80 comandos de Linux sobre el FAT32 propio y un `/proc`.
- **`apt`**: instala, actualiza y desinstala programas de JARVIS-OS desde el repositorio del
  proyecto (`kernel/paquetes/`). `neofetch`, `cowsay`, `fortune`, fondos de pantalla…
- **Programas de Windows y Linux**: se descargan (navegador, `wget`, `winget`, `snap download`) y
  se inspeccionan (`file`, `strings`, `xxd`); todavía no se pueden ejecutar (hace falta espacio de
  usuario: K11).
- **Configuración** (Win+I): fondo de pantalla, apariencia, tipografía, ventanas, idioma, zona horaria, reloj, red, navegador,
  sonido, mouse, teclado, programas, almacenamiento, PIN de bloqueo y firewall. Se guarda en
  `/Sistema/config.ini`.
- **Navegador con motor de maquetación propio**: cajas con márgenes y bordes, flotantes, flex,
  grid, tablas, posiciones, `@media`, `calc()`, variables; fuente proporcional (DejaVu) en
  cualquier tamaño; imágenes, SVG, fondos e íconos con transparencia (el puente los convierte a
  BMP). Formularios (GET), modo lectura (F9) y descargas a /Descargas. Sin JavaScript: YouTube se
  arma con los datos que trae la página; otras páginas así avisan que pueden verse incompletas.
- **Firewall**: reglas por sitio, puerto y app (`ufw` en la terminal o Configuración →
  Firewall). Lo bloqueado queda en `/Sistema/firewall.log`.
- **`snap`** (tienda propia con canales y revisiones; búsqueda y descarga en Snapcraft) y
  **`winget`** (instaladores de Windows del repositorio oficial de Microsoft, a /Descargas).
- **Idiomas**: castellano, inglés o portugués (Configuración → Hora e idioma).
- **Teclado latinoamericano** (ñ, tildes con tecla muerta, AltGr+Q = @) o de EE. UU.
- **Panel de estado** (abajo a la izquierda) con gráficos en vivo de CPU, memoria, disco y red.
- **Apps**: Brave, Archivos, Terminal, Configuración, Monitor, Consola JARVIS, Editor, Música,
  Visor y Navegador simple.
- **Red propia**: driver virtio-net, TCP/IP (smoltcp), DHCP y DNS. Las páginas `https://` pasan por
  un puente en el anfitrión (el kernel todavía no tiene TLS).

## Cómo correrlo

Requisitos: [rustup](https://rustup.rs) y [QEMU](https://www.qemu.org). En Windows también hace
falta el compilador de C++ de Visual Studio (Build Tools). El toolchain nightly correcto se instala
solo la primera vez, porque está fijado en `kernel/rust-toolchain.toml`.

```bash
cd kernel
cargo xtask run          # QEMU con ventana, disco persistente, red, sonido y el puente
cargo xtask test         # sin ventana, de punta a punta (lo usa la CI)
cargo xtask screenshot   # capturas del escritorio, las apps y los menús (en target/)
cargo xtask disk --reset # vuelve el disco a su contenido inicial (kernel/rootfs/)
cargo xtask vdi          # target/jarvis-os.vdi para bootear en VirtualBox (VM con EFI)
cargo xtask relay        # el relé de la sincronización (--publico para abrirlo a la red)
cargo xtask run2         # dos JARVIS (PC1 y PC2, cada uno con su disco) unidos por el relé
cargo xtask sincronizar  # prueba: dos QEMU se emparejan, se mandan archivos, renombre → Papelera
cargo xtask iso          # target/jarvis-os.iso (--probar: la arranca sin ventana; --abrir: con ventana)
cargo xtask pantallas    # dos monitores: extender, mover una ventana, Win+P, duplicar (capturas)
cargo xtask brave --instalar           # instala Brave en el anfitrión (winget)
cargo xtask brave --probar https://…   # prueba el puente de Brave sin QEMU (target/brave-prueba.png)
cargo test               # tests en el host: FAT32, red, escritorio, terminal y gráficos
```

- **El disco** es `kernel/target/disco.img` (FAT32, etiqueta `JARVIS`). Se crea la primera vez con
  el contenido de `kernel/rootfs/` y después **no se toca**: lo que hagas queda guardado entre
  reinicios. Los discos nuevos son de 256 MiB (los creados antes de K4, de 64 MiB, siguen
  funcionando; `disk --reset` crea uno nuevo pero **borra** lo que tenía). Se puede abrir desde
  Windows con 7-Zip.
- **Mouse y teclado**: hacé clic en la ventana de QEMU para que los "capture" (Ctrl+Alt+G los
  libera). Mientras están capturados, la tecla Windows y Alt+Tab van a JARVIS-OS y no a Windows.
  Si Windows igual se queda con alguna combinación (Ctrl+Alt+Supr siempre es de Windows), usá el
  ícono de inicio, el menú de inicio o "Control de misión".
- **Teclado**: por defecto, latinoamericano. Si tu teclado es de EE. UU. (o los símbolos no
  coinciden), cambialo en Configuración → Mouse y teclado.
- **Sonido**: el parlante de la PC suena por DirectSound en Windows. En Linux, definí
  `QEMU_AUDIO=pa` (o `alsa`) antes de `cargo xtask run`.
- **Red**: QEMU da una red privada con DHCP (JARVIS-OS queda en 10.0.2.15) y sale a internet por
  la computadora anfitriona. El puente (HTTPS, imágenes y paquetes) escucha solo en
  127.0.0.1:8118 mientras dura `run`: sin `cargo xtask run`, no hay `https://`, `apt`, `snap` ni
  `winget`.
- **Brave**: mientras dura `cargo xtask run` también corre el puente de Brave (127.0.0.1:8119,
  que QEMU ve como 10.0.2.2:8119). Brave usa su propio perfil (`kernel/target/brave-perfil`:
  cookies y sesiones), separado del Brave que uses en Windows. Para usarlo desde otra máquina:
  `cargo xtask brave --red --token <secreto>` y, en esa máquina, Configuración → Navegador.
- **Monitores**: `xtask` le pregunta a Windows cuántos monitores hay y le da a QEMU una
  `virtio-vga` con una salida por monitor (QEMU abre una ventana o pestaña por salida).
  `JARVIS_MONITORES=2 cargo xtask run` suma uno virtual para probar con una sola pantalla.
- Los logs del kernel (puerto serie) salen en la terminal donde corriste `cargo xtask run`.
- En Windows, `xtask` usa la aceleración por hardware (WHPX) si está disponible.
- Para compilar mientras tenés QEMU abierto (la imagen queda bloqueada), usá otra carpeta de
  salida: `CARGO_TARGET_DIR=target/otra cargo xtask test`.

### Atajos de teclado (como en Windows)

| Atajo | Qué hace |
|---|---|
| Win (sola) · Win+S · Ctrl+Esc | menú de inicio: escribí para buscar apps o una dirección web |
| Alt+Tab / Alt+Shift+Tab | cambiar de ventana (con Alt apretado se ve el selector) |
| Alt+Esc | pasar a la ventana siguiente, sin selector |
| Win+Tab | vista de tareas ("Control de misión") |
| Win+D · Win+, | mostrar el escritorio (JARVIS); otra vez, volver |
| Win+M · Win+Shift+M | minimizar todo · volver a mostrar lo minimizado |
| Win+Inicio | minimizar todas menos la activa |
| Win+↑ / Win+↓ | maximizar / restaurar o minimizar; con la ventana en una mitad, el cuarto de arriba / abajo |
| Win+← / Win+→ · Win+Shift+↑ | acoplar a la mitad izquierda / derecha · estirar a lo alto |
| Win+Z | distribuciones: mitades, tercios, 2/3 + 1/3, cuartos, grande + dos, columna central (las demás ventanas completan) |
| Win+Shift+T · Win+Shift+C | mosaico con todas las ventanas · cascada |
| Win+J | hablarle a JARVIS sin decir "JARVIS" (micrófono del anfitrión) |
| Win+P · Win+Shift+← / → | monitores: solo 1, duplicar, extender, solo 2 · llevar la ventana al otro monitor |
| arrastrar contra un borde / esquina | mitad / cuarto (arriba: maximizar) |
| Win+Ctrl+D · Win+Ctrl+← / → · Win+Ctrl+F4 | escritorio virtual nuevo · cambiar · cerrarlo |
| Alt+Espacio · clic derecho en la barra de título | menú de la ventana (restaurar, minimizar, maximizar, acoplar, botones a la izquierda o derecha, cerrar) |
| Alt+F4 · Ctrl+W | cerrar la ventana (Ctrl+W si la app no lo usa); sin ventanas, Alt+F4 ofrece apagar |
| F11 | maximizar la ventana |
| Win+X | enlaces rápidos (apps, monitor, configuración, terminal, apagar…) |
| Win+A | configuración rápida (sonidos, animaciones, páginas claras, reloj…) |
| Win+N · Win+Alt+D | notificaciones y calendario |
| Win+I | Configuración |
| Win+E | Archivos |
| Win+R | Consola de JARVIS ("Ejecutar") |
| Ctrl+Alt+T · Win+Enter | Terminal (como en Ubuntu) |
| Ctrl+Shift+Esc · Ctrl+Alt+Supr | Monitor del sistema |
| Win+1 … Win+9, Win+0 | el ícono número N de la barra |
| Win+L | bloquear (con PIN, si hay uno en la Configuración) |
| Impr Pant · Win+Shift+S | captura de pantalla a /Imágenes (BMP) |
| Alt+Impr Pant | captura solo de la ventana activa |
| Win+Ctrl+Shift+B | redibujar toda la pantalla |
| F1 | ayuda con todos los atajos |

En el escritorio (sin ventana con foco): Espacio o clic en la esfera hace hablar a JARVIS, Tab
abre Archivos.

| Archivos | |
|---|---|
| Enter / doble clic | abrir: carpeta, texto → editor, BMP → visor, HTML → navegador, `.sh` → se ejecuta, `.exe` → la terminal dice qué es |
| Retroceso / Alt+↑ | subir una carpeta · Alt+← atrás |
| F7 o Ctrl+Shift+N / F6 | nueva carpeta / nuevo archivo de texto |
| F2 | renombrar |
| Ctrl+E · Ctrl+A | seleccionar todo |
| Shift+↑↓ (y Inicio/Fin/RePág/AvPág) · Ctrl+clic · Shift+clic | elegir varios |
| Ctrl+C / Ctrl+X / Ctrl+V | copiar / cortar / pegar (también varios y carpetas enteras) |
| Supr | a la Papelera (adentro de la Papelera: borrar definitivo, con confirmación) |
| RESTAURAR (en la Papelera) | vuelve a la carpeta de donde vino |
| Clic en NOMBRE / TAMAÑO / MODIFICADO | ordenar por esa columna |
| Letras | ir al primer elemento que empieza así |
| F5 | recargar |

| Navegador | |
|---|---|
| Ctrl+L / Alt+D / F6 / clic en la barra | escribir una dirección o una búsqueda; Enter va |
| Tab / Shift+Tab, Enter | recorrer enlaces y campos de formularios; abrir o enviar (o clic) |
| Alt+← / Retroceso · Alt+→ | atrás · adelante |
| F5 / Ctrl+R · Ctrl+H | recargar · inicio |
| F9 | modo lectura (solo el contenido) |
| Clic en un enlace `#sección` | baja hasta esa parte de la página |
| Flechas, RePág, AvPág, Espacio, rueda | moverse por la página |

| Terminal | |
|---|---|
| Tab | completar comandos y rutas (dos opciones o más: las muestra) |
| ↑ / ↓ | historial |
| Ctrl+C | cancelar la línea o el comando en curso (una descarga, `sleep`) |
| Ctrl+L · Ctrl+A / Ctrl+E · Ctrl+U / Ctrl+K · Ctrl+W | limpiar · inicio / fin · borrar hasta el inicio / el fin · borrar la palabra |
| RePág / AvPág, rueda | moverse por lo que ya salió |
| Ctrl+D (línea vacía), `exit` | cerrar la terminal |

| Editor | Consola JARVIS | Música |
|---|---|---|
| Ctrl+S guarda (si es nuevo, pide la ruta) | `ayuda` lista las órdenes | ↑ ↓ elegir, Enter reproducir |
| Ctrl+Inicio / Ctrl+Fin | `abrir`, `ir`, `buscar`, `ls`, `cat`, `editar` | Esc detener |
| Cerrar con cambios avisa una vez | `hora`, `estado`, `red`, `captura`, `apagar` | |

### La terminal

```
roman@jarvis:~$ ls -l Documentos | grep txt > lista.txt && cat lista.txt
roman@jarvis:~$ cd Proyectos; cp -r ../Documentos respaldo; tree
roman@jarvis:~$ wget https://ejemplo.com/archivo.zip       # descarga a la carpeta actual
roman@jarvis:~$ find / -name "*.bmp" | wc -l
roman@jarvis:~$ apt update && apt install neofetch && neofetch
```

- **Comandos**: `ls cd pwd cat head tail wc grep sed sort uniq cut tr rev tee seq expr echo
  printf find tree du df free stat file xxd strings touch mkdir rmdir rm cp mv ps kill top uname
  whoami hostname id date cal uptime ip ping curl wget apt dpkg snap winget ufw history alias
  export env which test sleep sh open nano jarvis screenshot reboot shutdown exit` y más (`help`,
  `man <comando>`).
- **Shell**: `|`, `>`, `>>`, `<`, `2>`, `2>&1`, `&&`, `||`, `;`, `'…'`, `"…$VAR…"`, `$(…)`, `$?`,
  `*.txt`, `~`, alias (`ll`, `la`, `dir`, `cls`, `ipconfig`…), `VAR=valor`.
- **`/proc`**: `cpuinfo`, `meminfo`, `uptime`, `version`, `loadavg`.
- **`rm` no borra para siempre**: manda a la Papelera, como la app Archivos.
- **Programas propios**: un archivo en `/Programas/bin` con comandos (uno por línea) es un
  programa: `nano /Programas/bin/saludo`, escribí `echo Hola, $1` y después `saludo Roman`.

### Paquetes: `apt`

| Orden | Qué hace |
|---|---|
| `apt update` | baja la lista de paquetes del repositorio |
| `apt list` (`--installed`, `--upgradable`) · `apt search x` · `apt show x` | ver qué hay |
| `apt install x y` | instala (con dependencias) |
| `apt remove x` | desinstala: los archivos van a la Papelera |
| `apt upgrade` | actualiza lo instalado |

El repositorio es la carpeta `kernel/paquetes/` (lo sirve el puente en `http://paquetes.jarvis/`):
`indice.txt` y un `manifiesto.txt` por paquete. Para agregar un paquete: una carpeta con sus
archivos y su manifiesto (`origen -> /destino`, `[bmp]` si es una imagen a convertir), y un
renglón en el índice. Paquetes de ejemplo: `neofetch`, `cowsay`, `fortune`, `sl`, `calc`, `hola`,
`fondos`, `manual` y `esenciales` (instala varios de una vez).

### `snap`

```
roman@jarvis:~$ snap find                     # la tienda de JARVIS-OS
roman@jarvis:~$ snap find vlc                 # también busca en la tienda real (Snapcraft)
roman@jarvis:~$ snap install saludo && saludo Ana
roman@jarvis:~$ snap refresh saludo --beta    # al canal beta (revisión 5)
roman@jarvis:~$ snap revert saludo            # vuelve a la revisión 3
roman@jarvis:~$ snap list · snap info saludo · snap remove saludo
roman@jarvis:~$ snap download hello-world     # un .snap de Linux a /Descargas (no se ejecuta)
```

Cada revisión queda en `/snap/<nombre>/<revisión>/`; `/snap/bin` está en el `PATH`. La tienda es
`kernel/paquetes/snaps/`: `indice.txt` (`nombre|versión|revisión|canal|editor|resumen|tamaño|comando`)
y una carpeta por revisión con su `manifiesto.txt`. Snaps de ejemplo: `saludo` (con canal beta),
`notas`, `dado` y `reloj`.

### `winget` (programas de Windows)

```
roman@jarvis:~$ winget search zip
roman@jarvis:~$ winget show 7zip.7zip          # del repositorio oficial de Microsoft (GitHub)
roman@jarvis:~$ winget install 7zip.7zip       # baja el instalador de 64 bits a /Descargas
roman@jarvis:~$ file /Descargas/7z2408-x64.exe
```

`winget search` busca en `kernel/paquetes/winget.txt`; con el Id exacto, `show` e `install` van
al repositorio real. Los instaladores no se ejecutan (ADR 0005); hasta 32 MB por descarga.

### Firewall: `ufw`

```
roman@jarvis:~$ sudo ufw deny out to tiktok.com          # también sus subdominios
roman@jarvis:~$ sudo ufw deny out port 80 app navegador  # el navegador, sin http://
roman@jarvis:~$ sudo ufw default deny outgoing && sudo ufw allow out to wikipedia.org
roman@jarvis:~$ ufw status numbered · ufw delete 1 · ufw show blocked · ufw app list
```

Las reglas se evalúan en orden (gana la primera). Las apps que se pueden nombrar: `navegador`,
`terminal`, `apt`, `snap`, `winget`, `configuracion`, `jarvis` y `sistema`. Lo mismo se maneja
desde Configuración → Firewall. Lo que entra ya está cerrado: JARVIS-OS no escucha en ningún
puerto.

## Arquitectura

```
firmware UEFI (OVMF en QEMU)
  └─ bootloader (crate bootloader 0.11): modo 64 bits, tablas de páginas iniciales, framebuffer,
     mapeo de toda la memoria física (el kernel lo reemplaza por el suyo, K8)
       └─ kernel_main (kernel/kernel/src/main.rs) — solo hardware
            ├─ serial.rs      COM1: logs al host
            ├─ gdt.rs         GDT + TSS (stack de emergencia para el doble fallo)
            ├─ interrupts.rs  IDT: excepciones, timer (IRQ0), teclado (IRQ1), mouse (IRQ12),
            │                 líneas PCI del disco y la red (compartibles)
            ├─ pit.rs         PIT: el tick del planificador (250 Hz) y calibra el TSC
            ├─ task.rs        tareas (K9): cambio de contexto, pilas con guarda, esperas, desalojo
            ├─ irqlock.rs     lock sin interrupciones para lo que comparten las tareas
            ├─ nettask.rs     la tarea de la red: colas de pedidos y respuestas con el escritorio
            ├─ entropy.rs     entropía (K10): RDSEED/RDRAND y variación del TSC → generador global
            ├─ tls.rs         TLS (K10): une jarvis-tls con la entropía y la hora; prueba al arrancar
            ├─ audio.rs       la tarea "audio" (K12): del anillo que llena el escritorio a la placa
            ├─ syscall.rs     syscall/sysret (K11): MSR, la entrada, SSE y el salto al anillo 3
            ├─ process.rs     procesos (K11): crear, llamadas, fallos de página, terminar
            ├─ tareas "proceso" (anillo 3): un programa de Linux cada una, con su PML4
            │    └─ jarvis-linux   ELF, pila inicial, memoria y llamadas al sistema de Linux
            ├─ time.rs        reloj en ms con el TSC
            ├─ queue.rs       cola de bytes sin locks (interrupción → bucle)
            ├─ keyboard.rs    teclado PS/2 → teclas y modificadores (Alt, Ctrl, Win, AltGr),
            │                 distribución EE. UU. o latinoamericana (desktop/keymap.rs)
            ├─ mouse.rs       mouse PS/2 con rueda (puerto auxiliar del 8042)
            ├─ allocator.rs   heap de 256 MiB (alloc: Vec, String)
            ├─ paging.rs      tablas de páginas propias (jarvis-mem), W^X, map_mmio
            ├─ pci.rs         enumeración del bus PCI
            ├─ virtio_blk.rs  driver de disco virtio-blk (DMA, virtqueue, interrupción)
            ├─ virtio_net.rs  driver de placa de red virtio-net (dos virtqueues, interrupción)
            ├─ speaker.rs     parlante de la PC (canal 2 del PIT)
            ├─ power.rs       apagar (\_S5 de ACPI) y reiniciar (registro de reset de la FADT, 8042)
            ├─ cpu.rs         nombre de la CPU (cpuid) y temperatura (DTS de Intel, Tctl de AMD,
            │                 zonas térmicas de ACPI)
            ├─ acpi.rs        K13: tablas del firmware y el AML (crate acpi): _PRT, \_S5, _TMP
            ├─ apic.rs        K13: APIC local, IOAPIC y MSI (el PIC queda de respaldo)
            ├─ storage.rs     K13: discos reales (ahci.rs, nvme.rs, pendrives por xhci.rs) y la
            │                 partición de JARVIS de su GPT
            ├─ nic.rs         K13: virtio-net o una placa real (e1000.rs, rtl8139.rs, rtl8169.rs)
            ├─ xhci.rs        K13: USB 3 (teclado, mouse, pendrives, hubs), con su tarea "usb"
            ├─ hda.rs         K13: la placa de sonido de las PC (Intel HDA y su codec)
            ├─ rtw88.rs       K14: la placa Wi-Fi RTL8821CE (anillos de DMA de 32 bits, MSI)
            ├─ wifi.rs        K14: la placa + la estación de jarvis-wifi, como una placa de red
            ├─ installer.rs   K13: copia el arranque a un disco vacío y crea la partición de datos
            ├─ bootlog.rs     K13: el registro del arranque (en pantalla y en /Sistema/arranque.log)
            ├─ hw.rs          K13: la lista de dispositivos para Configuración → Hardware
            ├─ rtc.rs         reloj CMOS → fecha y hora
            ├─ tarea "red"    (prioridad alta) interrupción o pedido → smoltcp → respuestas
            │    └─ jarvis-net      TCP/IP (smoltcp), DHCP, DNS, descargas HTTP
            ├─ tarea "ociosa" hlt cuando nadie más puede correr
            └─ tarea "escritorio" (la del arranque): dormir hasta el cuadro → eventos → render →
                 │               present → pedidos a la red → estadísticas
                 └─ jarvis-desktop  ventanas, atajos, apps, barra, panel, menús, configuración,
                    │               firewall, idiomas, terminal (jsh, apt, snap, winget, ufw),
                    │               web (DOM, CSS, estilos, maquetación en cajas, HTTP)
                      ├─ jarvis-fs   FAT32 sobre el disco (con caché de sectores)
                      ├─ jarvis-image PNG (propio, con inflate) y JPEG (zune-jpeg) → RGBA
                      ├─ jarvis-audio WAV, IMA ADPCM, remuestreo, mezclador, sintetizador, AVI
                      └─ jarvis-gfx  dibujo: HUD, esfera, texto, fuente vectorial, fuente de las
                                     páginas (DejaVu + fontdue), figuras
```

| Crate | Qué es | Cómo se prueba |
|---|---|---|
| `gfx` (`jarvis-gfx`) | Dibujo: canvas con recorte (anidado) y `blit`, paleta, texto, **fuente vectorial**, **fuente proporcional de las páginas** (DejaVu, cualquier tamaño), figuras, íconos, trigonometría en punto fijo, esfera, asistente, HUD. | `cargo test`: 39 tests |
| `fs` (`jarvis-fs`) | FAT32 propio: montaje, FAT (dos copias), nombres largos, lectura, escritura, carpetas, renombrar, mover, **copiar**, borrar, **caché de sectores**. Sobre un trait `BlockDevice`. | 22 tests, 14 de ellos **cruzados contra `fatfs`**: cada uno lee lo que escribe el otro, y el espacio libre se cuenta sobre la FAT cruda |
| `desktop` (`jarvis-desktop`) | Escritorio: gestor de ventanas (con escritorios virtuales), atajos, barra, panel de estado, menús y paneles, configuración, firewall, idiomas, composición; apps (Archivos, Terminal, Configuración, Monitor, Consola, Editor, Música, Visor, Navegador); shell `jsh`, `apt`, `snap`, `winget`, `ufw`, formatos PE/ELF/squashfs; web: URL, HTTP, DOM, selectores y cascada, maquetación en cajas (flujo, flotantes, flex, grid, tablas), JSON, adaptador de YouTube; teclado latinoamericano. | 116 tests: el escritorio manejado con teclas y clics sobre un disco en memoria, verificado con `fatfs`; la terminal, `apt`, `snap` y `winget` contra el repositorio real y respuestas grabadas; el firewall; la maquetación sobre HTML de prueba; incluye "render incremental == redibujar todo". Más `vista_previa` (a mano): arma una página real, con imágenes, y la guarda en BMP |
| `net` (`jarvis-net`) | Red: smoltcp, DHCP, DNS (con respaldo), descargas HTTP con redirecciones, HTTPS por el puente, conexiones TCP largas. | 6 tests de punta a punta en memoria (placa "loopback" + servidores de juguete), incluido `poll_delay` |
| `audio` (`jarvis-audio`) | Audio y video (K12), todo entero (sin punto flotante): WAV (PCM de 8/16/24 bits) e IMA ADPCM propios, remuestreo a la frecuencia de la placa, mezclador con volumen y el nivel de la voz por momento, sintetizador para las partituras y el contenedor AVI (MJPEG + audio). | 17 tests, dos **cruzados** contra el generador en Python de `paquetes/` (el ADPCM coincide muestra a muestra; el AVI y sus JPEG los armó Pillow), y `cargo xtask test` (una canción y un video suenan en la placa virtio-sound de QEMU) |
| `image` (`jarvis-image`) | Imágenes (K10): inflate (DEFLATE + zlib) y PNG propios (todos los tipos de color y profundidades, paletas, `tRNS`, Adam7, CRC y Adler-32), JPEG con `zune-jpeg` (progresivos incluidos), topes de tamaño y achicado por promedio. | 13 tests, los de PNG **cruzados contra la crate `png`** (todas las combinaciones de color, bits y filtro; entrelazado; cortado en cada byte) y los de JPEG contra `image`; y `cargo xtask test` (el visor abre un PNG y un JPEG progresivo en el kernel) |
| `mem` (`jarvis-mem`) | Memoria: allocator de marcos físicos (mapa de bits), tablas de páginas de 4 niveles (mapear, traducir, desmapear, recorrer; páginas de 4 KiB, 2 MiB y 1 GiB) y segmentos del ELF del kernel para W^X. Sobre un trait `PhysMem`. | 8 tests sobre una RAM de mentira (copiar una jerarquía da las mismas traducciones) y `cargo xtask test` (el kernel arranca con sus tablas y verifica W^X) |
| `task` (`jarvis-task`) | Multitarea: el planificador (prioridades, ronda con turno de 10 ms, esperas por evento o plazo, avisos que llegan antes de esperar, tiempo de CPU por tarea) en una tabla fija. | 12 tests (turnos, desalojo, "lost wakeup", plazos) y `cargo xtask test` (tres tareas, disco y red por interrupción) |
| `linux` (`jarvis-linux`) | La ABI de Linux x86_64 (K11): cargador de ELF (estáticos y static-pie), pila inicial con el vector auxiliar, zonas de memoria (brk, mmap, mprotect, páginas al primer uso) y ~90 llamadas al sistema (archivos, directorios, consola, tiempo, azar, sockets TCP, señales mínimas). Sobre un trait `System`. | 20 tests: un proceso de mentira de punta a punta (cargar, archivos, directorios, memoria, punteros del kernel → EFAULT, sockets) y `cargo xtask test` (programas de verdad, compilados con musl) |
| `drivers` (`jarvis-drivers`) | Hardware real (K13, ADR 0011), la mitad que interpreta: tablas fijas de ACPI, entradas del IOAPIC y mensajes MSI, GPT (leer, crear, disco vacío), comandos AHCI/ATA y NVMe, descriptores de e1000, RTL8139 y RTL8168, USB (descriptores, TRB y contextos de xHCI, HID, Bulk-Only + SCSI), verbos y grafo de HDA, sensores de temperatura; la placa Wi-Fi RTL8821CE (K14, `rtw88`: encendido, efuse, carga del firmware, comandos H2C, tablas de Realtek con condiciones, canales, potencia, descriptores) y el ramdisk del arranque. Sin `unsafe`. | Tests con tablas y descriptores armados byte por byte; la GPT **cruzada contra la crate `gpt`** en los dos sentidos; la RTL8821CE contra una **placa simulada** (bits que se borran solos, el DMA interno, el efuse, la radio, el firmware que arranca). En QEMU, `cargo xtask test-hardware` (AHCI + e1000e + HDA, NVMe + RTL8139, todo por USB) y `cargo xtask test-instalar` |
| `wifi` (`jarvis-wifi`) | Wi-Fi (K14, ADR 0012), lo que no toca la placa: tramas 802.11 (encabezado, datos ↔ Ethernet, beacons, sondeo, autenticación, asociación), elemento RSN, PMK y PTK, el saludo de 4 vías y el de grupo de la estación, CCMP con contadores contra repeticiones, y la estación entera (sin E/S): buscar redes, conectarse, datos, pérdida de beacons y reintentos. | 20 tests: vectores del estándar (PBKDF2 del anexo J, AES Key Wrap del RFC 3394), 6 **cruzados** contra un punto de acceso en Python con `cryptography` (`wifi/tests/datos/generar.py`: los mensajes 2 y 4 y las tramas cifradas coinciden byte a byte) y 8 de la estación contra puntos de acceso de mentira en un aire simulado |
| `kernel` (`jarvis-kernel`) | El binario sin sistema operativo debajo. Solo hardware → eventos, bloques y píxeles. | `cargo xtask test` en QEMU |
| `xtask` | Imagen booteable, disco FAT32, QEMU (serie + monitor + red + audio), puente (HTTPS, repositorio de paquetes, conversión de imágenes y SVG a BMP con transparencia), puente de Brave (DevTools → mosaicos LZ4), test de punta a punta, capturas. | 2 tests (el puente no sale de su carpeta; PNG y SVG → BMP) y `cargo xtask test` |

## Lo que se aprendió (y por qué el código es así)

### K14: Wi-Fi

- **Una placa Wi-Fi no es una placa de red con antena.** Una Ethernet manda las tramas que le
  dan. Una SoftMAC como la RTL8821CE solo hace la radio: buscar redes, asociarse y cifrar lo
  hace el sistema, con tramas 802.11 que tienen hasta cuatro direcciones (cuál es cuál depende de
  las banderas ToDS/FromDS) y un encabezado LLC/SNAP para llevar el ethertype.
- **La contraseña nunca viaja.** De ella sale la PMK (PBKDF2, 4096 vueltas: lento a propósito);
  de la PMK, las MAC y dos números al azar sale la PTK. Cada lado demuestra que sabe la
  contraseña firmando un mensaje con su parte de la PTK (la KCK). La clave de grupo viaja
  cifrada con otra parte (la KEK, con AES Key Wrap).
- **Lo que protege contra repeticiones se avanza recién con la firma bien.** El contador del
  saludo y el PN de CCMP solo suben después de verificar el MIC: si no, una trama falsa con un
  número enorme dejaría afuera a todas las verdaderas. Y el mensaje 1, que no va firmado, no
  mueve el contador.
- **El encabezado se autentica, pero no entero.** CCMP firma el encabezado MAC con los bits que
  cambian en el camino en cero (reintento, ahorro de energía, el número de secuencia): si no,
  una retransmisión no pasaría el MIC.
- **Probar contra otra implementación encuentra lo que un test propio no.** Un test que cifra y
  descifra con el mismo código pasa aunque los dos lados entiendan mal el mismo campo. Por eso
  el punto de acceso de los tests está escrito en Python con otra biblioteca.
- **La placa corre un programa propio.** La RTL8821CE tiene un procesador adentro y no hace
  nada sin su firmware. Se carga raro: cada pedazo de 4 KiB se manda por la *cola de beacons*
  a una "página reservada" de la memoria de la placa, y un DMA interno lo copia a su memoria de
  código o de datos verificando una suma. Después el sistema le habla por mensajes (H2C) en
  cuatro buzones de registros o en paquetes.
- **Las tablas de Realtek son un pequeño programa.** Miles de pares (registro, valor) con
  `if`/`elif`/`else` según el tipo de placa: la misma tabla sirve para todas las variantes del
  chip. Se generan del código de Linux (BSD-3-Clause) con un script, no a mano.
- **Un driver sin el hardware se prueba contra un simulador.** QEMU no tiene placas Wi-Fi. La
  mitad que interpreta corre contra una placa de mentira que hace lo que el driver espera del
  silicio (bits que se borran solos, el DMA interno, el efuse) y la estación contra puntos de
  acceso de mentira con su propio autenticador. Lo que eso no cubre (los valores analógicos, la
  radio de verdad) se ve en la PC, en el registro del arranque.
- **32 bits todavía importan.** La placa solo ve direcciones de 32 bits, y en una PC con 16 GiB
  el heap (la región de RAM más grande) queda arriba de 4 GiB. El kernel aparta 4 MiB abajo de
  4 GiB antes de armar la paginación (`dma::alloc32`).
- **Un firmware que no se puede redistribuir modificado no va en el repositorio.** `cargo xtask`
  lo baja de linux-firmware con un hash fijo, lo valida con el mismo parser del driver y lo pone
  en el *ramdisk* que el bootloader carga con el kernel: así viaja en la partición de arranque y
  llega solo a la ISO, al pendrive y al disco instalado.
- **Con radar no se habla primero.** En los canales de 5 GHz que comparten banda con radares
  (52–144), una estación no puede mandar un pedido de sondeo: solo escucha beacons.

### K13: hardware real

- **Un driver son dos mitades.** Una habla con el hardware (registros, DMA, interrupciones) y va
  en el binario del kernel; la otra interpreta (arma un comando, recorre una tabla, decide qué
  hacer con un anillo) y va en `jarvis-drivers`, sin `unsafe`, probada en el host. Así se
  escribieron drivers para placas que QEMU no emula (el RTL8168 de la PC de Roman, el codec
  Realtek de su HDA): la parte fácil de equivocarse se prueba con datos armados a mano.
- **ACPI: tablas y un lenguaje.** Las tablas fijas (MADT, FADT, MCFG, HPET) son estructuras: se
  leen con un parser propio. El AML de la DSDT es un programa con métodos y variables, y para él
  se usa la crate `acpi`. De ahí sale a qué entrada del IOAPIC va cada línea PCI (`_PRT`), cómo
  apagar (`\_S5` + PM1_CNT) y las zonas térmicas (`_TMP`, en décimas de kelvin).
- **Del PIC al APIC, y MSI.** En una PC con UEFI la línea INTx de un dispositivo no tiene un
  número confiable para el PIC. Con la MADT, el kernel pasa al APIC local y los IOAPIC, y los
  drivers nuevos piden **MSI**: el dispositivo avisa escribiendo en una dirección de memoria, con
  su propio vector, sin líneas compartidas.
- **El disco del sistema es una partición.** En una PC el disco tiene GPT; JARVIS-OS monta solo
  la partición con su GUID de tipo y nunca toca las demás (el SSD con Windows ni aparece como
  destino del instalador). Un disco sin GPT se monta entero, como el `disco.img` de siempre.
- **El instalador copia sectores, no archivos.** La partición de arranque que arma el
  bootloader es FAT16 (jarvis-fs solo entiende FAT32): se copia entera, sector por sector, y
  después se escriben el MBR protector, la GPT y la partición de datos. La tabla se escribe
  **después** de la copia, así un error a la mitad deja el disco "vacío" para reintentar, y se
  vuelve a verificar que esté vacío justo antes de escribir.
- **Un bug de lectura desordenada.** El anillo de eventos de xHCI se leía de a 16 bytes de una
  vez; con transferencias largas llegaba un evento "bien" con el puntero de la vuelta anterior
  (0) y la lectura quedaba sin respuesta. La controladora escribe la palabra del ciclo **al
  final**: hay que leerla primero, poner una barrera y recién después leer el resto.
- **Temperatura en cada fabricante.** Intel la da en un MSR (el DTS); los Ryzen, en el registro
  Tctl, al que se llega por el SMN a través de dos registros de configuración del complejo raíz
  PCI (como `k10temp` de Linux); si no, quedan las zonas térmicas de ACPI (que muchas placas de
  escritorio no declaran). En QEMU no hay ninguno: la temperatura queda vacía.
- **Sin puerto serie.** En la PC no hay dónde ver el serie: todo lo que el kernel escribe hasta
  el escritorio se dibuja abajo en la pantalla, se guarda en `/Sistema/arranque.log` y, si hay un
  panic, sus últimas líneas quedan debajo del error.
- **Lo que no se puede probar acá.** El RTL8168 y el codec real de HDA siguen el datasheet y el
  driver de Linux: quedan verificados recién cuando corran en la PC.
- **Qué quedó afuera: la suspensión S3.** Dormir en RAM es fácil (`\_S3` + PM1_CNT); despertar
  no: la CPU vuelve en modo real por el vector de la FACS, y **todos** los dispositivos vuelven
  reseteados. Además de un trampolín de 16 a 64 bits y de reprogramar el APIC, el IOAPIC, los
  MSI y cada driver, hace falta la **placa de video**: el firmware no la reinicia al despertar, y
  en la APU de la PC de Roman eso es un driver nativo de GPU (como `amdgpu`), que JARVIS-OS no
  tiene. Sin él, S3 despierta con la pantalla negra. Suspender sigue siendo la pantalla negra
  con la CPU en `hlt` (K6), y S3 queda para cuando haya un driver de video.

### K12: audio y video

- **La placa de sonido manda el tiempo.** virtio-sound devuelve cada buffer de salida cuando lo
  terminó de consumir: ese es el reloj más confiable que hay. Todo se mide con él: cuánto va de
  una canción, qué cuadro de un video se muestra (el video sigue a su audio: si un cuadro tarda
  en dibujarse, se saltea, no se atrasa) y el nivel de la voz que mueve la esfera.
- **Mezclar por adelantado, reproducir aparte.** El mezclador vive en el escritorio (lo usan las
  apps y la voz), pero un cuadro lento no puede cortar el sonido. El escritorio deja ~120 ms de
  audio mezclado en un anillo y una tarea "audio" de prioridad alta lo pasa a la placa cada 5 ms.
  Como lo que se mezcla va adelantado a lo que suena, el mezclador anota el nivel de la voz de
  cada bloque de 20 ms con el cuadro en que empieza, y la esfera pregunta por el cuadro que la
  placa está reproduciendo **ahora**.
- **Un bug del planificador que K9 no mostraba.** La ronda de turnos arrancaba desde la tarea que
  acababa de correr. Con el audio despertando cada 5 ms (más seguido que el turno de 10 ms), la
  ronda siempre arrancaba después del audio y le tocaba a la misma tarea de abajo: un programa
  que calcula sin parar dejaba al escritorio sin CPU. Ahora cada prioridad recuerda por dónde iba
  su ronda (el test lo reproduce: con el planificador viejo, el escritorio corría 0 veces).
- **Códecs enteros.** El kernel no tiene SSE: el punto flotante es por software. MP3 y Vorbis
  decodifican con `float` y no llegarían a tiempo real, así que se eligieron códecs de
  aritmética entera: **PCM** (no hay nada que decodificar) e **IMA ADPCM**, que predice cada
  muestra con la anterior y guarda la diferencia en 4 bits, con un paso que se adapta (un cuarto
  del tamaño, calidad de radio). El video es **MJPEG**: cada cuadro es un JPEG (el decodificador
  de K10, IDCT entera) y ninguno depende de otro, así que no hace falta un decodificador de
  video "de verdad" (H.264 predice cada cuadro a partir de otros).
- **Remuestrear** es preguntarse cuánto vale la onda en instantes donde no hay muestras. Se
  interpola en línea recta con la posición en punto fijo (16 bits de fracción): 22 050 Hz mono
  → 48 000 Hz estéreo. Al sumar señales de 16 bits se suma en 32 y se recorta.
- **La voz de JARVIS, ahora en JARVIS-OS.** El kernel le dice al cerebro en `hola` que tiene
  parlantes (`parlantes: 48000`); entonces la voz sintetizada viaja como audio (`audio{tasa,
  pcm}`, en base64) en vez de sonar en el anfitrión. El cerebro la manda **al ritmo en que
  suena** (con 300 ms de ventaja): así "callar" corta enseguida y el micrófono sabe cuándo está
  hablando para no escucharse.
- **Qué queda**: HDA (la placa de sonido de las PC reales, para K13), un filtro mejor para
  remuestrear, códecs con punto flotante (MP3, Vorbis, Opus) y video con compresión entre
  cuadros (H.264).

### K11: espacio de usuario

- **Un proceso es una tarea con otra PML4.** Los primeros 512 GiB (la entrada 0 de la PML4) son
  del programa; las otras 511 entradas se copian de la PML4 del kernel, así el kernel está en
  todos los espacios. Truco necesario: al arrancar se crean **todas** esas entradas (511 tablas
  vacías, 2 MiB): si el kernel agregara una después, los procesos que ya existen no la verían.
  El bootloader dejaba en la entrada 0 el código con el que salta al kernel (mapeado
  "identidad", virtual = física): ya no se usa y se descarta.
- **Entrar y salir del anillo 3.** La primera vez se "vuelve" de una interrupción que nunca
  pasó: `iretq` con los selectores de usuario (RPL 3). Después, el programa entra al kernel con
  `syscall`, que salta a LSTAR **sin cambiar de pila**: lo primero es pasar a la pila del kernel
  de esa tarea. Las interrupciones que llegan con el programa corriendo usan `rsp0` de la TSS.
  Las dos cosas cambian con cada tarea, igual que CR3, FS (el TLS del programa, `arch_prctl`) y
  los registros XMM (`fxsave`/`fxrstor`: el kernel no usa SSE, los programas sí). El orden de la
  GDT no es libre: `sysret` calcula los selectores sumando 8 y 16 al de STAR.
- **Nunca creerle a un puntero del programa.** `read(fd, buf, n)` con `buf` apuntando al kernel
  sería una forma de pisarlo. El kernel no usa esos punteros: recorre las tablas del proceso,
  exige el bit USER (y WRITABLE para escribir) y copia por el mapeo de la RAM. Si la página no
  está todavía pero es de una zona válida, se asigna y se reintenta; si no, `EFAULT`.
- **Memoria al primer uso.** `mmap` de 1 GiB o una pila de 8 MiB no gastan nada: solo se anota la
  zona. La página aparece cuando el programa la toca (fallo de página → ¿es de una zona? → una
  página en cero). El malloc de musl me enseñó que las zonas se superponen de formas
  inesperadas: pone una página de guarda (`mmap` PROT_NONE fijo) en el medio de su heap, y un
  `brk` que rehacía toda la zona del heap la pisaba (el programa moría en la asignación 20000).
- **Los procesos no tocan el disco.** El FAT32 y la Terminal son de la tarea del escritorio: un
  `open` deja un pedido en una cola, despierta al escritorio (`EV_PROC`) y espera la respuesta,
  como un microkernel con su servidor de archivos. Un archivo se lee entero al abrirlo y se
  escribe entero al cerrarlo; `unlink` lo manda a la Papelera. El escritorio dejó de dormir
  "hasta el próximo cuadro": duerme hasta el cuadro **o** hasta que un programa pida algo.
- **Un programa que se porta mal termina él, no el sistema.** Un fallo de página inválido, una
  instrucción ilegal o una división por cero en el anillo 3 terminan el proceso con la señal de
  Linux (139 = SIGSEGV, como en bash). Ctrl+C marca al proceso; se termina en su próxima
  llamada, espera o **tick del timer** (así también se corta un bucle que no llama al sistema).
- **Programas de verdad, sin compilador de C.** `kernel/usuario/` se compila para
  `x86_64-unknown-linux-musl` con `rust-lld` y los objetos de musl que trae Rust: estáticos
  *static-pie* con la biblioteca estándar entera. `std::fs`, `println!`, `Vec`, `f64` y
  `TcpStream` funcionan sin tocar una línea. Para saber qué llamadas hacían falta alcanzó con
  correrlos: lo que no existe devuelve `ENOSYS` y se anota en el log.
- **Sockets con firewall.** `connect` es una conexión larga del `Outbox` (ADR 0007) a nombre de la
  app `programas`: pasa por las reglas de `ufw` como todo lo demás, y un bloqueo llega al
  programa como `EACCES` ("Permission denied").
- **Un intérprete de JavaScript, sin escribirlo.** `js` es el motor Boa (Rust) compilado como
  cualquier otro programa de Linux: 5,7 MB, se instala con `apt install js` y corre código
  suelto (`js -e`), archivos y una consola interactiva cuya entrada es la Terminal. No pidió
  ninguna llamada al sistema nueva: esa es la gracia de implementar la ABI de Linux en vez de
  una propia. Se compila sin Temporal ni Intl (los datos de zonas horarias e idiomas pesan
  megas).
- **Qué queda para más adelante** (ADR 0010): bibliotecas dinámicas (`ld.so`), hilos (`clone` y
  `futex` de verdad), `fork`/`exec` desde un programa, señales entregadas al programa, `pipe`,
  UDP (y con eso el DNS de musl: hoy `connect` necesita una IP) y programas de Windows. Brave
  nativo necesita todo eso más un servidor gráfico.

### K10: TLS y decodificadores en el kernel

- **Primero, el azar.** TLS entero se apoya en claves efímeras impredecibles: si el generador es
  malo, el cifrado más fuerte no sirve (le pasó a Debian con OpenSSL en 2008). `jarvis_tls::rng`
  separa dos cosas: un **acumulador** (`Pool`, SHA-256) que junta entropía y cuenta los bits que se
  le acreditan, y un **generador** (`Rng`, ChaCha20) que estira la semilla. El generador usa
  *borrado rápido de la clave*: cada pedido genera 32 bytes de más que reemplazan la clave, así
  que quien lea la memoria después no puede reconstruir lo que ya salió.
- **Las fuentes** (`entropy.rs`): RDSEED y RDRAND si la CPU los tiene (se les cree la mitad y un
  cuarto: nunca se depende solo de la CPU) y la **variación del TSC** al repetir un trabajo corto.
  QEMU (`qemu64`) no tiene RDRAND, así que la variación del TSC tiene que alcanzar sola; se
  acredita como mucho 1 bit por medición, y solo si ni la medición ni su variación repiten la
  anterior (el "stuck test" de jitterentropy, de Linux). Con WHPX alcanza en dos vueltas (~270
  bits). En QEMU sin aceleración (TCG) casi todo se repite: ahí el log dice
  `ENTROPIA_INSUFICIENTE` y TLS no se va a usar. Pendiente: un driver virtio-rng (QEMU le pasa
  entropía del anfitrión) como fuente más.
- **El cliente TLS** (`jarvis_tls::client`) no hace entrada/salida: recibe los bytes que llegan
  por TCP y devuelve los que hay que mandar ("sans-I/O"). Así los tests lo hacen hablar con un
  servidor de referencia (rustls con *ring*, otra implementación de la criptografía) en memoria,
  y en el kernel se va a sentar sobre un socket de smoltcp. Por dentro usa la API *unbuffered*
  de rustls, la única sin `std`: rustls dice en qué estado está (hay que mandar algo, llegaron
  datos, se puede escribir, falta leer) y el cliente reacciona hasta que no queda nada por hacer.
- **El proveedor propio** (`tls/src/provider/`): rustls hace el protocolo y le pide la matemática
  a un `CryptoProvider`. Lo más delicado fue AES-GCM en TLS 1.2: el nonce son 4 bytes fijos (del
  bloque de claves) + 8 que viajan al principio de cada registro; en TLS 1.3 y con ChaCha20 no
  viaja nada (IV XOR número de secuencia). La verificación de firmas se prueba con un
  certificado "impostor": su emisor tiene el mismo nombre que la autoridad de confianza pero
  otra clave, así que solo la firma lo delata (y el test falla si la verificación de ECDSA
  acepta cualquier cosa: se probó rompiéndola a propósito).
- **Sin SSE**: el kernel compila para un target sin SSE ni AVX, y los backends rápidos de AES
  (AES-NI), GCM, SHA-2 (SHA-NI) y curve25519 no compilan ahí (LLVM falla). Se fuerzan las
  versiones por software con `--cfg` en `.cargo/config.toml` y, para SHA-2, un feature solo
  para ese target. Todo por software y aun así el ClientHello (dos pares de claves efímeras) sale
  en menos de 1 ms. El kernel creció ~0,9 MB (RSA, las curvas y las 121 raíces de Mozilla).
- **La hora**: un certificado se valida contra la fecha actual. El RTC se lee una vez al
  arrancar (`DateTime::unix_seconds`, el algoritmo *days_from_civil*) y después se suma el TSC:
  así la tarea de la red no toca los puertos del CMOS a la vez que el reloj del escritorio. Sin
  hora válida o sin entropía suficiente, TLS no arranca: es preferible a aceptar certificados
  vencidos o usar claves adivinables.
- **HTTPS directo** (`kernel/net`): el cliente TLS se sienta entre el socket de smoltcp y el
  HTTP. Lo que llega por TCP entra a `receive` y sale descifrado (`take_plaintext`) hacia la
  respuesta; lo que TLS quiere mandar (el ClientHello con el pedido ya encolado detrás, las
  claves, el Finished) se junta en un buffer de salida y se manda **en la misma vuelta**: el
  saludo son varias idas y vueltas, y esperar al siguiente `poll` las haría más lentas. El fin de
  la respuesta es el `close_notify` o el FIN de TCP (muchos servidores no mandan el primero; el
  HTTP dice su largo igual). Si TCP se corta antes de terminar el saludo, es un error. Las
  imágenes y los nombres `.jarvis` siguen yendo al puente (las convierte; el repositorio vive
  ahí), y el interruptor "HTTPS por el puente" de Configuración deja el camino viejo de
  respaldo. El handshake corre en la tarea de la red, cuya pila pasó a 512 KiB. Un certificado
  vencido o de una autoridad desconocida corta la descarga: la página de error lo dice y no hay
  "continuar de todos modos". Se prueba en memoria (`net/tests/https.rs`: un servidor rustls con
  *ring* sobre la placa loopback, con un certificado para 127.0.0.1) y contra sitios reales en
  `cargo xtask test` con `JARVIS_TEST_INTERNET=1` (example.org llega; expired.badssl.com no).
- **PNG propio** (`image/src/png.rs` e `inflate.rs`). DEFLATE son dos ideas apiladas: LZ77
  ("copiá 12 bytes de 300 atrás") y códigos de Huffman (lo frecuente, con menos bits). La
  sorpresa: los códigos se leen desde el bit menos significativo de cada byte, pero cada código
  va con su bit más alto primero; por eso la tabla rápida (códigos de hasta 9 bits de una sola
  consulta) se indexa con el código **invertido**, y los raros de más de 9 bits se decodifican
  bit a bit como en `puff.c`. Una copia puede pisarse a sí misma (distancia 1, largo 100 = repetir
  un byte 100 veces), así que se copia byte a byte. Encima de eso PNG filtra cada fila (la
  diferencia con el píxel de la izquierda, el de arriba, su promedio o el predictor de Paeth),
  que no comprime nada por sí mismo pero deja números chicos que DEFLATE aprovecha. Se sabe de
  antemano cuánto tienen que ocupar los píxeles descomprimidos: ese es el tope del inflate, y
  así un PNG de 1 KB que se descomprime en 4 GB (una "bomba") falla enseguida. Los chunks
  críticos verifican su CRC; los opcionales no, como hacen los navegadores.
- **JPEG con `zune-jpeg`**: sin `std` ni SIMD compila tal cual para el kernel. Se pide la salida
  directo en RGBA y con tope de tamaño (una cabecera de 20 bytes puede decir 65535 × 65535). Su
  IDCT es entera: el punto flotante por software no lo frena.
- **Qué va directo y qué al puente**: las imágenes PNG y JPEG se piden sin `X-Jarvis-Imagen` y
  las decodifica el navegador; las `.svg`, `.gif`, `.webp` e `.ico` van al puente de entrada. Como
  la dirección no siempre dice el formato (`/foto?id=3`), si lo que llega directo no es PNG,
  JPEG ni BMP (se mira la firma, no el `Content-Type`), se vuelve a pedir al puente. Las imágenes
  de la web se achican a 900 px de lado (como hacía el puente) promediando cajas de píxeles,
  pesando el color por la opacidad para que los bordes transparentes no se oscurezcan. El visor,
  los fondos de pantalla y `open` también abren PNG y JPEG.

### K9: multitarea
- **Una tarea es una pila y un `rsp` guardado**. Cambiar de tarea (`jarvis_switch`, 14
  instrucciones en `task.rs`) apila solo los registros que la convención de llamadas obliga a
  preservar (rbx, rbp, r12–r15), guarda `rsp`, carga el de la otra y hace `ret`: vuelve a donde
  *esa* tarea había llamado al cambio. Los demás registros ya los guardó el compilador en quien
  llamó. Como el kernel no usa SSE (punto flotante por software), no hay estado de FPU que
  guardar. Una tarea nueva arranca con una pila armada a mano cuyo `ret` cae en un trampolín.
- **Desalojo desde el timer**: el manejador de IRQ0 avisa "fin de interrupción" al PIC *antes* de
  cambiar de tarea. Si no, el PIC no manda otra interrupción del timer hasta que ese manejador
  termine, y eso pasa recién cuando la tarea interrumpida vuelve a correr. Cada tarea retoma
  dentro de su propio manejador y sale con `iretq` como si nada.
- **Locks y desalojo no se llevan bien**. Con un solo núcleo, si una tarea tiene un spinlock y
  el timer le saca la CPU, la siguiente que lo pida gira todo su turno; si lo pide una
  interrupción, se traba para siempre. Por eso el heap, el puerto serie, las colas entre
  tareas (`IrqMutex`) y el planificador mismo se toman **con las interrupciones
  deshabilitadas**: nadie puede desalojar a quien tiene el lock. El planificador usa una tabla
  fija, no `Vec`: corre dentro de la interrupción del timer y no puede pedir memoria.
- **El aviso que llega antes de esperar** ("lost wakeup"): el disco puede terminar entre que la
  tarea mira el anillo (todavía no) y se pone a esperar. Si el aviso se perdiera, la tarea
  dormiría para siempre. El planificador **anota** los eventos que nadie esperaba y la próxima
  espera vuelve enseguida. Además, toda espera de un driver tiene plazo y revisa el anillo: una
  interrupción perdida cuesta 20 ms, no un cuelgue.
- **Interrupciones PCI compartidas y "por nivel"**: el firmware eligió la línea de cada
  dispositivo (registro 0x3C: en QEMU, 11 para el disco y 10 para la red). Una línea puede ser de
  varios, así que el manejador le pregunta a cada uno leyendo su registro ISR, que además baja
  la línea: si no se lee, la interrupción vuelve apenas se avisa el fin. El video y el
  micrófono (virtio moderno, por polling) tienen apagada su interrupción (bit 10 del comando PCI)
  para que no disparen una línea que nadie atiende.
- **Paso de mensajes entre tareas**: el escritorio no toca la pila de red ni al revés. Se dejan
  pedidos y respuestas en dos colas y se despiertan con un evento. La red es **más prioritaria**
  (poco trabajo, urgente) pero cede la CPU si lleva varias vueltas sin dormir, para no dejar al
  escritorio sin cuadros durante una ráfaga.
- **Páginas de guarda**: cada pila vive en su propia ventana de 2 MiB y lo de abajo queda sin
  mapear. Un desborde da fallo de página → doble fallo (no hay pila para el marco) → el stack de
  emergencia de la TSS, y el mensaje dice qué tarea fue. Sin guarda, una pila desbordada pisa en
  silencio lo que tenga abajo.
- **Qué se ganó**: la CPU% ahora es el tiempo que *no* corrió la tarea ociosa; el bucle del
  escritorio duerme hasta cada cuadro en vez de despertar cada 4 ms; la red atiende paquetes
  aunque el escritorio esté en medio de un cuadro largo (maquetar una página pesada tarda ~1 s).
- **Qué queda para más adelante**: un solo núcleo (SMP necesita el APIC y locks de verdad) y
  tareas que no terminan (sus pilas no se liberan). Con el espacio de usuario (K11), cada proceso
  va a ser una tarea con su propia PML4.

### K8: paginación propia
- **Cambiar de tablas en caliente**: al cargar una PML4 nueva en CR3, la instrucción siguiente ya
  se traduce con ella. Por eso la tabla nueva tiene que mapear *todo* lo que está en uso: el
  código que está corriendo, la pila, los estáticos, la información de arranque, el framebuffer y
  la RAM. En vez de adivinar qué dejó el bootloader, se **recorren sus tablas** y se copia cada
  hoja (salvo el mapeo de la RAM, que se rehace). El test "copiar una jerarquía da las mismas
  traducciones" es la garantía de que ese paso no pierde nada.
- **Las tablas de páginas son un árbol en memoria física**: cada entrada guarda la dirección
  *física* de la tabla siguiente. Para leerlas hace falta verlas en alguna dirección virtual; acá
  se usa el mapeo de toda la RAM (`offset + física`). En `jarvis-mem` eso es un trait
  (`PhysMem`), así la misma lógica corre en el kernel y, en los tests, sobre un `HashMap`.
- **Un allocator de marcos empieza con todo ocupado** y libera solo lo que el firmware declara
  como RAM usable. Al revés (todo libre y reservar lo que se sepa) un agujero no declarado
  termina entregado como si fuera memoria. El heap y el primer MiB se reservan aparte.
- **W^X** (write xor execute): los permisos salen de los encabezados de programa del ELF del
  kernel (el bootloader deja el archivo en memoria). El código queda R-X, las constantes R-- y los
  datos RW-. Hacen falta dos bits de control: EFER.NXE (sin él, el bit "no ejecutar" es un error
  de formato) y CR0.WP (sin él, el kernel escribe aunque la página diga "solo lectura"). Al
  arrancar se comprueba en la tabla activa que el código no se pueda escribir y los datos no se
  puedan ejecutar.
- **Páginas grandes**: el bootloader mapeaba hasta 1 TiB (el firmware declara zonas reservadas
  ahí arriba) con páginas de 2 MiB: ~1050 tablas, 4 MiB de RAM solo en tablas. Ahora se mapea
  denso hasta la última RAM (y todo lo de abajo de 4 GiB); lo de más arriba lo mapea `map_mmio`
  cuando un driver lo pide, de a 4 KiB y **sin caché**, que es lo correcto para registros de un
  dispositivo. Resultado: 29 tablas. Con `cpuid` se ve si hay páginas de 1 GiB (QEMU `qemu64` no
  las tiene).
- **Qué queda para más adelante**: los buffers de DMA siguen saliendo del heap (física = virtual −
  offset); cuando haya espacio de usuario (K11) cada proceso va a tener su propia PML4, que
  comparte la mitad alta (el kernel) y los marcos van a salir de este allocator.

### K6: Brave, sesión, personalización y sincronización
- **Un navegador remoto es un VNC con más información**: en vez de mandar la pantalla entera, el
  puente le pide a Brave cada cuadro por DevTools (`Page.startScreencast`), lo compara con el
  anterior en mosaicos de 64×64 y manda solo los que cambiaron, comprimidos con LZ4. Una página
  entera son ~1 MB la primera vez, y después unos pocos KB por cambio. Además de la imagen viajan
  las pestañas, el título y si se puede ir atrás: por eso la barra la dibuja JARVIS.
- **Control de flujo de punta a punta**: Brave no manda otro cuadro hasta que se le confirma el
  anterior, y el puente no se lo confirma hasta que el kernel confirmó el suyo. Si el kernel está
  ocupado, los cuadros intermedios se descartan en vez de acumularse (lo que importa es el último).
- **El mismo protocolo en los dos lados**: `desktop/src/remote.rs` es `no_std` y lo usan el kernel y
  el puente (que corre en Windows). Un cambio de formato no puede quedar a medias.
- **Conexiones largas**: hasta K5 la red solo hacía "pedí esto, dame la respuesta". Una conexión
  que dura necesita una cola de salida con tope (si el otro lado no lee, es un error y no se come
  la memoria) y un cierre en dos pasos: en smoltcp, sacar el socket enseguida después de
  `close()` hacía que el FIN no saliera nunca.
- **Sincronización = estado por archivo, no un registro de eventos**: cada archivo guarda su hash,
  un **reloj de Lamport** (un contador que salta al máximo visto + 1: ordena cambios de máquinas
  sin relojes de pared confiables) y su **origen**, la versión en la que las dos coincidieron. Un
  cambio que llega con un origen distinto de la versión local, si la local también cambió, es un
  conflicto; las dos máquinas deciden lo mismo sin hablar (gana el reloj más alto; empate, el id
  de máquina). El motor (`kernel/sync/`) no sabe de discos ni de red: se prueba entero en el host
  con dos máquinas simuladas.
- **Cifrado autenticado en vez de TLS**: las dos puntas comparten el código de emparejado, así que
  alcanza con HKDF (código → id de grupo + clave) y ChaCha20-Poly1305 con un nonce de id de
  máquina + contador; un contador repetido se descarta. El kernel compila sin SSE/AVX: las crates
  de cifrado van con sus versiones por software (`--cfg chacha20_force_soft` en `.cargo/config.toml`;
  la de AVX2 hacía fallar a LLVM).
- **El relé no guarda nada**: reenvía a quien esté conectado. Un manifiesto mandado antes de que la
  otra máquina llegue se pierde, así que al recibir el de una máquina nueva se contesta con el
  propio (lo encontró la prueba con dos QEMU, no los tests en memoria).
- **El Torito**: una ISO 9660 mínima (descriptores, tabla de rutas, raíz) con un catálogo que
  apunta a una imagen FAT "sin emulación" para EFI: la partición EFI que ya genera el bootloader,
  sacada de la tabla GPT. El firmware la monta y ejecuta `EFI/BOOT/BOOTX64.EFI`.
- **virtio moderno**: el disco y la red usan la interfaz vieja (por puertos). La placa de video solo
  existe en la moderna, con los registros en memoria: su dirección sale de la lista de
  "capacidades" PCI, y hay que mapearla **sin caché** (`paging.rs`), porque un registro de un
  dispositivo no se puede leer de una copia. Las tablas de páginas nuevas salen del heap, donde se
  conoce la dirección física.
- **Un solo lienzo, varias salidas**: el escritorio sigue dibujando una imagen; cada salida de la
  placa muestra un rectángulo de ella. Extender es una imagen más ancha, duplicar es que las dos
  salidas muestren el mismo rectángulo. El principal siempre está en (0, 0): así el HUD, las
  barras y los menús no cambiaron (solo el reloj y el mensaje, que se ubicaban con el ancho de la
  imagen y ahora se dibujan sobre una vista del principal, `Canvas::sub`).
- **El orden importa al cambiar de modo**: el escritorio se redimensiona al procesar la tecla,
  pero el kernel cambia las superficies después del cuadro. Ese cuadro se dibujaba con la
  geometría vieja y quedaban restos: después de cambiar, se redibuja todo.
- **`screendump` de QEMU**: la salida y el dispositivo van *después* del archivo
  (`screendump archivo video 1`); con `-d`/`-p` antes contesta `invalid char 'p'`.
- **Selección = ancla + cursor**: lo seleccionado es lo que queda entre donde empezó (el ancla) y
  donde está el cursor, en el orden que sea. Así Shift+flechas, arrastrar con el mouse y
  Shift+clic son lo mismo: mover el cursor sin mover el ancla. En Archivos, además, un conjunto
  de filas marcadas para Ctrl+clic, que puede tener huecos.
- **Distribuciones como fracciones**: cada plantilla es una lista de zonas en milésimos de la
  zona de trabajo, y los bordes se calculan con la misma cuenta desde los dos lados (`x0` de una
  es el `x1` de la otra), así no quedan huecos de un píxel por redondeo. El test de mosaico suma
  las áreas y verifica que no se superpongan.
- **Colores que se cambian en vivo**: la paleta eran constantes (`theme::CYAN`) usadas en 450
  lugares. Pasaron a ser funciones que leen casilleros atómicos (`theme::cyan()`); un tema nuevo
  es escribir 17 números. La paleta por defecto es la de siempre y hay un test que lo verifica.
  El aspecto que no es color (tamaños de letra, lado de los botones) está en `look.rs`, con la
  misma idea. Como es global, los tests que lo cambian van todos en un solo test
  (`tests/personalizacion.rs`): si fueran varios, correrían en paralelo y se pisarían.
- **Por qué tardaba la captura de pantalla**: no era la RAM ni los núcleos de la máquina virtual.
  Guardar 3 MB hacía **4.505 pedidos al disco**: un cluster de datos por pedido, y cada cluster
  además escribía su sector de la FAT en las dos copias. Con WHPX, cada pedido cuesta ~0,6 ms
  (va y vuelve del anfitrión), así que eran ~3 s de espera. Ahora los clusters seguidos van en un
  solo pedido y la FAT se actualiza por sector: **98 pedidos y 145 ms** (antes, 2.946 ms). Al
  arrancar, la FAT se lee en bloques de 32 KiB en vez de sector por sector. El log
  `FRAME_LENTO` del puerto serie anota cualquier frame de más de 300 ms, con cuánto fue disco.
- **Una trampa al medir**: `Copy-Item` de Windows conserva la fecha del archivo, y cargo decide
  qué recompilar por fecha. Al restaurar una versión de `fat32.rs` para comparar, el kernel
  siguió con la vieja y la primera medición salió igual. La fecha manda.
- **Leer un sensor sin romper nada**: la temperatura está en un registro específico del modelo
  (MSR). Leer uno que no existe es una excepción (#GP) que, sin manejador, cuelga el kernel. Por
  eso se pregunta antes con `cpuid`: fabricante, sensor presente y, sobre todo, si hay un
  hipervisor (QEMU a veces copia el bit de la CPU real pero no emula el registro).
- **Suspender sin ACPI**: el S3 de verdad apaga la CPU y la memoria queda en autorrefresco; volver
  necesita código que arranque en modo real y un intérprete de AML para saber qué escribir. Por
  ahora "suspender" es no dibujar nada: el bucle ya duerme en `hlt`, así que la CPU queda casi
  ociosa, y el primer evento de entrada despierta.
- **Procesos huérfanos**: si `xtask` se corta con Ctrl+C, su Brave sin ventana queda vivo y con el
  perfil tomado, y el próximo no arranca. Al empezar, el puente cierra solo los Brave que usan
  **su** perfil (nunca el de Roman).

### K5: motor web, firewall, tiendas e idiomas
- **Un navegador son cuatro etapas**: HTML → árbol, árbol + CSS → estilo de cada elemento
  (cascada y herencia), estilos → cajas con posición (maquetación), cajas → píxeles. En K4 se
  saltaba la tercera y por eso todo salía en una columna. Separarlas hizo que cada una se pueda
  probar sola: la maquetación se prueba con HTML chico y se mira con páginas reales en el host.
- **Armar en (0, 0) y correr después**: en un renglón o una fila flex no se sabe dónde va una
  caja hasta medir a sus vecinas. Cada caja se arma aparte con su esquina en (0, 0) y después se
  corre todo lo que dibujó. Si se armara dos veces (medir y ubicar), las filas flex anidadas
  costarían el doble por cada nivel.
- **Tamaños intrínsecos**: para flex, tablas, `inline-block` y flotantes hace falta saber cuánto
  mediría el contenido "sin cortar renglones" y "lo más angosto posible" (la palabra más larga).
  Se calculan una vez por elemento y quedan en caché.
- **Lo que esconde una página**: menús cerrados con `display:none`, textos "solo para lectores de
  pantalla" con `clip`, cosas fuera de la pantalla con `left:-9999px`, íconos hechos con
  `mask-image`. Sin entender cada truco, la página se llenaba de basura; con `:not()`, atributos
  y `@media` bien evaluados (con el ancho real de la ventana) desaparece sola.
- **Variables que se nombran a sí mismas**: Wikipedia define `--font-size-medium:
  var(--font-size-medium, 1rem)`. Resolverla contra sí misma daba un ciclo; en CSS eso vale lo del
  padre o el respaldo.
- **El firewall donde pasa todo**: como ninguna app abre sockets, el lugar más simple y más
  seguro es el `Outbox`: se decide antes del DNS y se sabe qué app lo pidió (un filtro de
  paquetes no lo sabría). Cuando haya sockets de verdad, bajará a la pila de red.
- **Transiciones sin romper el render por partes**: cada animación depende solo de la hora del
  frame (no de cuántos frames pasaron) y marca como sucia toda su zona en cada frame y una vez
  más al terminar. Mezclar con transparencia no es idempotente: si dos zonas de recorte se
  superponen, el píxel se mezclaba dos veces y el render por partes daba distinto que el
  completo. Ahora cada píxel se mezcla una sola vez (el test lo prueba a mitad de cada animación).
- **Revisando el navegador con sitios reales** aparecieron errores que las páginas chicas no
  muestran: las variables de CSS distinguen mayúsculas (`--fgColor-accent`, y GitHub quedaba sin
  colores); un `@media` con saltos de línea entre las condiciones (Hacker News se armaba como en
  un celular); un elemento flex con `flex: 1` no puede quedar más bajo que su contenido (el pie
  de rust-lang.org tapaba la página); y GitHub enlaza 18 variantes de tema antes de su CSS de
  verdad, así que el tope de hojas de estilo dejaba afuera lo importante.
- **Idiomas con el texto original como clave**: `tr("Papelera")` en vez de `tr(TRASH_LABEL)`.
  Se lee igual que antes, un texto sin traducir no rompe nada, y la búsqueda es binaria sobre
  tablas ordenadas (un test verifica el orden y que todo sea Latin-1).

### K4: terminal, paquetes, configuración y CSS
- **Una shell sin procesos**: `jsh` es una biblioteca del escritorio, no un programa. Cada comando
  recibe sus argumentos y la entrada estándar y devuelve texto: una tubería es pasar el texto de
  uno al siguiente. Lo que tiene que esperar a la red no puede bloquear (hay un solo hilo): el
  comando devuelve un "trabajo pendiente", la shell guarda en qué parte de la línea quedó y sigue
  cuando llega la respuesta. Es un planificador cooperativo en miniatura (K9 lo hace de verdad).
- **Las palabras se expanden al ejecutar**, no al leer: por eso `false || echo $?` dice 1. Y las
  asignaciones (`N=$(wc -l < x)`) no se parten en palabras, como en bash.
- **Programas = scripts**: sin espacio de usuario no hay dónde cargar un ELF o un `.exe`. Los
  scripts de `jsh` alcanzan para `neofetch` o `cowsay` y sirven de ejemplo de cómo se distribuyen
  programas con dependencias y versiones.
- **La ventana TCP depende del driver**: el driver decía `max_burst_size = 1` y smoltcp limitaba la
  ventana de recepción a un paquete por ida y vuelta: ~90 KB/s. Sin ese tope (la cola de
  recepción tiene 256 buffers) y con 512 KiB de buffer TCP, 3 MB bajan en un segundo.
- **CSS con índices**: una página grande tiene miles de reglas. Como en los navegadores, cada regla
  se indexa por lo que exige su último selector (id, clase o etiqueta) y cada elemento solo
  prueba las que le pueden tocar.
- **Flex sin cajas**: sin maquetación de cajas, lo que más se acerca es poner en fila solo a los
  hijos directos de un `display: flex` (los menús). Hacerlo con todos los descendientes juntaba
  páginas enteras en un párrafo.
- **Teclado por posición**: QEMU manda la tecla física (scancode). La distribución es una tabla
  "tecla física → carácter" y los acentos son teclas muertas que esperan la siguiente.

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

El orden cambió varias veces a pedido: el gestor de archivos (K2), el escritorio con red (K3), la
terminal con paquetes (K4), el motor web con firewall e idiomas (K5) y Brave con sincronización
(K6) se adelantaron.

**Dónde estamos:** K0–K14 terminados (15 de 15 hitos). Lo último: el Wi-Fi (K14, ADR 0012).
La RTL8821CE de la PC de Roman tiene driver (port de rtw88, con el firmware de Realtek en el
ramdisk del arranque), una estación que busca redes en 2,4 y 5 GHz, se conecta con WPA2, usa
802.11n/ac y se reconecta sola, y una sección Wi-Fi en Configuración → Red. También: el volumen
general (Configuración → Sonido y Win+A) y la voz de JARVIS en el anfitrión arreglada.
**Falta verificar en la PC** lo que QEMU no emula: la RTL8168, el codec real de HDA y ahora la
RTL8821CE (QEMU no tiene ninguna placa Wi-Fi: la lógica se probó contra una placa simulada y
puntos de acceso de mentira, y el registro del arranque dice qué pasó).

| Hito | Qué se logra | Qué se aprende |
|---|---|---|
| **K0** ✅ | Arranca en QEMU y dibuja el HUD | Arranque UEFI, framebuffer, E/S por puertos, `no_std` |
| **K1** ✅ | Interrupciones, timer + TSC, teclado, heap. Esfera animada y pulsaciones al hablar | Interrupciones, PIC, calibración de tiempo, concurrencia sin locks |
| **K2** ✅ | **Archivos**: PCI, virtio-blk, FAT32 propio, mouse PS/2 | Drivers con DMA, sistemas de archivos, UI dirigida por eventos |
| **K3** ✅ | **Escritorio y red**: ventanas y atajos como Windows, monitor, apps, virtio-net + TCP/IP, navegador | Composición, gestores de ventanas, redes, HTTP/HTML |
| **K4** ✅ | **Terminal y sistema**: shell `jsh`, `apt`, Configuración, más atajos, escritorios virtuales, navegador con CSS e imágenes, teclado latinoamericano | Intérpretes, gestión de paquetes, CSS y la cascada |
| **K5** ✅ | **Motor web y sistema**: maquetación en cajas (flex, grid, tablas, flotantes), fuente proporcional, SVG y transparencias, YouTube sin JavaScript, firewall (`ufw`), `snap`, `winget`, idiomas, barra de arriba y transiciones de ventanas | Motores de maquetación, tipografía, filtrado de red, internacionalización, animación |
| **K6** ✅ | **Brave y sistema**: conexiones TCP largas ✅, Brave remoto (DevTools + mosaicos) ✅, temperatura, cerrar sesión y suspender ✅, personalización en capas ✅, selección múltiple y distribuciones de ventanas ✅, varios monitores (virtio-gpu) ✅, sincronización de carpetas entre máquinas (relé + ChaCha20-Poly1305) ✅ e ISO con modo en vivo ✅ | Protocolos binarios, control de flujo, relojes lógicos, criptografía autenticada, El Torito |
| **K7** ✅ | **JARVIS con Claude** (ADR 0008): la consola le habla a Claude (`jarvis serve` en el anfitrión, con el login de Claude Code) y la esfera pulsa con la respuesta ✅; acciones en JARVIS-OS con 3 niveles de permiso ✅; "abrí tal proyecto y seguí" ✅; **voz** con el micrófono y los parlantes del anfitrión (adelantada de K12) ✅; micrófono virtio-sound ✅; cuenta de Claude e inicio de sesión con Google desde Configuración ✅ | Protocolos, agentes, permisos, voz |
| **K8** ✅ | Paginación propia (tablas de páginas del kernel, no las del bootloader): allocator de marcos, W^X, páginas grandes, `map_mmio` sin caché | Memoria virtual, allocators de frames |
| **K9** ✅ | Multitarea: planificador con prioridades y desalojo, tareas del kernel con pila propia (escritorio, red, ociosa), disco y red por interrupciones | Cambio de contexto, sincronización |
| **K10** ✅ | **TLS en el kernel** (sin puente, ADR 0009): entropía y generador ChaCha20 ✅; cliente TLS 1.3/1.2 (rustls `no_std` con proveedor propio) ✅; HTTPS directo ✅; decodificadores PNG (propio) y JPEG (`zune-jpeg`) ✅ | Criptografía, certificados, compresión |
| **K11** ✅ | Espacio de usuario (ADR 0010): ring 3, syscalls, cargador ELF ✅. Los primeros programas de Linux estáticos ✅; sockets (y el firewall en la pila de red) ✅; un intérprete de JavaScript (Boa, `apt install js`) ✅. Brave **nativo** (sin el anfitrión) necesita además bibliotecas dinámicas, hilos, un servidor gráfico y mucha memoria: es la meta de este camino | Aislamiento, ABI |
| **K12** ✅ | Audio y video: salida por virtio-sound con su propia tarea ✅; mezclador, WAV e IMA ADPCM propios ✅; la voz de JARVIS suena en JARVIS-OS y la envolvente de la esfera sale del audio que suena ✅; Música con archivos y el sintetizador ✅; videos AVI (MJPEG + audio) en el Visor ✅. HDA queda para K13 (hardware real) | Drivers de audio, códecs |
| **K13** ✅ | Hardware real (ADR 0011): ACPI (tablas propias y AML), APIC y MSI ✅; discos SATA (AHCI) y NVMe con GPT ✅; placas de red e1000/e1000e, RTL8139 y RTL8168 ✅; USB (xHCI): teclado, mouse, pendrives y hubs ✅; sonido HDA ✅; sensores de temperatura (Intel, AMD, ACPI) y registro del arranque ✅; instalador desde el pendrive a un disco vacío ✅. La suspensión S3 queda para cuando haya un driver de video (ver "Lo que se aprendió") | Drivers reales |
| **K14** ✅ | **Wi-Fi** (ADR 0012), en 4 etapas: 1) tramas 802.11, RSN, WPA2-PSK (PMK, PTK, saludos de 4 vías y de grupo) y CCMP en `jarvis-wifi`, cruzados contra Python ✅; 2) driver de la RTL8821CE (port de rtw88: encendido, efuse, firmware por la página reservada, tablas de Realtek, canales, potencia, antena sin Bluetooth) y su firmware en el ramdisk del arranque ✅; 3) la estación (buscar, autenticar, asociar, saludo, datos cifrados), cable y Wi-Fi juntos con DHCP al cambiar de conexión, y Wi-Fi en Configuración → Red ✅; 4) 5 GHz (con los canales de radar solo escuchando), 802.11n/ac con WMM y reconexión con espera ✅. Falta verificarlo en la PC. El ahorro de energía queda para después | Redes inalámbricas, criptografía de enlace, firmware de dispositivos |

Recursos: [Writing an OS in Rust](https://os.phil-opp.com), la [wiki de OSDev](https://wiki.osdev.org),
la especificación de virtio y la especificación "Microsoft FAT32 File System".
