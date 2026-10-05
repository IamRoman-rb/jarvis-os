"""La conversación por voz: después de la primera orden no hace falta repetir "JARVIS" hasta
que pase 1 minuto sin hablarle (voice/listener.py)."""

from jarvis.voice.listener import CONVERSATION_MS, Event, Listener, is_goodbye

MIN = CONVERSATION_MS / 1000


def orders(events: list[Event]) -> list[str]:
    return [e.text for e in events if e.kind == "orden"]


def test_sin_jarvis_no_es_para_jarvis() -> None:
    lis = Listener()
    assert lis.phrase("pasame la sal", 0.0) == []


def test_despues_de_la_primera_orden_no_hace_falta_repetir_jarvis() -> None:
    lis = Listener()
    ev = lis.phrase("JARVIS, ¿qué hora es?", 0.0)
    assert orders(ev) == ["qué hora es"]  # split_wake recorta los signos de los bordes
    assert Event("activo", on=True) in ev
    # Sigue la conversación: sin "JARVIS".
    assert orders(lis.phrase("y mañana va a llover?", 20.0)) == ["y mañana va a llover?"]
    # Con "JARVIS" también vale (se saca la palabra).
    assert orders(lis.phrase("JARVIS abrí el navegador", 40.0)) == ["abrí el navegador"]


def test_un_minuto_sin_hablarle_termina_la_conversacion() -> None:
    lis = Listener()
    lis.phrase("JARVIS, hola", 0.0)
    assert lis.tick(MIN - 1) == []
    assert lis.tick(MIN) == [Event("activo", on=False)]
    assert lis.phrase("y otra cosa", MIN + 5) == []
    # Hay que volver a decir "JARVIS".
    assert orders(lis.phrase("JARVIS, otra cosa", MIN + 10)) == ["otra cosa"]


def test_cada_frase_renueva_el_minuto() -> None:
    lis = Listener()
    lis.phrase("JARVIS, hola", 0.0)
    lis.phrase("contame algo", 50.0)
    assert lis.tick(MIN + 10) == []  # 60 s después de la segunda, todavía no
    assert orders(lis.phrase("seguí", MIN + 40)) == ["seguí"]


def test_el_minuto_cuenta_desde_que_jarvis_termina_de_hablar() -> None:
    lis = Listener()
    lis.phrase("JARVIS, contame la historia de Roma", 0.0)
    # JARVIS habla durante 90 s: Roman igual tiene su minuto para contestarle.
    for t in range(0, 90):
        lis.speaking(float(t))
    assert lis.tick(MIN + 80) == []
    assert orders(lis.phrase("¿y después?", MIN + 85)) == ["¿y después?"]


def test_eso_es_todo_cierra_la_conversacion() -> None:
    lis = Listener()
    lis.phrase("JARVIS, hola", 0.0)
    ev = lis.phrase("Eso es todo.", 5.0)
    assert Event("fin") in ev and Event("activo", on=False) in ev
    assert orders(ev) == []
    assert lis.phrase("pasame la sal", 6.0) == []


def test_jarvis_solo_pregunta_y_espera_la_orden() -> None:
    lis = Listener()
    ev = lis.phrase("JARVIS", 0.0)
    assert ev == [Event("si"), Event("activo", on=True)]
    assert orders(lis.phrase("abrí la música", 3.0)) == ["abrí la música"]
    # Y ya está en conversación.
    assert orders(lis.phrase("subí el volumen", 30.0)) == ["subí el volumen"]


def test_win_j_escucha_la_frase_siguiente() -> None:
    lis = Listener()
    assert lis.listen_now(0.0) == [Event("activo", on=True)]
    assert orders(lis.phrase("¿qué tiempo hace?", 2.0)) == ["¿qué tiempo hace?"]
    assert lis.in_conversation(2.0)


def test_armado_vence_a_los_8_segundos() -> None:
    lis = Listener()
    lis.phrase("JARVIS", 0.0)
    assert lis.tick(8.0) == [Event("activo", on=False)]
    assert lis.phrase("abrí la música", 9.0) == []


def test_despedidas() -> None:
    assert is_goodbye("Nada más, gracias")
    assert is_goodbye("chau")
    assert is_goodbye("Listo, gracias.")
    assert not is_goodbye("chaucha")
    assert not is_goodbye("abrí el navegador")
