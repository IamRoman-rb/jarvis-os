"""Los datos de una orden: qué app, qué carpeta, qué buscar, qué recordar.

En esta primera entrega son reglas y diccionarios (sin red): el dato casi siempre es "lo que
viene después del verbo" o una palabra de una lista cerrada. La red de etiquetas por palabra
(BIO, C3) queda para la entrega siguiente, cuando haya con qué compararla.

Si falta un dato que la tool necesita, se devuelve `None` y el NLU se abstiene: nunca se
inventa un argumento.
"""

from __future__ import annotations

import re
import unicodedata

from jarvis.nlu.dataset import APPS, FOLDERS, folder_path
from jarvis.nlu.intents import BY_NAME
from jarvis.nlu.text import strip_wake


def _plain(s: str) -> str:
    t = unicodedata.normalize("NFD", s.lower())
    return "".join(c for c in t if unicodedata.category(c) != "Mn")


def _light(text: str) -> str:
    """Minúsculas, sin "JARVIS," adelante, sin cortesías ni signos al final (con tildes)."""
    t = strip_wake(text).strip().lower()
    t = re.sub(r"[\s?!.¿¡]+$", "", t)
    t = re.sub(r"\s+(por favor|porfa|dale|cuando puedas)$", "", t)
    t = re.sub(r"^[¿¡]+", "", t)
    return " ".join(t.split())


def _distance(a: str, b: str) -> int:
    if abs(len(a) - len(b)) > 1:
        return 2
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


#: Las palabras clave de cada app (sin tildes), más largas primero.
_APP_KEYS: list[tuple[str, str]] = sorted(
    {
        **{_plain(re.sub(r"^(el|la|los|las|una|un) ", "", s)): c for s, c in APPS},
        "explorador": "archivos",
        "musica": "música",
        "reproductor": "música",
        "navegador": "navegador",
        "brave": "navegador",
        "editor": "editor",
        "bloc de notas": "editor",
        "terminal": "terminal",
        "configuracion": "configuración",
        "ajustes": "configuración",
        "consola": "consola",
        "visor": "visor",
        "monitor": "monitor",
        "administrador de tareas": "monitor",
        "archivos": "archivos",
    }.items(),
    key=lambda kv: -len(kv[0]),
)

_FOLDER_KEYS: list[tuple[str, str]] = sorted(
    {_plain(re.sub(r"^(el|la|los|las|mis) ", "", s)): c for s, c in FOLDERS}.items(),
    key=lambda kv: -len(kv[0]),
)


def find_app(text: str) -> str | None:
    t = _plain(_light(text))
    for key, canon in _APP_KEYS:
        if re.search(rf"\b{re.escape(key)}\b", t):
            return canon
    # Con un error de tipeo ("termnal", "navegdor"): una letra de diferencia, en palabras largas.
    for w in t.split():
        if len(w) < 6:
            continue
        for key, canon in _APP_KEYS:
            if " " not in key and len(key) >= 6 and _distance(w, key) <= 1:
                return canon
    return None


def _after(text: str, patterns: list[str]) -> str | None:
    """Lo que viene después del primer patrón que aparece (con tildes, en minúsculas)."""
    t = _light(text)
    for p in patterns:
        m = re.search(p, t)
        if m:
            rest = t[m.end() :].strip(" ,:;")
            return rest or None
    return None


_SEARCH = [
    r"\bbusc[aá]me\b(?: en (?:internet|google|la web|brave))?",
    r"\bme busc[aá]s\b(?: en (?:internet|google|la web|brave))?",
    r"\bhac[eé] una b[uú]squeda de\b",
    r"\bquiero buscar\b",
    r"\bfijate en internet\b",
    r"\bgoogle[aá]\b",
    r"\bbusc[aá] (?:en (?:internet|google|la web|brave) )?(?:info(?:rmaci[oó]n)? (?:sobre|de) )?",
    r"\bbusca\b",
]

_REMEMBER = [
    r"\bquiero que recuerdes que\b",
    r"\bque no se te olvide que\b",
    r"\b(?:record[aá](?:me)?|acordate|anot[aá]|anotate|guard[aá]|memoriz[aá]|registr[aá]|"
    r"apunt[aá]|no te olvides|ten[eé] en cuenta|tom[aá] nota)"
    r"(?: en tu memoria)?(?: de)?(?: esto)?\s*(?:que\b|:)",
]

_MEMORY = [
    r"\bte acord[aá]s de lo que te cont[eé] sobre\b",
    r"\bqu[eé] te hab[ií]a dicho sobre\b",
    r"\b(?:busc[aá]|fijate) en tus? (?:memoria|recuerdos)(?: algo)?(?: de| del| sobre)?\b",
    r"\bte acord[aá]s (?:de|del)\b",
    r"\brecord[aá]s algo (?:de|del)\b",
    r"\bqu[eé] (?:te dije|te cont[eé]|hablamos|sab[eé]s|record[aá]s|anotaste) (?:de|del|sobre)\b",
]

_NEW_FOLDER = [
    r"\bcarpeta (?:nueva )?(?:que se llame|llamada)\b",
    r"\bcarpeta nueva\b",
    r"\bnueva carpeta\b",
    r"\bcarpeta\b",
]

_URL = re.compile(r"\b((?:https?://)?[a-z0-9][a-z0-9-]*(?:\.[a-z0-9-]+)+(?:/\S*)?)", re.IGNORECASE)
_FILE = re.compile(r"(/[^\s]+|[^\s/]+\.[a-z0-9]{2,4})\b", re.IGNORECASE)


def _url(text: str) -> str | None:
    for m in _URL.finditer(_light(text)):
        v = m[1]
        # Un archivo ("notas.txt") no es una dirección.
        if re.search(r"\.(txt|bmp|wav|avi|png|jpg|md|html?|exe|deb|zip)$", v):
            continue
        return v
    return None


def _file(text: str) -> str | None:
    t = _light(text)
    m = _FILE.search(t)
    return m[1] if m else None


def _folder(text: str) -> str | None:
    t = _light(text)
    m = re.search(r"(/\S+)", t)
    if m:
        # Una ruta que termina en un archivo no es una carpeta para listar.
        return None if re.search(r"\.[a-z0-9]{2,4}$", m[1]) else m[1]
    p = _plain(t)
    for key, canon in _FOLDER_KEYS:
        if re.search(rf"\b{re.escape(key)}\b", p):
            return canon
    return None


def extract(intent: str, text: str) -> dict[str, str] | None:
    """Los argumentos de la tool para `intent`, o `None` si falta alguno."""
    spec = BY_NAME[intent]
    out: dict[str, str] = dict(spec.fixed)
    for slot in spec.slots:
        value: str | None
        if slot == "app":
            value = find_app(text)
        elif slot == "url":
            value = _url(text)
        elif slot == "consulta":
            value = _after(text, _MEMORY if intent == "buscar_memoria" else _SEARCH)
            # "buscá en tu memoria X" no es una búsqueda en internet.
            if intent == "buscar_web" and value and re.match(r"en tus? (memo|recue)", value):
                value = None
        elif slot == "dato":
            value = _after(text, _REMEMBER)
        elif slot == "ruta" and intent == "listar_archivos":
            value = _folder(text)
        elif slot == "ruta" and intent == "crear_carpeta":
            name = _after(text, _NEW_FOLDER)
            value = folder_path(name) if name else None
        elif slot == "ruta":
            value = _file(text)
        else:
            value = None
        if not value:
            return None
        out[slot] = value
    return out


def same(a: str, b: str) -> bool:
    """¿Es el mismo dato? (Sin mayúsculas, tildes ni signos sueltos al final.)"""
    return _plain(a).strip(" .?!") == _plain(b).strip(" .?!")
