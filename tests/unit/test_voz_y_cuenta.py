"""Lo que filtra la voz, cómo habla, la cuenta de Claude y el prompt de JARVIS."""

from datetime import datetime

import pytest

from jarvis.account import AccountError, parse_status
from jarvis.agent.prompts import build_prompt
from jarvis.voice.audio import for_speech, is_noise, split_wake


def test_las_alucinaciones_de_whisper_no_son_pedidos() -> None:
    assert is_noise("¡Suscríbete!")
    assert is_noise("Gracias por ver el video.")
    assert is_noise("Subtítulos realizados por la comunidad de Amara.org")
    assert is_noise("...")
    assert not is_noise("JARVIS, abrí el navegador")
    assert not is_noise("¿qué hora es?")


def test_jarvis_solo_no_es_una_orden() -> None:
    assert split_wake("JARVIS.") == ""
    assert split_wake("Jarvis, abrime el archivo") == "abrime el archivo"


def test_habla_sin_markdown_ni_emojis() -> None:
    text = "**Listo**, abrí `file.txt` 😀\n- uno\n- dos\nMirá https://ejemplo.com/x"
    assert for_speech(text) == "Listo, abrí file.txt. uno. dos. Mirá el enlace"
    assert for_speech("🙂") == ""


def test_la_cuenta_de_claude_code() -> None:
    acc = parse_status(
        '{"loggedIn": true, "authMethod": "claude.ai", "email": "r@x.com", '
        '"subscriptionType": "pro"}'
    )
    assert (acc.logged_in, acc.email, acc.plan) == (True, "r@x.com", "pro")
    assert not parse_status('{"loggedIn": false}').logged_in
    with pytest.raises(AccountError):
        parse_status("no es json")


def test_el_prompt_sabe_donde_esta_y_la_fecha() -> None:
    p = build_prompt(True, datetime(2026, 9, 28, 11, 30))
    assert "JARVIS-OS, no a Windows" in p
    assert "abrir_archivo" in p and "WebSearch" in p
    assert "Tenés voz" in p
    assert "lunes 28/09/2026 11:30" in p
    assert "Ahora no tenés voz" in build_prompt(False)
