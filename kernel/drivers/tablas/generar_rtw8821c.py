"""Genera src/rtw88/tablas.rs: las tablas de registros de la RTL8821C (MAC, BB, AGC y radio).

Son los valores que Realtek publicó en el driver rtw88 de Linux (rtw8821c_table.c, licencia
dual GPL-2.0 / BSD-3-Clause: se usan bajo la BSD-3-Clause). Cada tabla es una lista de pares
(dirección, valor); algunos pares son condiciones (if/elif/else/endif según la placa) que
interpreta `rtw88::phy::load_table`.

    uv run --no-project python generar_rtw8821c.py
"""

import re
import urllib.request
from pathlib import Path

COMMIT = "a90ee4305c4a5df72c11b31dacfdc76e00fcf78a"
URL = (
    "https://raw.githubusercontent.com/torvalds/linux/"
    f"{COMMIT}/drivers/net/wireless/realtek/rtw88/rtw8821c_table.c"
)
TABLES = {
    "rtw8821c_mac": "MAC",
    "rtw8821c_bb": "BB",
    "rtw8821c_agc": "AGC",
    "rtw8821c_agc_btg_type2": "AGC_BTG",
    "rtw8821c_rf_a": "RF_A",
}

source = urllib.request.urlopen(URL).read().decode()  # noqa: S310 - URL fija de arriba
out = [
    "//! Tablas de registros de la RTL8821C, generadas por tablas/generar_rtw8821c.py: no editar.",
    "//!",
    "//! Origen: drivers/net/wireless/realtek/rtw88/rtw8821c_table.c de Linux",
    f"//! (commit {COMMIT}),",
    "//! Copyright (c) 2018-2019 Realtek Corporation, licencia dual GPL-2.0 / BSD-3-Clause, usada",
    "//! acá bajo la BSD-3-Clause. Pares (dirección, valor); los que tienen el bit 31 o 30 en la",
    "//! dirección son condiciones (ver `phy::load_table`).",
    "",
]
for name, rust in TABLES.items():
    m = re.search(rf"static const u32 {name}\[\] = \{{(.*?)\}};", source, re.DOTALL)
    if not m:
        raise SystemExit(f"no encontré {name}")
    values = [int(v, 16) for v in re.findall(r"0x[0-9A-Fa-f]+", m.group(1))]
    if len(values) % 2:
        raise SystemExit(f"{name}: cantidad impar de valores")
    out.append(f"pub static {rust}: [u32; {len(values)}] = [")
    for i in range(0, len(values), 8):
        out.append("    " + " ".join(f"0x{v:08x}," for v in values[i : i + 8]))
    out.append("];")
    out.append("")
dest = Path(__file__).parent.parent / "src" / "rtw88" / "tablas.rs"
dest.write_text("\n".join(out), encoding="utf-8", newline="\n")
print(f"{dest} listo")
