"""La cuenta de Claude del cerebro: la de Claude Code en el anfitrión (el Agent SDK la usa).

Configuración → Asistente de JARVIS-OS muestra con qué cuenta está y tiene un botón para iniciar
sesión con Google: acá se lanza `claude auth login`, que abre el navegador del anfitrión en
claude.ai (ahí se elige "Continuar con Google") y espera a que termine. Las credenciales nunca
pasan por JARVIS: las maneja el navegador y Claude Code.
"""

from __future__ import annotations

import asyncio
import json
import logging
import platform
import shutil
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger("jarvis.cuenta")

#: Lo que se espera a que Roman termine de entrar en el navegador.
LOGIN_TIMEOUT = 300.0
STATUS_TIMEOUT = 20.0


class AccountError(RuntimeError):
    """No se pudo preguntar o iniciar la sesión (sin Claude Code, error del comando)."""


@dataclass(frozen=True)
class Account:
    logged_in: bool
    email: str = ""
    #: "pro", "max", "api"... (vacío si no se sabe)
    plan: str = ""


def find_cli() -> str:
    """El Claude Code que usa el Agent SDK (el que trae adentro) o, si no, el del PATH."""
    try:
        import claude_agent_sdk

        name = "claude.exe" if platform.system() == "Windows" else "claude"
        bundled = Path(claude_agent_sdk.__file__).parent / "_bundled" / name
        if bundled.exists():
            return str(bundled)
    except ImportError:
        pass
    cli = shutil.which("claude")
    if cli is None:
        raise AccountError("No encontré Claude Code en esta PC.")
    return cli


def parse_status(out: str) -> Account:
    """La salida de `claude auth status --json`."""
    try:
        data = json.loads(out)
    except json.JSONDecodeError as e:
        raise AccountError("Claude Code contestó algo que no entiendo.") from e
    if not isinstance(data, dict):
        raise AccountError("Claude Code contestó algo que no entiendo.")
    plan = data.get("subscriptionType") or ""
    if not plan and data.get("authMethod") not in (None, "", "claude.ai"):
        plan = "api"
    return Account(
        logged_in=data.get("loggedIn") is True,
        email=str(data.get("email") or ""),
        plan=str(plan),
    )


async def _run(*args: str, wait: float) -> tuple[int, str]:
    proc = await asyncio.create_subprocess_exec(
        find_cli(),
        *args,
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


async def status() -> Account:
    try:
        code, out = await _run("auth", "status", "--json", wait=STATUS_TIMEOUT)
    except TimeoutError as e:
        raise AccountError("Claude Code no contestó a tiempo.") from e
    # Sin sesión, `auth status` puede salir con error pero igual imprime el JSON.
    try:
        return parse_status(out)
    except AccountError:
        if code != 0:
            raise AccountError(out.strip()[:200] or "falló `claude auth status`") from None
        raise


async def login() -> Account:
    """Abre el navegador del anfitrión en claude.ai y espera a que Roman entre (con Google o
    como quiera: la página lo ofrece)."""
    try:
        code, out = await _run("auth", "login", "--claudeai", wait=LOGIN_TIMEOUT)
    except TimeoutError as e:
        raise AccountError("Pasaron 5 minutos sin terminar de entrar.") from e
    if code != 0:
        log.warning("claude auth login: %s", out.strip())
        raise AccountError(out.strip().splitlines()[-1][:200] if out.strip() else "falló")
    return await status()
