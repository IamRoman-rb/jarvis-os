"""El motor de redes neuronales (C1): cada gradiente contra el calculado numéricamente, y redes
chicas que tienen que aprender de verdad."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import numpy as np
import pytest

from jarvis.neural import (
    SGD,
    AdamW,
    Embedding,
    LayerNorm,
    Linear,
    Tensor,
    clip_grad_norm,
    concat,
    cross_entropy,
    mlp,
    no_grad,
    tensor,
)

RNG = np.random.default_rng(1234)


def gradcheck(f: Callable[..., Tensor], *shapes: tuple[int, ...], positive: bool = False) -> None:
    """Compara el gradiente de autograd con el de diferencias centrales: (f(x+h) - f(x-h)) / 2h.

    `f` recibe tensores y devuelve cualquier tensor; se reduce con pesos al azar para que cada
    salida cuente distinto (si se sumara todo, algunos errores se cancelarían)."""
    xs = [RNG.normal(size=s) for s in shapes]
    if positive:
        xs = [np.abs(x) + 0.5 for x in xs]
    weights = RNG.normal(size=f(*[Tensor(x) for x in xs]).shape)

    def loss(arrays: list[np.ndarray]) -> float:
        return float((f(*[Tensor(a) for a in arrays]).data * weights).sum())

    params = [Tensor(x.copy(), requires_grad=True) for x in xs]
    (f(*params) * weights).sum().backward()
    h = 1e-6
    for i, x in enumerate(xs):
        numeric = np.zeros_like(x)
        for j in np.ndindex(x.shape):
            up = [a.copy() for a in xs]
            down = [a.copy() for a in xs]
            up[i][j] += h
            down[i][j] -= h
            numeric[j] = (loss(up) - loss(down)) / (2 * h)
        got = params[i].grad
        assert got is not None, f"el operando {i} no recibió gradiente"
        np.testing.assert_allclose(got, numeric, rtol=1e-5, atol=1e-6)


# --- Cada operación ------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("f", "shapes"),
    [
        (lambda a, b: a + b, [(3, 4), (3, 4)]),
        (lambda a, b: a + b, [(2, 3, 4), (4,)]),  # difusión: b se repite 6 veces
        (lambda a, b: a - b, [(3, 1), (1, 4)]),
        (lambda a, b: a * b, [(2, 3), (2, 1)]),
        (lambda a: -a * 3.0 + 1.0, [(5,)]),
        (lambda a, b: a @ b, [(3, 4), (4, 2)]),
        (lambda a, b: a @ b, [(2, 3, 4), (4, 5)]),  # por lotes, pesos compartidos
        (lambda a, b: a @ b, [(2, 3, 4), (2, 4, 5)]),
        (lambda a: a.sum(), [(3, 4)]),
        (lambda a: a.sum(axis=0), [(3, 4)]),
        (lambda a: a.sum(axis=1, keepdims=True), [(3, 4)]),
        (lambda a: a.mean(axis=-1), [(2, 3, 4)]),
        (lambda a: a.var(-1), [(3, 5)]),
        (lambda a: a.reshape(4, 3), [(2, 6)]),
        (lambda a: a.transpose(), [(2, 3, 4)]),
        (lambda a: a.transpose(1, 0, 2), [(2, 3, 4)]),
        (lambda a: a.T, [(3, 4)]),
        (lambda a: a[1:, ::2], [(4, 5)]),
        (lambda a: a.tanh(), [(3, 4)]),
        (lambda a: a.sigmoid(), [(3, 4)]),
        (lambda a: a.exp(), [(3, 4)]),
        (lambda a: a.softmax(-1), [(3, 5)]),
        (lambda a: a.softmax(0), [(3, 5)]),
        (lambda a: a.log_softmax(-1), [(2, 3, 5)]),
        (lambda a, b, c: concat([a, b, c], axis=1), [(2, 3), (2, 1), (2, 4)]),
        (lambda a: a * a + a, [(3,)]),  # el mismo tensor dos veces: se suman los gradientes
    ],
)
def test_gradiente(f: Callable[..., Tensor], shapes: list[tuple[int, ...]]) -> None:
    gradcheck(f, *shapes)


@pytest.mark.parametrize(
    ("f", "shapes"),
    [
        (lambda a, b: a / b, [(3, 4), (4,)]),
        (lambda a: a.log(), [(3, 4)]),
        (lambda a: a**0.5, [(3, 4)]),
        (lambda a: a**-1.0, [(3,)]),
    ],
)
def test_gradiente_con_positivos(f: Callable[..., Tensor], shapes: list[tuple[int, ...]]) -> None:
    gradcheck(f, *shapes, positive=True)


def test_relu_deja_pasar_solo_lo_positivo() -> None:
    x = tensor([-2.0, -0.5, 0.5, 3.0], requires_grad=True)
    x.relu().sum().backward()
    assert x.grad is not None
    np.testing.assert_array_equal(x.grad, [0, 0, 1, 1])


def test_indices_repetidos_suman_el_gradiente() -> None:
    # Como un embedding con la misma palabra dos veces en la frase.
    table = tensor(np.arange(12).reshape(4, 3), requires_grad=True)
    table[np.array([1, 3, 1])].sum().backward()
    assert table.grad is not None
    np.testing.assert_array_equal(table.grad[:, 0], [0, 2, 0, 1])


def test_entropia_cruzada_contra_la_formula() -> None:
    logits = RNG.normal(size=(4, 5))
    targets = np.array([0, 3, 4, 1])
    p = np.exp(logits) / np.exp(logits).sum(1, keepdims=True)
    expected = -np.log(p[np.arange(4), targets]).mean()
    assert cross_entropy(Tensor(logits), targets).item() == pytest.approx(expected)
    gradcheck(lambda x: cross_entropy(x, targets), (4, 5))
    gradcheck(lambda x: cross_entropy(x, np.array([[1, 2], [0, 4]])), (2, 2, 5))


def test_softmax_estable_con_numeros_grandes() -> None:
    p = tensor([[1000.0, 1001.0, 1002.0]]).softmax()
    assert np.isfinite(p.data).all()
    assert p.data.sum() == pytest.approx(1.0)


def test_backward_sin_gradiente_solo_para_escalares() -> None:
    with pytest.raises(ValueError, match="escalar"):
        tensor([1.0, 2.0], requires_grad=True).backward()


def test_no_grad_no_arma_el_grafo() -> None:
    w = tensor([1.0, 2.0], requires_grad=True)
    with no_grad():
        y = w * 3.0
    assert not y.requires_grad
    assert y._parents == ()
    assert (w * 3.0).requires_grad  # afuera, sí


def test_grafo_profundo_sin_recursion() -> None:
    x = tensor(1.0, requires_grad=True)
    y = x
    for _ in range(5000):
        y = y * 1.0 + 0.0
    y.backward()
    assert x.grad == pytest.approx(1.0)


# --- Capas -------------------------------------------------------------------------------


def test_lineal_y_layernorm_tienen_buen_gradiente() -> None:
    layer = Linear(4, 3, rng=np.random.default_rng(0))
    norm = LayerNorm(3)
    w, b = layer.weight.data.astype(np.float64), layer.bias
    assert b is not None

    def f(x: Tensor) -> Tensor:
        return ((x @ Tensor(w) + Tensor(b.data.astype(np.float64))) - 0.1).tanh()

    gradcheck(f, (2, 4))
    gradcheck(lambda x: norm(x), (2, 3))
    out = norm(tensor(RNG.normal(size=(5, 3)) * 10 + 7))
    np.testing.assert_allclose(out.data.mean(-1), 0, atol=1e-5)
    np.testing.assert_allclose(out.data.std(-1), 1, atol=1e-3)


def test_parametros_con_nombre() -> None:
    net = mlp([2, 8, 3], rng=np.random.default_rng(0))
    names = [n for n, _ in net.named_parameters()]
    assert names == ["layers.0.weight", "layers.0.bias", "layers.2.weight", "layers.2.bias"]
    assert net.num_parameters() == 2 * 8 + 8 + 8 * 3 + 3


def test_guardar_y_cargar_los_pesos(tmp_path: Path) -> None:
    a = mlp([3, 5, 2], rng=np.random.default_rng(1))
    b = mlp([3, 5, 2], rng=np.random.default_rng(2))
    x = tensor(RNG.normal(size=(4, 3)))
    a.save(tmp_path / "pesos.npz")
    b.load(tmp_path / "pesos.npz")
    np.testing.assert_array_equal(a(x).data, b(x).data)
    with pytest.raises(ValueError, match="no son de esta red"):
        mlp([3, 5, 5, 2]).load(tmp_path / "pesos.npz")


def test_embedding_busca_y_aprende() -> None:
    emb = Embedding(10, 4, rng=np.random.default_rng(0))
    out = emb(np.array([[1, 2], [2, 9]]))
    assert out.shape == (2, 2, 4)
    out.sum().backward()
    assert emb.weight.grad is not None
    assert emb.weight.grad[2].sum() == pytest.approx(8.0)  # la fila 2, usada dos veces
    assert emb.weight.grad[0].sum() == 0


# --- Aprender ------------------------------------------------------------------------------


def test_aprende_xor() -> None:
    """XOR no se separa con una recta: hace falta la capa oculta (el ejemplo clásico)."""
    x = tensor([[0, 0], [0, 1], [1, 0], [1, 1]])
    y = np.array([0, 1, 1, 0])
    net = mlp([2, 16, 2], activation=Tensor.tanh, rng=np.random.default_rng(3))
    opt = AdamW(net.parameters(), lr=0.05)
    for _ in range(300):
        loss = cross_entropy(net(x), y)
        opt.zero_grad()
        loss.backward()
        opt.step()
    assert loss.item() < 0.05
    np.testing.assert_array_equal(net(x).data.argmax(1), y)


def test_sgd_ajusta_una_recta() -> None:
    xs = RNG.uniform(-1, 1, size=(64, 1))
    ys = 3 * xs - 0.5
    layer = Linear(1, 1, rng=np.random.default_rng(0))
    opt = SGD(layer.parameters(), lr=0.1, momentum=0.9)
    for _ in range(200):
        err = layer(tensor(xs)) - tensor(ys)
        loss = (err * err).mean()
        opt.zero_grad()
        loss.backward()
        opt.step()
    assert layer.weight.item() == pytest.approx(3.0, abs=1e-2)
    assert layer.bias is not None
    assert layer.bias.item() == pytest.approx(-0.5, abs=1e-2)


def test_clasifica_espirales() -> None:
    """Tres espirales entrelazadas: un problema no lineal de verdad, en minilotes."""
    rng = np.random.default_rng(7)
    n = 100
    xs, ys = [], []
    for c in range(3):
        r = np.linspace(0.05, 1, n)
        t = np.linspace(c * 4, (c + 1) * 4, n) + rng.normal(0, 0.2, n)
        xs.append(np.c_[r * np.sin(t), r * np.cos(t)])
        ys.append(np.full(n, c))
    x, y = np.concatenate(xs), np.concatenate(ys)
    net = mlp([2, 64, 64, 3], rng=rng)
    opt = AdamW(net.parameters(), lr=0.01, weight_decay=1e-4)
    for _ in range(400):
        batch = rng.choice(len(x), 64, replace=False)
        loss = cross_entropy(net(tensor(x[batch])), y[batch])
        opt.zero_grad()
        loss.backward()
        clip_grad_norm(net.parameters(), 5.0)
        opt.step()
    with no_grad():
        accuracy = (net(tensor(x)).data.argmax(1) == y).mean()
    assert accuracy > 0.95


def test_recortar_el_gradiente() -> None:
    w = tensor([3.0, 4.0], requires_grad=True)
    (w * w * 0.5).sum().backward()  # gradiente = (3, 4), largo 5
    assert clip_grad_norm([w], 1.0) == pytest.approx(5.0)
    assert w.grad is not None
    assert np.linalg.norm(w.grad) == pytest.approx(1.0, abs=1e-4)
