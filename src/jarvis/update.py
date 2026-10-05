"""Aplicar los cambios que JARVIS le hizo a JARVIS-OS (`aplicar_cambios_sistema`).

1. Antes de tocar nada, se verifica que lo nuevo compile: el kernel (`cargo build` del mismo
   binario que arma `xtask`; mientras QEMU corre se puede, porque usa la imagen y no el binario)
   y que el cerebro se pueda importar. Si algo falla, no se reinicia: el error vuelve a JARVIS.
2. Se deja la marca de reinicio (`JARVIS_REINICIO`, la lee `cargo xtask run`) y el kernel se
   apaga. `xtask` arma la imagen nueva, levanta el cerebro de nuevo y vuelve a arrancar QEMU.
3. `xtask` deja cómo le fue en `JARVIS_ACTUALIZACION`; el saludo del arranque lo cuenta.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import sys
from dataclasses import dataclass
from pathlib import Path

#: Lo que puede tardar en compilar el kernel (en release) la primera vez.
BUILD_TIMEOUT = 900.0
IMPORT_CHECK = "import jarvis.cli, jarvis.service.server, jarvis.agent.council"


class UpdateError(RuntimeError):
    """No se puede aplicar (no compila, o este JARVIS no lo lanzó `cargo xtask run`)."""


async def _run(args: list[str], cwd: Path, wait: float) -> tuple[int, str]:
    proc = await asyncio.create_subprocess_exec(
        *args,
        cwd=cwd,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    try:
        out, _ = await asyncio.wait_for(proc.communicate(), wait)
    except TimeoutError:
        proc.kill()
        await proc.wait()
        raise
    return proc.returncode or 0, out.decode("utf-8", "replace")


def _tail(out: str, lines: int = 12) -> str:
    """Lo último de la salida (donde están los errores de cargo)."""
    return "\n".join(out.strip().splitlines()[-lines:])


@dataclass
class Updater:
    #: La raíz del repositorio de JARVIS-OS.
    repo: Path
    #: La marca que hace que `cargo xtask run` recompile y reinicie (None = no la lanzó xtask).
    marker: Path | None
    #: Donde `xtask` deja cómo le fue ("ok" o "error: ...").
    result: Path | None

    async def check(self) -> None:
        """Que lo nuevo compile, antes de apagar nada."""
        try:
            code, out = await _run(
                [
                    "cargo",
                    "build",
                    "--package",
                    "jarvis-kernel",
                    "--target",
                    "x86_64-unknown-none",
                    "--release",
                ],
                self.repo / "kernel",
                BUILD_TIMEOUT,
            )
        except TimeoutError as e:
            raise UpdateError("El kernel tardó demasiado en compilar.") from e
        except OSError as e:
            raise UpdateError(f"No pude ejecutar cargo: {e}") from e
        if code != 0:
            raise UpdateError("El kernel no compila:\n" + _tail(out))
        # El cerebro nuevo: el mismo Python, con el código de ahora (la instalación es editable).
        code, out = await _run([sys.executable, "-c", IMPORT_CHECK], self.repo, 60.0)
        if code != 0:
            raise UpdateError("El cerebro no arranca:\n" + _tail(out))

    def request_restart(self) -> None:
        if self.marker is None:
            raise UpdateError(
                "Este JARVIS no lo lanzó `cargo xtask run`: reinicialo a mano para ver los cambios."
            )
        self.marker.parent.mkdir(parents=True, exist_ok=True)
        self.marker.write_text("actualizar\n", encoding="utf-8")

    def take_result(self) -> str | None:
        """Cómo salió la última actualización, una vez (después se borra)."""
        if self.result is None or not self.result.is_file():
            return None
        try:
            text = self.result.read_text(encoding="utf-8").strip()
        finally:
            with contextlib.suppress(OSError):
                self.result.unlink()
        if text == "ok":
            return "Ya estoy con los cambios que hicimos."
        return "No pude aplicar los cambios, arranqué la versión anterior: " + text.removeprefix(
            "error: "
        )


def find_repo() -> Path | None:
    """El repositorio de JARVIS-OS: `JARVIS_REPO` (lo pone xtask) o el de este código."""
    env = os.environ.get("JARVIS_REPO")
    candidates = [Path(env)] if env else []
    candidates.append(Path(__file__).resolve().parents[2])
    for c in candidates:
        if (c / "kernel" / "Cargo.toml").is_file() and (c / "src" / "jarvis").is_dir():
            return c
    return None


def from_env() -> Updater | None:
    repo = find_repo()
    if repo is None:
        return None
    marker, result = os.environ.get("JARVIS_REINICIO"), os.environ.get("JARVIS_ACTUALIZACION")
    return Updater(repo, Path(marker) if marker else None, Path(result) if result else None)
