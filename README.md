# JARVIS-OS

Un sistema operativo nuevo, con **kernel propio escrito en Rust**, cuya interfaz es un HUD con el
asistente **JARVIS** en el centro y un escritorio con ventanas al estilo Windows. JARVIS piensa
con Claude: el "cerebro" corre en Python en el host y el kernel le va a hablar por la red.

| JARVIS | Ventanas |
|---|---|
| ![JARVIS](docs/img/k3-escritorio.png) | ![Archivos y monitor](docs/img/k3-monitor.png) |
| **Navegador** | **Alt+Tab** |
| ![Navegador](docs/img/k3-web.png) | ![Alt+Tab](docs/img/k3-alt-tab.png) |

*Todo lo que se ve corre sobre el kernel propio, sin ningún sistema operativo debajo: la esfera
de partículas que late cuando JARVIS habla, ventanas con minimizar/maximizar/cerrar y atajos
como en Windows (Alt+Tab, Win+D…), un gestor de archivos sobre un FAT32 escrito desde cero, un
monitor del sistema con gráficos y un navegador con red propia (driver, TCP/IP, HTTP y HTML).*

## Estado

| Parte | Qué hay | Dónde |
|---|---|---|
| Kernel | K3 ✅: ventanas y atajos como Windows, monitor, consola, editor, música, placa de red virtio-net, TCP/IP y navegador. Antes: FAT32 propio, mouse, HUD animado. Siguiente: K4 (puente con el cerebro) | [docs/kernel.md](docs/kernel.md) |
| Cerebro | Núcleo por texto: agente con Claude, 4 tools, permisos de 3 niveles, auditoría | `src/jarvis/` ([PR #1](https://github.com/IamRoman-rb/jarvis-os/pull/1)) |
| Puente kernel ↔ cerebro | Hito K4 | — |

## Probarlo

Requisitos: [rustup](https://rustup.rs), [QEMU](https://www.qemu.org) y, en Windows, el compilador
de C++ de Visual Studio. El toolchain nightly correcto se instala solo.

```bash
cd kernel
cargo xtask run      # compila y abre JARVIS-OS en QEMU (Win+E = Archivos, Win+R = consola, Alt+Tab…)
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
  desktop/         escritorio: ventanas, atajos, apps, navegador (no_std, testeable en el host)
  net/             red: TCP/IP (smoltcp), DHCP, DNS, descargas HTTP (no_std, testeable)
  kernel/          el kernel: arranque, interrupciones y drivers
  xtask/           imagen booteable, disco virtual, QEMU, puente HTTPS, tests de punta a punta
  rootfs/          contenido inicial del disco virtual
src/jarvis/        cerebro: agente, tools, política de permisos, auditoría
design/            design system "Obsidian Kinetic HUD" y mockups
docs/              kernel.md, ADRs, permisos, investigación inicial
CLAUDE.md          instrucciones para desarrollar con Claude Code
```

## Decisiones

- [ADR 0003](docs/adr/0003-kernel-propio-rust.md): kernel propio en Rust (reemplaza la base Debian + XFCE).
- [ADR 0004](docs/adr/0004-red-y-navegador-propio.md): red y navegador propios (por qué no Brave) y el puente HTTPS temporal.
- ADR 0002: el cerebro usa el Claude Agent SDK, aislado y con una sola compuerta de permisos (llega con el [PR #1](https://github.com/IamRoman-rb/jarvis-os/pull/1)).
- [Investigación inicial](docs/investigacion.md): la capa JARVIS, la sincronización y Android siguen vigentes.
