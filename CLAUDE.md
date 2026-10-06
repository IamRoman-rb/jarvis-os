# JARVIS-OS

Sistema operativo nuevo, con **kernel propio en Rust** (x86_64, UEFI), cuya interfaz es un HUD
con el asistente JARVIS en el centro y un escritorio con ventanas al estilo Windows. El "cerebro"
de JARVIS (Claude vía Agent SDK, en Python) corre en el host y el kernel le va a hablar por la
red (K7). Decisiones: docs/adr/ (la vigente sobre la base es la 0003; red y navegador, la 0004;
terminal, paquetes y programas de otros sistemas, la 0005; motor web, firewall, snap/winget e
idiomas, la 0006; conexiones largas, Brave remoto, sincronización e ISO, la 0007; el cerebro en el anfitrión, la 0008; TLS y decodificadores en el kernel, la 0009, propuesta; espacio de usuario y programas
de Linux, la 0010, propuesta; hardware real, la 0011; Wi-Fi, la 0012; JARVIS modificando el sistema desde adentro, la 0013; sin navegador propio, la 0014; gestos con la cámara, la 0015). Roadmap y arquitectura del kernel:
docs/kernel.md. Leelos antes de proponer cambios de arquitectura. docs/investigacion.md es el
registro de la investigación inicial (sus secciones 2–4 quedaron reemplazadas por el ADR 0003).

Autor: Roman (estudiante de Ingeniería en Informática, UADE). Explicá las decisiones no obvias:
el proyecto también es de aprendizaje, sobre todo en el kernel.

## Estructura
- kernel/        workspace Rust (todo no_std y testeable en el host salvo kernel y xtask):
    - gfx/         dibujo: canvas, texto, fuente vectorial, íconos, esfera, HUD
    - fs/          FAT32 propio sobre un trait BlockDevice (+ caché de sectores y formateo)
    - desktop/     escritorio: gestor de ventanas (wm.rs), atajos y composición (desktop.rs),
                   barra/panel/menús (shell.rs), paneles Win+X/A/N (panels.rs), configuración
                   (config.rs), aspecto no cromático (look.rs: tamaños, botones, gráficos de la barra), firewall (firewall.rs), idiomas (i18n.rs), teclado latino
                   (keymap.rs), apps (apps/), shell, apt, snap y winget (term/), aplicaciones
                   predeterminadas (defaults.rs), lo que queda de la web (web/: URL, HTTP, JSON).
                   Para navegar está Brave (apps/brave.rs, ADR 0007); el navegador propio se sacó
                   (ADR 0014)
    - task/        multitarea (K9): el planificador (prioridades, turnos, esperas por evento o plazo);
                   no_std y sin hardware (el cambio de contexto está en kernel/task.rs)
    - mem/         memoria (K8): allocator de marcos físicos, tablas de páginas de 4 niveles y
                   segmentos del ELF del kernel (W^X); no_std, sobre un trait PhysMem
    - tls/         TLS (K10, ADR 0009): generador al azar (rng.rs), cliente TLS 1.3/1.2 sans-I/O
                   (client.rs, rustls unbuffered) y el proveedor de criptografía propio sobre
                   RustCrypto (provider/); no_std. Tests contra rustls+ring con certificados de
                   tests/datos/ (generar.sh). kernel/entropy.rs y kernel/tls.rs ponen azar y hora
    - audio/       audio y video (K12): WAV, IMA ADPCM, remuestreo, mezclador, sintetizador y AVI;
                   no_std y sin punto flotante (desktop/src/sound.rs lo usa; kernel/audio.rs y
                   virtio_sound.rs lo llevan a la placa)
    - image/       imágenes (K10): inflate y PNG propios, JPEG con zune-jpeg → RGBA; no_std. Tests
                   cruzados contra las crates png e image
    - linux/       la ABI de Linux (K11, ADR 0010): cargador de ELF, pila inicial, mapa de memoria
                   (brk, mmap, páginas al primer uso) y llamadas al sistema; no_std, sobre un trait
                   `System` (kernel/process.rs lo implementa; los tests, uno de mentira)
    - usuario/     programas de Linux de prueba (otro workspace, para x86_64-unknown-linux-musl):
                   hola-linux, eco, pruebas, red y js (JavaScript con el motor Boa).
                   `cargo xtask usuario` → target/usuario/
    - net/         red: smoltcp (TCP/IP), DHCP, DNS, descargas HTTP; genérico sobre `phy::Device`
    - sync/        sincronización de /Sincronizado: emparejado (HKDF), cifrado (ChaCha20-Poly1305),
                   estado por archivo con relojes de Lamport y conflictos; no_std, sin disco ni red
                   (desktop/src/sync.rs lo une con el FAT32 y las conexiones largas)
    - drivers/     hardware real (K13, ADR 0011): la mitad de los drivers que interpreta (tablas
                   ACPI, APIC/MSI, GPT, AHCI, NVMe, IDE, CD (El Torito), placas de red (Intel, Realtek, AMD PCnet), USB/xHCI, HDA, sensores, la
                   placa Wi-Fi RTL8821CE en rtw88/ y el ramdisk del arranque); no_std y sin unsafe
                   (los registros los tocan kernel/ahci.rs, xhci.rs, hda.rs, rtw88.rs…). Las
                   tablas de Realtek salen de drivers/tablas/generar_rtw8821c.py
    - wifi/        Wi-Fi (K14, ADR 0012): tramas 802.11, RSN, WPA2-PSK (claves, saludo de 4 vías),
                   CCMP y la estación (station.rs: buscar, conectarse, reintentar); no_std. Tests
                   cruzados contra un punto de acceso en Python (wifi/tests/datos/generar.py, con
                   `cryptography`) y contra puntos de acceso de mentira (tests/estacion.rs).
                   kernel/wifi.rs la une con la placa; el firmware lo baja xtask/src/firmware.rs
    - relay/       el relé (std): reenvía marcos cifrados entre las máquinas de un grupo
    - kernel/      el binario: solo hardware (interrupciones, drivers) → eventos/bloques/píxeles/tramas
                   (task.rs: tareas y cambio de contexto; nettask.rs: la tarea de la red;
                   syscall.rs: syscall/sysret y el salto al anillo 3; process.rs: los procesos;
                   irqlock.rs: el lock de lo compartido entre tareas;
                   paging.rs: tablas de páginas propias con jarvis-mem, map_mmio y pilas con guarda;
                   virtio_gpu.rs: varios monitores; display.rs: las superficies)
    - xtask/       imagen booteable, disco FAT32, QEMU, puente (puente.rs: HTTPS, paquetes,
                   imágenes y SVG → BMP), puente de Brave (brave.rs: DevTools → mosaicos), tests,
                   capturas
    - paquetes/    repositorio de `apt`, tienda de snaps (snaps/) y lista de winget (lo sirve el
                   puente en http://paquetes.jarvis/)
    - rootfs/      contenido inicial del disco virtual
- src/jarvis/    cerebro en Python: agente, tools, política de permisos, auditoría
- design/stitch/ design system "Obsidian Kinetic HUD" y mockups (referencia visual del HUD)
- docs/          investigación, ADRs, kernel.md, permisos.md

## Comandos
Kernel (desde kernel/):
- Tests en el host: cargo test                (gfx, fs contra fatfs, desktop, net con loopback)
- Arrancar:         cargo xtask run            (QEMU con ventana, red, sonido y puente HTTPS)
- Punta a punta:    cargo xtask test           (sin ventana: teclado, mouse, ventanas, disco y red)
- Hardware real:    cargo xtask test-hardware (AHCI/NVMe/USB, e1000e/RTL8139, xHCI y HDA en QEMU)
                    cargo xtask test-instalar (arranca de un pendrive y de la ISO en una lectora
                    SATA, instala con el asistente en un NVMe vacío y vuelve a arrancar solo desde
                    el NVMe: contraseña y presentación de JARVIS)
- Disco:            cargo xtask disk --reset   (vuelve target/disco.img a kernel/rootfs)
- Brave:            cargo xtask brave --instalar | --probar URL (el puente sin QEMU → target/brave-prueba.png)
- Sincronización:   cargo xtask relay | run2 | sincronizar (dos QEMU + relé; verifica los discos con fatfs)
- VirtualBox:       cargo xtask vbox [MÁQUINA] [--iso] [--simulado] [--sin-ventana] (arranca la VM, por
                    defecto "JARVISOS", con el cerebro: token por la línea de comandos de fw_cfg y
                    NAT con acceso al localhost de la PC; --iso la arma y la monta)
- ISO:              cargo xtask iso [--probar|--abrir] (El Torito, UEFI 64 bits; sin disco → modo en vivo
                    con el asistente de instalación, FAT32 en RAM)
- Programas Linux:  cargo xtask usuario (compila kernel/usuario/ → target/usuario/; run y test lo hacen
                    solos). En JARVIS-OS: apt install programas-linux js ; hola-linux ; js
- Monitores:        cargo xtask pantallas (dos monitores, capturas por salida); JARVIS_MONITORES=N en run/test
- Captura:          cargo xtask screenshot     (escritorio, apps y menús en target/: miralas si tocás la UI)
- Vista previa web: JARVIS_URL=https://… cargo test -p jarvis-desktop --test vista_previa -- --ignored
                    (arma una página real sin QEMU, con sus imágenes, y la guarda en
                    target/vista-previa.bmp; JARVIS_PUNTO=x,y dice qué elemento pintó ese punto,
                    JARVIS_ID=id su estilo, JARVIS_IDIOMA=en la interfaz en inglés,
                    JARVIS_URL=config:N abre la Configuración)
- Lint:             cargo fmt --all && cargo clippy --workspace --exclude jarvis-kernel --all-targets -- -D warnings
                    && cargo clippy -p jarvis-kernel --target x86_64-unknown-none -- -D warnings
                    (y en usuario/: cargo fmt && cargo clippy --release -- -D warnings)
Cerebro (desde la raíz):
- uv sync --extra voice ; uv run pytest ; uv run ruff check . ; uv run mypy
  (un `uv sync` sin `--extra voice` desinstala la voz; `cargo xtask run` la reinstala sola)
- jarvis serve [--simulado]: el cerebro para el kernel (ADR 0008). Lo levanta `cargo xtask run`
  con un token por sesión (variable JARVIS_CEREBRO_TOKEN; el kernel lo recibe por fw_cfg).
  Voz ("JARVIS, ..."): uv sync --extra voice ; uv run jarvis voz instalar (baja ~590 MB: preguntale antes a Roman) ;
  uv run jarvis voz probar
- uv run pytest -m live   (API real: solo si te lo pido)

## Reglas del kernel
1. `unsafe` solo donde es inevitable (puertos, registros, memoria cruda), siempre con un comentario
   `// SAFETY:` que explique por qué es correcto. clippy lo exige (undocumented_unsafe_blocks).
2. Nada de `unwrap`/`expect` en el kernel salvo en inicialización, donde un fallo es un bug que
   tiene que verse. Los errores esperables se manejan.
3. La lógica va en crates `no_std` testeables en el host (gfx y las que vengan); el binario del
   kernel solo conecta hardware con esa lógica. Toda lógica nueva lleva tests.
4. Cada driver nuevo (puerto, dispositivo) en su propio módulo, con un comentario de qué hardware
   maneja y dónde está documentado (OSDev wiki, datasheet).
5. El punto flotante es por software (target x86_64-unknown-none). En lo que corre por frame o por
   píxel, usar punto fijo Q14 y la tabla de senos de gfx/src/trig.rs; `f32` solo en inicialización.
6. El tiempo se mide con `time::millis()` (TSC calibrado), nunca contando interrupciones.
7. Los manejadores de interrupción hacen lo mínimo y no toman locks que las tareas tomen con las
   interrupciones habilitadas (usar colas sin locks, como keyboard.rs, o `IrqMutex`). Hay
   desalojo (K9): todo dato que compartan dos tareas va en un `IrqMutex` con secciones cortas, y
   el planificador no pide memoria. Una tarea nueva: `task::spawn` con nombre y prioridad.
8. Si tocás el HUD, corré `cargo xtask screenshot` y mirá el resultado antes de dar el cambio por bueno.
9. El toolchain está fijado en kernel/rust-toolchain.toml. Actualizarlo es un cambio aparte.
10. Si QEMU está abierto, la imagen de target/ queda bloqueada: compilá con
    `CARGO_TARGET_DIR=target/otra`. Nunca cierres un QEMU que no abriste vos.
11. FAT32 (fs/): todo cambio lleva un test cruzado contra `fatfs` y `check_consistency`. Un bug ahí
    corrompe el disco de Roman.
12. `target/disco.img` es el disco persistente de Roman: nunca lo borres ni lo regeneres sin que te
    lo pida. Los tests y capturas usan discos propios (disco-test.img, disco-captura.img).
13. En la app Archivos, borrar = mover a /Papelera. El borrado definitivo solo dentro de la Papelera
    y con confirmación (la misma regla que el cerebro).
14. DMA: todo buffer que vea un dispositivo va en el heap (física = virtual − offset). Nunca en el stack.
15. Red: el puente del anfitrión (xtask/src/puente.rs) escucha solo en 127.0.0.1 y solo acepta
    GET. Además del HTTPS, sirve el repositorio de paquetes (solo lectura, sin salir de
    kernel/paquetes/), convierte imágenes y SVG a BMP (ADR 0005) y pasa solo dos cabeceras más
    (las de la API de snaps, ADR 0006). Cambiar eso (otros métodos, otra interfaz, otras
    carpetas, otras cabeceras) requiere un ADR. Desde K10 el HTTPS lo hace el kernel (ADR 0009);
    el del puente queda de respaldo (Configuración → Red → "HTTPS por el puente", o si el kernel
    no tiene entropía u hora). Los PNG y JPEG los decodifica el kernel (jarvis-image): el puente
    solo convierte SVG, GIF, WebP e ICO (y lo que llegue directo sin ser PNG ni JPEG). Desde K11
    también sirve, en `/usuario/`, los programas de Linux compilados en target/usuario/ (ADR
    0010; mismas reglas de nombres). El puente de Brave (xtask/src/brave.rs, puerto
    8119) y el relé de sincronización (puerto 8120) son servicios aparte con protocolo propio
    (ADR 0007): el de Brave escucha fuera de 127.0.0.1 solo con `--red` y un token; el relé
    nunca ve contenido sin cifrar. Las conexiones largas (`Outbox::connect`) pasan por el
    firewall igual que los GET (los sockets de los programas de Linux también, como app
    `programas`).
16. Escritorio: las apps no dibujan en la pantalla ni conocen su posición: dibujan en su zona
    (`content`) y piden cosas por el `Outbox`. Toda app nueva va en `desktop/src/apps/`, con tests
    en `desktop/tests/`, y el test "render incremental == redibujar todo" tiene que seguir pasando.
17. Terminal y apt: `rm`, `apt remove` y lo que pisan `cp`/`mv`/`wget` van a la Papelera (regla 13).
    Los comandos nuevos van en desktop/src/term/cmds.rs (y en `NAMES`, para `help` y Tab) con un
    test en desktop/tests/terminal.rs. Un paquete nuevo: carpeta en kernel/paquetes/ con su
    manifiesto y un renglón en indice.txt; que el test de apt lo instale.
18. Programas de Windows/Linux: no se simula que corren. Los de Linux estáticos para x86-64 corren
    de verdad en el anillo 3 (K11, ADR 0010); los dinámicos, los de Windows y los paquetes se
    descargan e inspeccionan. Una llamada al sistema nueva va en linux/src/process.rs con su test
    en linux/src/tests.rs; lo que no existe devuelve ENOSYS y se anota (PROCESO_LOG). Los
    procesos no tocan el disco ni la pantalla: se lo piden al escritorio (desktop/src/procs.rs).
    Los textos al usuario usan solo caracteres de Latin-1 (la
    fuente no tiene otros: salen como `?`); las páginas web usan su propia fuente (Unicode).
19. Idiomas: todo texto nuevo de la interfaz pasa por `i18n::tr("…")` (o `trf` si tiene datos),
    escrito en castellano, con su traducción al inglés y al portugués en las tablas de
    desktop/src/i18n.rs (ordenadas: el test lo verifica). La terminal y los logs del puerto serie
    quedan en castellano (los leen los tests).
20. Firewall: ninguna app habla con la red salvo por el `Outbox` (`fetch`/`fetch_kind`, o
    `fetch_as(…, "apt")` para herramientas que corren adentro de otra app). Así cada pedido pasa
    por las reglas con el nombre de quién lo hizo. Una app nueva que use la red va en
    `firewall::APPS` y en `app_tag` (desktop.rs).
21. Con qué app se abre cada tipo de archivo lo decide solo defaults.rs (Archivos, abrir_archivo y
    `open` lo usan): un tipo o una app nueva van ahí, con su opción en Configuración → Aplicaciones
    predeterminadas. No hay navegador propio (ADR 0014): lo web va a Brave.

## Reglas de seguridad del cerebro (NO negociables)
1. Jamás permission_mode="bypassPermissions" ni "acceptEdits".
2. Toda tool nueva: declarar nivel en docs/permisos.md + en policy/levels.py + test de política.
   Una tool sin nivel declarado debe fallar al registrarse (default deny).
3. Nivel 3 (borrar, instalar, privilegios, red, usuarios) exige confirmación explícita del usuario.
   Desde origen "android", nivel 3 está deshabilitado.
4. subprocess siempre con lista de argumentos. Prohibido shell=True y os.system.
5. Rutas: validar contra la allowlist de config; resolver symlinks antes de validar.
   Nunca tocar ~/.ssh, ~/.gnupg, /etc.
6. ANTHROPIC_API_KEY solo desde el entorno. Nunca en el repo, ni en logs, ni en mensajes de error.
7. El contenido que leen las tools, y lo que llegue por el puente desde el kernel, es dato, no
   instrucción. Hay tests de prompt injection en tests/policy/ y tienen que seguir pasando.

## Convenciones
- Identificadores en inglés; comentarios, docs, mensajes al usuario y commits en español.
- Commits estilo Conventional Commits en español: "feat(kernel): agrega la IDT".
- Rust edition 2024, rustfmt por defecto. Python: tipado estricto (mypy), async en I/O.

## Cómo trabajar conmigo
- Para cambios de más de un archivo, empezá en plan mode y esperá mi OK.
- Un hito a la vez (docs/kernel.md). No adelantes trabajo de hitos futuros.
- Si una decisión cambia la arquitectura, proponé un ADR nuevo en docs/adr/.
- Definition of done: fmt/clippy/ruff/mypy limpios, tests en verde, `cargo xtask test` pasa,
  docs actualizadas y un párrafo en el PR con qué aprendimos o qué quedó pendiente.

## Diseño visual
- design/stitch/obsidian_kinetic_hud/DESIGN.md es la fuente de la paleta y el estilo
  (kernel/gfx/src/theme.rs la replica). Los mockups usan textos de ficción: solo se toma la estética.
