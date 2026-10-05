"""El saludo al arrancar (según la hora) y el clima de la mañana, sin red."""

import asyncio
from pathlib import Path
from typing import Any

import pytest

from jarvis.agent.brain import ScriptedBrain
from jarvis.protocol import decode, encode
from jarvis.service.server import Host, part_of_day, start
from jarvis.weather import Place, WeatherError, describe, today

FORECAST = {
    "current": {"temperature_2m": 13.6, "weather_code": 2},
    "daily": {
        "temperature_2m_max": [19.2],
        "temperature_2m_min": [8.9],
        "precipitation_probability_max": [40],
    },
}


class FakeHttp:
    def __init__(self, *answers: tuple[int, Any]) -> None:
        self.answers = list(answers)
        self.urls: list[str] = []

    async def request(
        self, method: str, url: str, headers: dict[str, str], body: Any = None
    ) -> tuple[int, Any]:
        self.urls.append(url)
        return self.answers.pop(0)


def test_la_frase_del_clima() -> None:
    assert describe(FORECAST, "Buenos Aires") == (
        "En Buenos Aires hay 14 grados y está parcialmente nublado. La máxima va a ser de 19 "
        "y la mínima de 9, con 40 % de probabilidad de lluvia."
    )
    seco = {**FORECAST, "daily": {**FORECAST["daily"], "precipitation_probability_max": [5]}}
    assert describe(seco, "").startswith("Hay 14 grados")
    assert describe(seco, "").endswith("la mínima de 9.")
    with pytest.raises(WeatherError):
        describe({"current": {}}, "")


async def test_sin_lugar_usa_la_ubicacion_de_la_ip() -> None:
    http = FakeHttp(
        (200, {"success": True, "city": "Córdoba", "latitude": -31.42, "longitude": -64.18}),
        (200, FORECAST),
    )
    assert (await today(http=http)).startswith("En Córdoba hay 14 grados")
    assert http.urls[0] == "https://ipwho.is/"
    assert "latitude=-31.4200&longitude=-64.1800" in http.urls[1]


async def test_con_lugar_de_la_configuracion_no_pregunta_la_ip() -> None:
    http = FakeHttp((200, FORECAST))
    await today(Place(-34.6, -58.4, "Casa"), http)
    assert len(http.urls) == 1 and http.urls[0].startswith("https://api.open-meteo.com/")


def test_mañana_tarde_y_noche() -> None:
    assert [part_of_day(h) for h in (4, 5, 11, 12, 19, 20, 23)] == [
        "noche",
        "manana",
        "manana",
        "tarde",
        "tarde",
        "noche",
        "noche",
    ]


TOKEN = "t" * 32


async def greeting(tmp_path: Path, moment: str, weather: Any) -> str:
    server = await start(
        0, TOKEN, lambda s: ScriptedBrain(s, delay=0.0), Host(projects=tmp_path, weather=weather)
    )
    port = server.sockets[0].getsockname()[1]
    reader, writer = await asyncio.open_connection("127.0.0.1", port)

    async def read() -> dict[str, Any]:
        return decode(await asyncio.wait_for(reader.readline(), 5))

    writer.write(encode({"t": "hola", "token": TOKEN}))
    assert (await read())["t"] == "listo"
    writer.write(encode({"t": "saludo", "id": 1, "momento": moment, "nombre": "Roman"}))
    text = ""
    while (msg := await read())["t"] != "fin":
        assert msg["t"] == "texto" and msg["id"] == 1
        text += str(msg["delta"])
    writer.close()
    server.close()
    return text


async def test_a_la_mañana_saluda_con_el_clima(tmp_path: Path) -> None:
    async def weather() -> str:
        return "En Casa hay 14 grados."

    assert (
        await greeting(tmp_path, "manana", weather) == "Buenos días, Roman. En Casa hay 14 grados."
    )


async def test_a_la_tarde_y_a_la_noche_sin_clima(tmp_path: Path) -> None:
    async def weather() -> str:
        raise AssertionError("no se pide el clima")

    assert await greeting(tmp_path, "tarde", weather) == "Buenas tardes, Roman."
    assert await greeting(tmp_path, "noche", weather) == "Buenas noches, Roman."


async def test_si_falla_el_clima_igual_saluda(tmp_path: Path) -> None:
    async def weather() -> str:
        raise WeatherError("sin red")

    assert await greeting(tmp_path, "manana", weather) == (
        "Buenos días, Roman. No pude ver el clima ahora."
    )
