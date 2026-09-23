# Entorno de desarrollo: VM Debian 13 + XFCE

El núcleo de JARVIS se puede testear en cualquier SO, pero el demonio (systemd + D-Bus), las
notificaciones, `gio`, el control de ventanas y el audio necesitan un Linux con escritorio. Esta
VM reproduce el stack de JARVIS-OS: **Debian 13, XFCE en X11, Btrfs y PipeWire**.

Tiempo total: ~30 minutos, casi todo esperando al instalador.

## 1. Crear la VM (Windows)

Requisitos: VirtualBox 7.1 o superior y ~70 GB libres.

1. Bajá `debian-13.7.0-amd64-netinst.iso` de
   <https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/> a `Descargas` y verificá su hash
   contra el `SHA512SUMS` de esa misma página. Si hay una versión 13.x más nueva, pasá la ruta
   con `-IsoPath`.
2. Desde la raíz del repo:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\dev-vm\crear-vm.ps1 -Start
   ```
   Crea `JARVIS-OS-dev` (EFI, 8 GB RAM, 4 CPUs, disco dinámico de 60 GB, NAT, audio con micrófono,
   portapapeles compartido) y abre el instalador. Si la VM ya existe, no la toca.

> Si Windows tiene Hyper-V o "Integridad de memoria" activos, VirtualBox muestra una tortuga
> verde en la barra de estado: funciona igual, solo un poco más lento.

## 2. Instalar Debian (ventana de la VM)

| Pantalla | Qué elegir |
|---|---|
| Menú de arranque | **Graphical install** |
| Idioma / ubicación / teclado | Español → Argentina → Latinoamericano |
| Nombre de la máquina | `jarvis-dev` (dominio: vacío) |
| Contraseña de root | **Dejala vacía**: así tu usuario queda con `sudo` |
| Usuario | tu nombre y usuario (`roman`) + una contraseña que elijas vos |
| Particionado | *Guiado - utilizar todo el disco* → el disco de 60 GB → *Todos los ficheros en una partición* |
| Btrfs | En el resumen, elegí la partición montada en `/` → *Utilizar como* → **sistema de ficheros btrfs** → *Se ha terminado de definir la partición* → *Finalizar el particionado y escribir los cambios* → Sí |
| Réplica de Debian | Argentina → `deb.debian.org` |
| Selección de programas | **Desmarcá GNOME**. Marcá *entorno de escritorio Debian*, **Xfce**, *servidor SSH* y *utilidades estándar del sistema* |

Al terminar, la VM se reinicia y arranca desde el disco (el ISO puede quedar puesto: no molesta).

## 3. Guest Additions (recomendado)

Dan resolución automática al redimensionar la ventana y portapapeles compartido. Primero instalá
los headers del kernel (el script del paso 4 también lo hace, así que podés hacer este paso después):

1. Menú de la VM: *Dispositivos → Insertar imagen de CD de las «Guest Additions»*.
2. En una terminal de la VM:
   ```bash
   sudo apt install -y build-essential linux-headers-amd64
   sudo sh /media/cdrom0/VBoxLinuxAdditions.run
   sudo reboot
   ```
   Si `/media/cdrom0` está vacío, montalo con `sudo mount /dev/sr0 /media/cdrom0`.

## 4. Preparar el entorno (terminal de XFCE en la VM)

```bash
sudo apt update && sudo apt install -y git gh
gh auth login          # el repo es privado; elegí GitHub.com → HTTPS → login con el navegador
gh repo clone IamRoman-rb/jarvis-os ~/Proyectos/jarvis-os
cd ~/Proyectos/jarvis-os
bash scripts/dev-vm/preparar-debian.sh
```

El script instala las dependencias del sistema (apt, con `sudo`), `uv`, crea `~/Facultad` y
`~/Proyectos`, corre los tests y termina con un chequeo: tiene que decir **sesión X11**,
**D-Bus de sesión**, **gio**, **notificaciones** y **raíz en Btrfs**.

## 5. Snapshot

Con todo en verde, apagá la VM y sacá un snapshot para volver a este punto si algo se rompe:

```powershell
& "C:\Program Files\Oracle\VirtualBox\VBoxManage.exe" snapshot JARVIS-OS-dev take base-limpia
```

## Problemas comunes

- **Pantalla chica o no se redimensiona**: faltan las Guest Additions (paso 3).
- **`uv: command not found` en una terminal nueva**: cerrá sesión y volvé a entrar (el instalador de
  uv agrega `~/.local/bin` al PATH).
- **La sesión dice `wayland`**: en la pantalla de login, elegí la sesión *Xfce* (X11).
- **El instalador no encuentra red**: la VM usa NAT; revisá que Windows tenga internet y reiniciá la VM.
