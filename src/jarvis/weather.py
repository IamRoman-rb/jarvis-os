"""El clima de donde está Roman, para el saludo de la mañana.

- **Dónde**: lo de `config.toml` (`[clima] latitud, longitud, lugar`) o, si no está, la ubicación
  aproximada de la IP pública de la PC (ipwho.is: ciudad, no la dirección).
- **El clima**: Open-Meteo (sin clave): ahora, la máxima, la mínima y la probabilidad de lluvia
  del día.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from typing import Any

from jarvis.agent.providers import AgentError, Http, UrllibHttp

log = logging.getLogger("jarvis.clima")

TIMEOUT = 10.0
LOCATE_URL = "https://ipwho.is/"
FORECAST_URL = "https://api.open-meteo.com/v1/forecast"


class WeatherError(RuntimeError):
    """No se pudo averiguar el lugar o el clima."""


@dataclass(frozen=True)
class Place:
    lat: float
    lon: float
    #: El nombre que se dice ("Buenos Aires"); vacío = no se nombra.
    name: str = ""


#: Los códigos del tiempo de la OMM (los que usa Open-Meteo), dichos en castellano.
WMO: dict[int, str] = {
    0: "despejado",
    1: "mayormente despejado",
    2: "parcialmente nublado",
    3: "nublado",
    45: "con niebla",
    48: "con niebla y escarcha",
    51: "con llovizna leve",
    53: "con llovizna",
    55: "con llovizna intensa",
    56: "con llovizna helada",
    57: "con llovizna helada intensa",
    61: "con lluvia leve",
    63: "lloviendo",
    65: "con lluvia fuerte",
    66: "con lluvia helada",
    67: "con lluvia helada fuerte",
    71: "con nevadas leves",
    73: "nevando",
    75: "con nevadas fuertes",
    77: "con granizo fino",
    80: "con chaparrones leves",
    81: "con chaparrones",
    82: "con chaparrones fuertes",
    85: "con chaparrones de nieve",
    86: "con chaparrones de nieve fuertes",
    95: "con tormenta",
    96: "con tormenta y granizo",
    99: "con tormenta y granizo fuerte",
}


def describe(data: dict[str, Any], place: str) -> str:
    """La respuesta de Open-Meteo como una frase para decir en voz alta."""
    try:
        now = data["current"]
        daily = data["daily"]
        temp = round(float(now["temperature_2m"]))
        code = int(now["weather_code"])
        hi = round(float(daily["temperature_2m_max"][0]))
        lo = round(float(daily["temperature_2m_min"][0]))
        rain = daily.get("precipitation_probability_max", [None])[0]
    except (KeyError, IndexError, TypeError, ValueError) as e:
        raise WeatherError("Open-Meteo contestó algo que no entiendo.") from e
    where = f"En {place} hay" if place else "Hay"
    grados = "grado" if abs(temp) == 1 else "grados"
    sky = WMO.get(code, "")
    text = f"{where} {temp} {grados}" + (f" y está {sky}" if sky else "")
    text += f". La máxima va a ser de {hi} y la mínima de {lo}"
    if isinstance(rain, int | float) and rain >= 30:
        text += f", con {round(rain)} % de probabilidad de lluvia"
    return text + "."


async def locate(http: Http) -> Place:
    """La ubicación aproximada de la IP pública (ciudad)."""
    status, data = await http.request("GET", LOCATE_URL, {})
    if status >= 300 or not isinstance(data, dict) or data.get("success") is False:
        raise WeatherError("No pude averiguar dónde estás.")
    try:
        return Place(float(data["latitude"]), float(data["longitude"]), str(data.get("city", "")))
    except (KeyError, TypeError, ValueError) as e:
        raise WeatherError("No pude averiguar dónde estás.") from e


async def today(place: Place | None = None, http: Http | None = None) -> str:
    """El clima de hoy, como una frase. Sin `place`, el de la IP pública."""
    http = http or UrllibHttp()

    async def run() -> str:
        where = place or await locate(http)
        url = (
            f"{FORECAST_URL}?latitude={where.lat:.4f}&longitude={where.lon:.4f}"
            "&current=temperature_2m,weather_code"
            "&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max"
            "&timezone=auto&forecast_days=1"
        )
        status, data = await http.request("GET", url, {})
        if status >= 300 or not isinstance(data, dict):
            raise WeatherError(f"Open-Meteo contestó {status}.")
        return describe(data, where.name)

    try:
        return await asyncio.wait_for(run(), TIMEOUT)
    except (TimeoutError, AgentError) as e:
        raise WeatherError("No pude conectarme para ver el clima.") from e
