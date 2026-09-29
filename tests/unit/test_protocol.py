"""El protocolo con el kernel: un objeto JSON por renglón, con `t`."""

import pytest

from jarvis.protocol import MAX_LINE, ProtocolError, decode, encode


def test_ida_y_vuelta() -> None:
    msg = {"t": "texto", "id": 3, "delta": "hola, ¿qué tal?\nsegunda"}
    line = encode(msg)
    assert line.endswith(b"\n") and line.count(b"\n") == 1
    assert decode(line) == msg


@pytest.mark.parametrize("bad", [b"no json\n", b"[1,2]\n", b'{"id":1}\n', b"\xff\n"])
def test_rechaza_lo_que_no_es_un_mensaje(bad: bytes) -> None:
    with pytest.raises(ProtocolError):
        decode(bad)


def test_rechaza_renglones_enormes() -> None:
    with pytest.raises(ProtocolError):
        decode(b'{"t":"x","a":"' + b"a" * MAX_LINE + b'"}')
