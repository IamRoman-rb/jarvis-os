"""El entendedor de órdenes propio (C2-C3): corpus sin fuga, datos, red, métricas y modos."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from datetime import datetime
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from jarvis.nlu import baseline, dataset, evaluate, router, telemetry, text
from jarvis.nlu import slots as slot_rules
from jarvis.nlu.intents import BY_NAME, INTENTS, NAMES
from jarvis.nlu.model import IntentModel
from jarvis.policy.levels import LEVELS

CLOCK = datetime(2026, 11, 8, 15, 7)


@pytest.fixture(scope="module")
def model() -> IntentModel:
    return router.train()


@pytest.fixture(scope="module")
def nlu(model: IntentModel) -> router.Understanding:
    return router.Understanding(model, clock=lambda: CLOCK)


# --- corpus -------------------------------------------------------------------------------------


def test_el_corpus_alcanza_y_es_reproducible() -> None:
    train = dataset.generate("train")
    assert len(train) >= 1500
    assert {e.intent for e in train} == set(NAMES)
    assert len(NAMES) >= 15
    assert [e.text for e in dataset.generate("train")] == [e.text for e in train]


def test_las_particiones_no_se_pisan() -> None:
    """Por plantilla y por valor libre: lo de prueba nunca se vio al entrenar."""
    parts = {s: dataset.generate(s) for s in ("train", "val", "test")}  # type: ignore[attr-defined]
    templates = {s: {(e.intent, e.template) for e in exs} for s, exs in parts.items()}
    assert not templates["train"] & templates["test"]
    assert not templates["train"] & templates["val"]
    assert not templates["val"] & templates["test"]
    texts = {s: {e.text.lower() for e in exs} for s, exs in parts.items()}
    assert not texts["train"] & texts["test"]
    for slot in ("consulta", "dato"):
        train_vals = {e.slots[slot] for e in parts["train"] if slot in e.slots}
        test_vals = {e.slots[slot] for e in parts["test"] if slot in e.slots}
        assert not train_vals & test_vals, slot
    # Cada intención tiene plantillas en las tres partes.
    for intent in NAMES:
        assert set(dataset.template_split(intent).values()) == {"train", "val", "test"}


def test_las_frases_fuera_de_alcance_nunca_se_entrenan() -> None:
    train = {e.text.lower() for e in dataset.generate("train")}
    assert not {t.lower() for t in dataset.OOD_HELDOUT} & train


# --- texto y datos ----------------------------------------------------------------------------


def test_normalizacion_y_vectores() -> None:
    assert text.plain("JARVIS, ¡Abrí la Música!") == "abri la musica"
    assert text.plain("che jarvis qué hora es?") == "que hora es"
    x = text.vectorize(["abrí la terminal", "abri la termnal", ""])
    assert x.shape == (3, text.DIM)
    assert np.allclose(np.linalg.norm(x[:2], axis=1), 1.0)
    assert not x[2].any()
    # Un error de tipeo deja la frase casi igual (n-gramas de caracteres).
    assert float(x[0] @ x[1]) > 0.7


@pytest.mark.parametrize(
    ("intent", "frase", "esperado"),
    [
        ("abrir_app", "abrime la terminal", {"app": "terminal"}),
        ("abrir_app", "jarvis, abrí brave porfa", {"app": "navegador"}),
        ("abrir_app", "abrí el termnal", {"app": "terminal"}),
        ("abrir_web", "entrá a wikipedia.org", {"url": "wikipedia.org"}),
        ("abrir_archivo", "mostrame /Documentos/notas.txt", {"ruta": "/documentos/notas.txt"}),
        (
            "buscar_web",
            "buscá en google recetas de empanadas",
            {"consulta": "recetas de empanadas"},
        ),
        ("listar_archivos", "qué hay en descargas", {"ruta": "/Descargas"}),
        ("crear_carpeta", "creá una carpeta facturas", {"ruta": "/Documentos/facturas"}),
        ("recordar", "acordate que el parcial es el jueves", {"dato": "el parcial es el jueves"}),
        ("recordar", "anotá esto: compré pilas", {"dato": "compré pilas"}),
        ("buscar_memoria", "te acordás de mi cumpleaños?", {"consulta": "mi cumpleaños"}),
    ],
)
def test_datos_de_cada_orden(intent: str, frase: str, esperado: dict[str, str]) -> None:
    got = slot_rules.extract(intent, frase)
    assert got is not None
    for k, v in esperado.items():
        assert slot_rules.same(got[k], v), (got, esperado)


def test_sin_el_dato_no_se_inventa() -> None:
    assert slot_rules.extract("abrir_app", "abrí eso") is None
    assert slot_rules.extract("abrir_web", "entrá a la página") is None
    # "buscá en tu memoria" no es una búsqueda en internet.
    assert slot_rules.extract("buscar_web", "buscá en tu memoria el parcial") is None
    # Una ruta de archivo no es una carpeta para listar.
    assert slot_rules.extract("listar_archivos", "qué hay en /Documentos/a.txt") is None


# --- la red -------------------------------------------------------------------------------------


def test_guardar_y_cargar_da_lo_mismo(model: IntentModel, tmp_path: Path) -> None:
    p = tmp_path / "m.npz"
    model.save(p)
    back = IntentModel.load(p)
    frases = ["abrí la música", "qué hora es", "explicame la relatividad"]
    assert np.allclose(back.proba(frases), model.proba(frases), atol=1e-6)
    assert back.threshold == model.threshold
    assert back.threshold_evidence == model.threshold_evidence


def test_un_archivo_de_otra_version_no_se_usa(tmp_path: Path) -> None:
    p = tmp_path / "roto.npz"
    np.savez(p, __meta__=np.array(json.dumps({"dim": 1, "hidden": 1})))
    with pytest.raises(ValueError):
        IntentModel.load(p)


def test_metricas_en_prueba_contra_las_reglas_de_hoy(nlu: router.Understanding) -> None:
    """El conjunto de prueba (plantillas nunca vistas). Los mínimos son los de esta entrega:
    pocas acciones equivocadas y bastante mejor que las reglas; la meta final de C3 (95 %) no
    está alcanzada todavía."""
    test = dataset.generate("test")
    ood = dataset.ood_heldout()
    red = evaluate.evaluate(nlu.predictor(), test, ood)

    def rules(t: str) -> tuple[str | None, dict[str, str] | None]:
        i = baseline.predict(t)
        if i is None:
            return None, None
        return i, slot_rules.extract(i, t) if BY_NAME[i].slots else {}

    reglas = evaluate.evaluate(rules, test, ood)
    assert red.macro_f1 >= reglas.macro_f1 + 0.4, (red.macro_f1, reglas.macro_f1)
    assert red.local_coverage >= 0.35
    assert red.wrong_action <= 0.05
    assert red.ood_wrong_action <= 0.15
    assert red.slot_exact >= 0.9
    assert red.p95_ms < 20


# --- decisiones -------------------------------------------------------------------------------


def test_decisiones(nlu: router.Understanding) -> None:
    d = nlu.decide("qué hora es")
    assert (d.action, d.intent, d.answer) == ("local", "hora", "Son las 15:07.")
    assert nlu.decide("qué día es hoy").answer == "Hoy es domingo 8 de noviembre de 2026."
    d = nlu.decide("abrime la música")
    assert (d.action, d.tool, d.args) == ("tool", "abrir_app", {"app": "música"})
    # Nivel 2: lo decide el modelo (con confirmación).
    d = nlu.decide("creá una carpeta fotos del viaje")
    assert d.action == "derivar"
    assert d.reason.startswith("nivel")
    # Programar va al agente de código; lo que no es una orden, al modelo.
    assert nlu.decide("arreglá el bug del login en el código").action in ("derivar", "abstener")
    assert nlu.decide("explicame la teoría de cuerdas").action in ("derivar", "abstener")


def test_lo_que_el_nlu_no_hace_va_al_modelo(nlu: router.Understanding) -> None:
    for frase in [
        "ejecutá rm -rf en la terminal",
        "abrí la terminal y ejecutá sudo apt install algo",
        "borrá la carpeta documentos",
        "instalá vlc",
        "apagá la computadora",
        "mové notas.txt a la papelera",
    ]:
        d = nlu.decide(frase)
        assert d.action in ("derivar", "abstener"), (frase, d)


# --- el cerebro local primero -------------------------------------------------------------------


class FakeGate:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def run(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        self.calls.append((tool, args))
        return True, "ok"


class FakeInner:
    def __init__(self) -> None:
        self.asked: list[str] = []

    async def reply(self, text: str) -> AsyncIterator[str]:
        self.asked.append(text)
        yield "respuesta del modelo"

    def last_info(self) -> dict[str, Any]:
        return {"agente": "claude", "uso": {"input_tokens": 100, "output_tokens": 20}}

    async def interrupt(self) -> None:
        pass

    async def close(self) -> None:
        pass


async def answer(brain: router.LocalFirstBrain, frase: str) -> str:
    return "".join([d async for d in brain.reply(frase)])


async def test_modo_sombra_mide_pero_no_actua(nlu: router.Understanding) -> None:
    gate, inner = FakeGate(), FakeInner()
    brain = router.LocalFirstBrain(inner, gate, lambda: "sombra", lambda: nlu)
    assert await answer(brain, "abrime la música") == "respuesta del modelo"
    assert gate.calls == []
    info = brain.last_info()
    assert info["nlu"]["accion"] == "tool"
    assert info["resuelto_local"] is False
    assert info["agente"] == "claude"


async def test_modo_primero_resuelve_lo_cotidiano(nlu: router.Understanding) -> None:
    gate, inner = FakeGate(), FakeInner()
    brain = router.LocalFirstBrain(inner, gate, lambda: "primero", lambda: nlu)
    assert await answer(brain, "abrime la música") == "Abro música."
    assert gate.calls == [("abrir_app", {"app": "música"})]
    assert await answer(brain, "qué hora es") == "Son las 15:07."
    assert brain.last_info()["agente"] == "jarvis-local"
    assert inner.asked == []
    # Lo de nivel 2, lo ambiguo y lo que no es una orden: al modelo.
    await answer(brain, "creá una carpeta facturas")
    await answer(brain, "escribime un poema sobre el otoño")
    assert len(inner.asked) == 2
    assert len(gate.calls) == 1


async def test_apagado_o_sin_modelo_todo_va_al_modelo(nlu: router.Understanding) -> None:
    gate, inner = FakeGate(), FakeInner()
    brain = router.LocalFirstBrain(inner, gate, lambda: "apagado", lambda: nlu)
    await answer(brain, "qué hora es")
    assert inner.asked == ["qué hora es"]
    assert "nlu" not in brain.last_info()
    brain = router.LocalFirstBrain(inner, gate, lambda: "primero", lambda: None)
    await answer(brain, "qué hora es")
    assert len(inner.asked) == 2


# --- el registro (C0) ---------------------------------------------------------------------------


def test_el_registro_no_guarda_el_texto_salvo_que_se_pida(pedidos_file: Path) -> None:
    e = telemetry.record("abrí la música", False, origen="voz", agente="claude")
    assert "texto" not in e
    assert e["largo"] == len("abrí la música")
    assert e["huella"] == telemetry.fingerprint("Abrí  la MÚSICA")
    e = telemetry.record("mi token=abc123 secreto", True, origen="texto")
    assert e["texto"] == "mi token=*** secreto"
    rows = telemetry.read(pedidos_file)
    assert len(rows) == 2


def test_el_informe_cuenta_llamadas_y_tokens_medidos() -> None:
    entries: list[dict[str, Any]] = [
        {"resuelto_local": True, "agente": "jarvis-local", "nlu": {"accion": "local"}},
        {
            "resuelto_local": False,
            "agente": "claude",
            "nlu": {"accion": "tool", "tool": "abrir_app"},
            "tools": ["abrir_app"],
            "uso": {"input_tokens": 1000, "output_tokens": 50},
        },
        {"resuelto_local": False, "agente": "gemini", "cambios": 1, "nlu": {"accion": "abstener"}},
    ]
    s = telemetry.summary(entries)
    assert "Resueltos local (modo primero): 1" in s
    assert "Se habrían resuelto local (modo sombra): 1" in s
    assert "coincidió con la tool que usó el modelo: 1 de 1" in s
    assert "entrada 1000, salida 50" in s
    assert "Cambios de agente: 1" in s
    assert telemetry.summary([]).startswith("Todavía no hay pedidos")


def test_el_catalogo_solo_usa_tools_declaradas() -> None:
    for i in INTENTS:
        if i.route == "tool":
            assert i.tool in LEVELS, i
            assert LEVELS[i.tool] <= 2, i
