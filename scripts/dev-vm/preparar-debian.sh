#!/usr/bin/env bash
# Prepara una Debian 13 + XFCE recién instalada para desarrollar JARVIS-OS.
# Idempotente: se puede correr varias veces. Correlo como tu usuario (NO con sudo):
# el script pide sudo solo para apt.
#
#   bash scripts/dev-vm/preparar-debian.sh
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [ "$(id -u)" -eq 0 ]; then
  echo "Correlo como tu usuario, no como root: el script usa sudo solo donde hace falta." >&2
  exit 1
fi

echo "==> Paquetes del sistema (te va a pedir tu contraseña de sudo)"
PAQUETES=(
  build-essential curl jq git gh python3 python3-venv shellcheck
  libglib2.0-bin          # gio: lo usan open_app y trash_file
  zenity libnotify-bin    # confirmaciones gráficas (fase 1b)
  xdotool wmctrl          # control de ventanas en X11
  dbus-user-session       # D-Bus de sesión para jarvisd (fase 1b)
  pipewire pipewire-pulse wireplumber   # audio (fase 1c)
  linux-headers-amd64     # para compilar las Guest Additions de VirtualBox
  btrfs-progs snapper     # snapshots del filesystem
)
sudo apt-get update
sudo apt-get install -y "${PAQUETES[@]}"

echo "==> uv (instalador oficial de Astral, en ~/.local/bin, sin sudo)"
if ! command -v uv >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/uv" ]; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
fi
export PATH="$HOME/.local/bin:$PATH"

echo "==> Carpetas permitidas por defecto de JARVIS"
mkdir -p "$HOME/Facultad" "$HOME/Proyectos"

echo "==> Dependencias y tests del proyecto"
cd "$REPO_DIR"
uv sync
uv run pytest

echo
echo "==> Chequeo del entorno"
ok()   { printf '  [ok]    %s\n' "$1"; }
warn() { printf '  [aviso] %s\n' "$1"; }

if [ "${XDG_SESSION_TYPE:-}" = "x11" ]; then ok "sesión X11"
else warn "XDG_SESSION_TYPE='${XDG_SESSION_TYPE:-}' (se espera x11; corré esto desde la terminal de XFCE)"; fi

if [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] && busctl --user status >/dev/null 2>&1; then ok "D-Bus de sesión"
else warn "no encontré el D-Bus de sesión (¿estás en una sesión gráfica?)"; fi

if gio --version >/dev/null 2>&1; then ok "gio $(gio --version)"; else warn "gio no responde"; fi

if notify-send "JARVIS-OS" "Entorno de desarrollo listo" >/dev/null 2>&1; then ok "notificaciones (mirá la esquina de la pantalla)"
else warn "notify-send falló"; fi

if findmnt -n -o FSTYPE / | grep -q btrfs; then ok "raíz en Btrfs"; else warn "la raíz no es Btrfs (no bloquea el desarrollo)"; fi

ok "$(uv run python --version) vía uv"
echo
echo "Listo. Si ~/.local/bin no queda en el PATH al abrir otra terminal, cerrá sesión y volvé a entrar."
