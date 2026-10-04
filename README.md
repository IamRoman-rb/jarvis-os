# JARVIS-OS

Un sistema operativo nuevo, con **kernel propio escrito en Rust**, cuya interfaz es un HUD con el
asistente **JARVIS** en el centro y un escritorio con ventanas al estilo Windows. JARVIS piensa
con Claude: el "cerebro" corre en Python en el host y el kernel le va a hablar por la red.

| Wikipedia en el navegador propio | YouTube (sin JavaScript) |
|---|---|
| ![Wikipedia](docs/img/k5-wikipedia.png) | ![YouTube](docs/img/k5-youtube.png) |
| **snap y ufw** | **JARVIS** |
| ![snap y ufw](docs/img/k5-snap-ufw.png) | ![JARVIS](docs/img/k4-escritorio.png) |

*Todo lo que se ve corre sobre el kernel propio, sin ningún sistema operativo debajo: la esfera
de partículas que late cuando JARVIS habla, ventanas y atajos como en Windows (Alt+Tab, Win+D,
Win+X, escritorios virtuales…), una terminal parecida a bash con `apt`, `snap`, `winget` y un
firewall (`ufw`), la Configuración del sistema en tres idiomas, un gestor de archivos sobre un
FAT32 escrito desde cero y un navegador con red y motor de maquetación propios (driver, TCP/IP,
HTTP, HTML, CSS, cajas, flex y grid).*

## Estado

| Parte | Qué hay | Dónde |
|---|---|---|
| Kernel | K0–K13 ✅ (14 de 15 hitos): HUD, FAT32, ventanas, red y navegador propios, terminal con `apt`, firewall, idiomas, Brave remoto, sincronización, paginación, multitarea, TLS, programas de Linux, audio y video, hardware real (ACPI, SATA/NVMe, USB, HDA, instalador). En curso: K14, Wi-Fi (etapa 1 de 4 ✅: 802.11 y WPA2 en `jarvis-wifi`; sigue el driver de la RTL8821CE) | [docs/kernel.md](docs/kernel.md) |
| Cerebro | Agente con Claude (Agent SDK), tools con permisos de 3 niveles, auditoría, voz local ("JARVIS, …": Whisper + Piper) | `src/jarvis/` |
| Puente kernel ↔ cerebro | K7 ✅ (ADR 0008): la consola y la voz de JARVIS-OS hablan con `jarvis serve` en el anfitrión | `src/jarvis/service/` |

## Probarlo

Requisitos: [rustup](https://rustup.rs), [QEMU](https://www.qemu.org) y, en Windows, el compilador
de C++ de Visual Studio. El toolchain nightly correcto se instala solo.

```bash
cd kernel
cargo xtask run      # compila y abre JARVIS-OS en QEMU (Ctrl+Alt+T = terminal, Win+I = configuración, F1 = atajos)
```

Todos los atajos, las apps y más comandos (tests, capturas, disco para VirtualBox):
[docs/kernel.md](docs/kernel.md).

El cerebro (Python, con [uv](https://docs.astral.sh/uv/)):

```bash
uv sync && uv run pytest
```

## Estructura

```
kernel/            workspace Rust
  gfx/             dibujo: HUD, esfera, texto (no_std, testeable en el host)
  fs/              FAT32 propio (no_std, testeado contra fatfs)
  desktop/         escritorio: ventanas, atajos, apps, terminal, apt/snap/winget, firewall, idiomas,
                   navegador con motor de maquetación (no_std, testeable en el host)
  net/             red: TCP/IP (smoltcp), DHCP, DNS, descargas HTTP (no_std, testeable)
  kernel/          el kernel: arranque, interrupciones y drivers
  xtask/           imagen booteable, disco virtual, QEMU, puente (HTTPS, paquetes, imágenes), tests
  paquetes/        repositorio de apt, tienda de snaps y lista de winget
  rootfs/          contenido inicial del disco virtual
src/jarvis/        cerebro: agente, tools, política de permisos, auditoría
design/            design system "Obsidian Kinetic HUD" y mockups
docs/              kernel.md, ADRs, permisos, investigación inicial
CLAUDE.md          instrucciones para desarrollar con Claude Code
```

## Decisiones

- [ADR 0003](docs/adr/0003-kernel-propio-rust.md): kernel propio en Rust (reemplaza la base Debian + XFCE).
- [ADR 0004](docs/adr/0004-red-y-navegador-propio.md): red y navegador propios (por qué no Brave) y el puente HTTPS temporal.
- [ADR 0005](docs/adr/0005-terminal-paquetes-y-programas.md): terminal propia, `apt` con paquetes de JARVIS-OS, y por qué los `.exe` se descargan pero todavía no se ejecutan.
- [ADR 0006](docs/adr/0006-motor-web-firewall-tiendas-e-idiomas.md): motor de maquetación propio (y por qué no hay JavaScript todavía), firewall sobre el `Outbox`, `snap`, `winget` e idiomas.
- ADR 0002: el cerebro usa el Claude Agent SDK, aislado y con una sola compuerta de permisos (llega con el [PR #1](https://github.com/IamRoman-rb/jarvis-os/pull/1)).
- [Investigación inicial](docs/investigacion.md): la capa JARVIS, la sincronización y Android siguen vigentes.
