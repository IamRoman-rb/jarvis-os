"""Hace de punto de acceso WPA2 con otra implementación (hashlib, hmac y `cryptography`) y
escribe wifi.txt: el beacon, los mensajes 1 y 3 del saludo de 4 vías, los 2 y 4 que tiene que
contestar la estación, un cambio de clave de grupo y tramas cifradas con CCMP.

    uv run --no-project --with cryptography python generar.py
"""

import hashlib
import hmac
import struct
from pathlib import Path

from cryptography.hazmat.primitives.ciphers.aead import AESCCM
from cryptography.hazmat.primitives.keywrap import aes_key_wrap

SSID = b"JARVIS-Casa"
PASSPHRASE = "contraseña segura"  # noqa: S105 - la de mentira de los tests
AP = bytes.fromhex("02aabbccdd01")
STA = bytes.fromhex("5e0011223344")
SERVER = bytes.fromhex("02aabbccdd99")
ANONCE = hashlib.sha256(b"anonce").digest()
SNONCE = hashlib.sha256(b"snonce").digest()
GTK = hashlib.sha256(b"gtk").digest()[:16]
GTK2 = hashlib.sha256(b"gtk2").digest()[:16]
OUI = bytes([0x00, 0x0F, 0xAC])

pmk = hashlib.pbkdf2_hmac("sha1", PASSPHRASE.encode(), SSID, 4096, 32)


def prf(key: bytes, label: bytes, data: bytes, n: int) -> bytes:
    out = b""
    i = 0
    while len(out) < n:
        out += hmac.new(key, label + b"\0" + data + bytes([i]), "sha1").digest()
        i += 1
    return out[:n]


ptk = prf(
    pmk,
    b"Pairwise key expansion",
    min(AP, STA) + max(AP, STA) + min(ANONCE, SNONCE) + max(ANONCE, SNONCE),
    48,
)
KCK, KEK, TK = ptk[:16], ptk[16:32], ptk[32:]


def rsn(caps: int) -> bytes:
    body = struct.pack("<H", 1) + OUI + b"\x04"
    body += struct.pack("<H", 1) + OUI + b"\x04" + struct.pack("<H", 1) + OUI + b"\x02"
    body += struct.pack("<H", caps)
    return bytes([48, len(body)]) + body


AP_RSN = rsn(0x000C)  # el del punto de acceso: otras capacidades que el de la estación
STA_RSN = rsn(0)


def eapol(info: int, key_len: int, replay: int, nonce: bytes, rsc: int, data: bytes) -> bytes:
    desc = struct.pack(">BHHQ", 2, info, key_len, replay) + nonce + bytes(16)
    desc += struct.pack("<Q", rsc) + bytes(8) + bytes(16) + struct.pack(">H", len(data)) + data
    frame = struct.pack(">BBH", 2, 3, len(desc)) + desc
    mic = hmac.new(KCK, frame, "sha1").digest()[:16]
    return frame[:81] + mic + frame[97:]


def unsigned(info: int, key_len: int, replay: int, nonce: bytes) -> bytes:
    f = eapol(info, key_len, replay, nonce, 0, b"")
    return f[:81] + bytes(16) + f[97:]


def key_data(*elements: bytes) -> bytes:
    d = b"".join(elements)
    if len(d) % 8:
        d += b"\xdd" + bytes(7 - len(d) % 8)
    return aes_key_wrap(KEK, d)


def gtk_kde(key_id: int, key: bytes) -> bytes:
    body = OUI + b"\x01" + bytes([key_id, 0]) + key
    return bytes([0xDD, len(body)]) + body


# Beacon: marca de tiempo, intervalo 100, capacidades ESS + privacidad; SSID, velocidades, canal 6.
beacon = struct.pack("<BBH", 0x80, 0, 0) + b"\xff" * 6 + AP + AP + struct.pack("<H", 5 << 4)
beacon += bytes(8) + struct.pack("<HH", 100, 0x0011)
beacon += bytes([0, len(SSID)]) + SSID + bytes([1, 4, 0x82, 0x84, 0x8B, 0x96, 3, 1, 6]) + AP_RSN

msg1 = unsigned(0x008A, 16, 1, ANONCE)
msg2 = eapol(0x010A, 0, 1, SNONCE, 0, STA_RSN)
msg3 = eapol(0x13CA, 16, 2, ANONCE, 0x2A, key_data(AP_RSN, gtk_kde(1, GTK)))
msg4 = eapol(0x030A, 0, 2, bytes(32), 0, b"")
group1 = eapol(0x1382, 16, 3, bytes(32), 0x50, key_data(gtk_kde(2, GTK2)))
group2 = eapol(0x0302, 0, 3, bytes(32), 0, b"")
# El mensaje 3 de un punto de acceso que cambió el RSN (bajó las capacidades).
msg3_rsn = eapol(0x13CA, 16, 2, ANONCE, 0x2A, key_data(rsn(0), gtk_kde(1, GTK)))


def ccmp(key: bytes, header: bytes, qos: int | None, key_id: int, pn: int, body: bytes) -> bytes:
    fc0, fc1 = header[0], header[1]
    fc1_aad = (fc1 & 0xC7) | 0x40
    if qos is not None:
        fc1_aad &= 0x7F
    seq = struct.unpack("<H", header[22:24])[0]
    aad = bytes([fc0 & 0x8F, fc1_aad]) + header[4:22] + struct.pack("<H", seq & 0xF)
    if qos is not None:
        aad += bytes([qos & 0xF, 0])
    a2 = header[10:16]
    nonce = bytes([(qos or 0) & 0xF]) + a2 + pn.to_bytes(6, "big")
    p = pn.to_bytes(6, "little")
    ccmp_hdr = bytes([p[0], p[1], 0, 0x20 | key_id << 6]) + p[2:]
    protected = bytes([fc0, fc1 | 0x40]) + header[2:]
    return protected + ccmp_hdr + AESCCM(key, tag_length=8).encrypt(nonce, body, aad)


SNAP = bytes([0xAA, 0xAA, 0x03, 0, 0, 0])
payload = b"\x45\x00" + b"un paquete IPv4 de mentira"
# Del punto de acceso a la estación: QoS (subtipo 8), FromDS, TID 5, con reintento.
rx_hdr = struct.pack("<BBH", 0x88, 0x02 | 0x08, 44) + STA + AP + SERVER + struct.pack("<H", 9 << 4)
rx_hdr += struct.pack("<H", 5)
ccmp_rx = ccmp(TK, rx_hdr, 5, 0, 7, SNAP + b"\x08\x00" + payload)
eth_rx = STA + SERVER + b"\x08\x00" + payload
# De la estación al punto de acceso (lo que arma from_ethernet con secuencia 3), PN 1.
tx_hdr = struct.pack("<BBH", 0x08, 0x01, 0) + AP + STA + SERVER + struct.pack("<H", 3 << 4)
eth_tx = SERVER + STA + b"\x08\x06" + b"un pedido ARP de mentira"
ccmp_tx = ccmp(TK, tx_hdr, None, 0, 1, SNAP + b"\x08\x06" + b"un pedido ARP de mentira")
# Difusión con la clave de grupo (índice 1, PN 0x2B: uno más que el RSC del mensaje 3).
bc_hdr = struct.pack("<BBH", 0x08, 0x02, 0) + b"\xff" * 6 + AP + SERVER + struct.pack("<H", 1 << 4)
ccmp_bc = ccmp(GTK, bc_hdr, None, 1, 0x2B, SNAP + b"\x08\x06" + b"difusion")

out = {
    "ssid": SSID,
    "ap": AP,
    "sta": STA,
    "anonce": ANONCE,
    "snonce": SNONCE,
    "pmk": pmk,
    "kck": KCK,
    "kek": KEK,
    "tk": TK,
    "gtk": GTK,
    "gtk2": GTK2,
    "beacon": beacon,
    "msg1": msg1,
    "msg2": msg2,
    "msg3": msg3,
    "msg4": msg4,
    "group1": group1,
    "group2": group2,
    "msg3_rsn": msg3_rsn,
    "ccmp_rx": ccmp_rx,
    "eth_rx": eth_rx,
    "eth_tx": eth_tx,
    "ccmp_tx": ccmp_tx,
    "ccmp_bc": ccmp_bc,
}
text = "# Generado por generar.py: no editar a mano.\n"
text += f"passphrase={PASSPHRASE.encode().hex()}\n"
text += "".join(f"{k}={v.hex()}\n" for k, v in out.items())
Path(__file__).with_name("wifi.txt").write_text(text, encoding="ascii", newline="\n")
print("wifi.txt listo")
