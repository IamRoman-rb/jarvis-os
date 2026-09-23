"""Tool ``system_info`` (nivel 1): estado general de la máquina."""

import asyncio
import platform
import time
from pathlib import Path
from typing import Any

import psutil

from jarvis.core.config import JarvisConfig
from jarvis.tools._common import ToolResult, ok

_GIB = 1024**3


def collect() -> dict[str, str]:
    """Junta los datos. Es síncrona (psutil bloquea ~0.3 s midiendo CPU): se llama en un hilo."""
    mem = psutil.virtual_memory()
    disk = psutil.disk_usage(str(Path.home()))
    uptime_h = (time.time() - psutil.boot_time()) / 3600
    return {
        "sistema": f"{platform.system()} {platform.release()}",
        "equipo": platform.node(),
        "cpu": f"{psutil.cpu_percent(interval=0.3):.0f} % de uso, {psutil.cpu_count()} núcleos",
        "ram": f"{mem.used / _GIB:.1f} de {mem.total / _GIB:.1f} GiB usados ({mem.percent:.0f} %)",
        "disco (home)": f"{disk.free / _GIB:.0f} GiB libres de {disk.total / _GIB:.0f} GiB",
        "encendido hace": f"{uptime_h:.1f} horas",
    }


async def system_info(args: dict[str, Any], cfg: JarvisConfig) -> ToolResult:
    data = await asyncio.to_thread(collect)
    return ok("\n".join(f"{k}: {v}" for k, v in data.items()))
