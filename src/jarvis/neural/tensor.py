"""Tensores con diferenciación automática (autograd en modo reverso), sobre NumPy.

Es la base del cerebro propio (ADR 0019): toda red de JARVIS (el entendedor de órdenes, el
modelo de lenguaje) se arma con estas operaciones y aprende con `backward()`.

Cómo funciona (lo no obvio):

- Cada operación devuelve un `Tensor` nuevo que recuerda a sus **padres** y una función que,
  dado el gradiente de la salida, calcula el de cada padre (la regla de la cadena de esa
  operación). Así se arma un grafo mientras se calcula hacia adelante.
- `backward()` recorre ese grafo al revés, en orden topológico, y **acumula** los gradientes:
  si un tensor se usa dos veces, recibe la suma de las dos contribuciones.
- NumPy "difunde" (broadcasting): sumar (B, n) + (n,) repite el segundo B veces. Hacia atrás,
  el gradiente del que se repitió es la suma sobre esas repeticiones (`_unbroadcast`).
- Las operaciones que se usan juntas siempre (softmax, log-softmax, entropía cruzada) van
  fusionadas: hacia atrás tienen una fórmula cerrada y numéricamente estable.
"""

from __future__ import annotations

import contextlib
from collections.abc import Callable, Iterator, Sequence
from typing import Any

import numpy as np
from numpy.typing import ArrayLike, NDArray

Array = NDArray[Any]
#: Dado el gradiente de la salida, el de cada padre (None = ese padre no necesita gradiente).
Backward = Callable[[Array], Sequence[Array | None]]

#: El tipo por defecto: float32 alcanza para entrenar y usa la mitad de memoria que float64.
DTYPE = np.float32

_grad_enabled = True


@contextlib.contextmanager
def no_grad() -> Iterator[None]:
    """Sin armar el grafo (para usar una red ya entrenada: más rápido y sin memoria extra)."""
    global _grad_enabled
    before = _grad_enabled
    _grad_enabled = False
    try:
        yield
    finally:
        _grad_enabled = before


def _unbroadcast(grad: Array, shape: tuple[int, ...]) -> Array:
    """El gradiente de un operando que NumPy difundió hasta la forma de `grad`."""
    # Las dimensiones que se agregaron adelante.
    while grad.ndim > len(shape):
        grad = grad.sum(axis=0)
    # Las de tamaño 1 que se estiraron.
    for axis, size in enumerate(shape):
        if size == 1 and grad.shape[axis] != 1:
            grad = grad.sum(axis=axis, keepdims=True)
    return grad


def _as_array(data: ArrayLike) -> Array:
    arr = np.asarray(data)
    # Los enteros y booleanos se pasan a flotante: un tensor que aprende es siempre real.
    return arr if np.issubdtype(arr.dtype, np.floating) else arr.astype(DTYPE)


class Tensor:
    __slots__ = ("_backward", "_parents", "data", "grad", "op", "requires_grad")

    def __init__(
        self,
        data: ArrayLike,
        requires_grad: bool = False,
        _parents: tuple[Tensor, ...] = (),
        _backward: Backward | None = None,
        op: str = "",
    ) -> None:
        self.data: Array = _as_array(data)
        self.grad: Array | None = None
        self.requires_grad = requires_grad
        self._parents = _parents
        self._backward = _backward
        #: El nombre de la operación que lo creó (para depurar).
        self.op = op

    # --- Lo básico ---------------------------------------------------------------------------

    @property
    def shape(self) -> tuple[int, ...]:
        return tuple(self.data.shape)

    @property
    def ndim(self) -> int:
        return int(self.data.ndim)

    def item(self) -> float:
        if self.data.size != 1:
            raise ValueError(f"item() de un tensor de forma {self.shape}")
        return float(self.data.reshape(-1)[0])

    def numpy(self) -> Array:
        return self.data

    def detach(self) -> Tensor:
        """Los mismos valores, fuera del grafo."""
        return Tensor(self.data)

    def __repr__(self) -> str:
        grad = ", requires_grad=True" if self.requires_grad else ""
        return f"Tensor({self.data!r}{grad})"

    def __len__(self) -> int:
        return len(self.data)

    @staticmethod
    def _make(data: Array, parents: tuple[Tensor, ...], backward: Backward, op: str) -> Tensor:
        """El resultado de una operación: con grafo solo si algún padre necesita gradiente."""
        if _grad_enabled and any(p.requires_grad for p in parents):
            return Tensor(data, True, parents, backward, op)
        return Tensor(data)

    # --- Hacia atrás -------------------------------------------------------------------------

    def backward(self, grad: ArrayLike | None = None) -> None:
        """Calcula el gradiente de este tensor respecto de todos los que lo produjeron.

        Sin `grad`, este tensor tiene que ser un escalar (la pérdida) y su gradiente es 1."""
        if grad is None:
            if self.data.size != 1:
                raise ValueError("backward() sin gradiente solo para un escalar (la pérdida)")
            seed = np.ones_like(self.data)
        else:
            seed = np.asarray(grad, dtype=self.data.dtype).reshape(self.data.shape)

        # Orden topológico, iterativo (una red profunda pasaría el límite de recursión).
        order: list[Tensor] = []
        seen: set[int] = set()
        stack: list[tuple[Tensor, bool]] = [(self, False)]
        while stack:
            node, expanded = stack.pop()
            if expanded:
                order.append(node)
                continue
            if id(node) in seen:
                continue
            seen.add(id(node))
            stack.append((node, True))
            for parent in node._parents:
                if parent.requires_grad and id(parent) not in seen:
                    stack.append((parent, False))

        grads: dict[int, Array] = {id(self): seed}
        for node in reversed(order):
            g = grads.pop(id(node), None)
            if g is None:
                continue
            if node._backward is None:
                # Una hoja (un parámetro o una entrada): acá queda el gradiente.
                node.grad = g if node.grad is None else node.grad + g
                continue
            for parent, pg in zip(node._parents, node._backward(g), strict=True):
                if pg is None or not parent.requires_grad:
                    continue
                prev = grads.get(id(parent))
                grads[id(parent)] = pg if prev is None else prev + pg

    # --- Aritmética --------------------------------------------------------------------------

    def __add__(self, other: Tensor | ArrayLike) -> Tensor:
        o = lift(other)
        a, b = self.shape, o.shape
        return Tensor._make(
            self.data + o.data,
            (self, o),
            lambda g: (_unbroadcast(g, a), _unbroadcast(g, b)),
            "+",
        )

    def __radd__(self, other: ArrayLike) -> Tensor:
        return self + other

    def __neg__(self) -> Tensor:
        return Tensor._make(-self.data, (self,), lambda g: (-g,), "neg")

    def __sub__(self, other: Tensor | ArrayLike) -> Tensor:
        return self + (-lift(other))

    def __rsub__(self, other: ArrayLike) -> Tensor:
        return lift(other) + (-self)

    def __mul__(self, other: Tensor | ArrayLike) -> Tensor:
        o = lift(other)
        x, y = self.data, o.data
        return Tensor._make(
            x * y,
            (self, o),
            lambda g: (_unbroadcast(g * y, x.shape), _unbroadcast(g * x, y.shape)),
            "*",
        )

    def __rmul__(self, other: ArrayLike) -> Tensor:
        return self * other

    def __truediv__(self, other: Tensor | ArrayLike) -> Tensor:
        o = lift(other)
        x, y = self.data, o.data
        return Tensor._make(
            x / y,
            (self, o),
            lambda g: (
                _unbroadcast(g / y, x.shape),
                _unbroadcast(-g * x / (y * y), y.shape),
            ),
            "/",
        )

    def __rtruediv__(self, other: ArrayLike) -> Tensor:
        return lift(other) / self

    def __pow__(self, exponent: float) -> Tensor:
        """Solo con exponente constante (lo que usan las redes: x², √x, 1/x)."""
        x = self.data
        return Tensor._make(
            x**exponent, (self,), lambda g: (g * exponent * x ** (exponent - 1),), "**"
        )

    def __matmul__(self, other: Tensor) -> Tensor:
        """Producto de matrices, también por lotes: (..., n, k) @ (..., k, m)."""
        if self.ndim < 2 or other.ndim < 2:
            raise ValueError("matmul necesita al menos 2 dimensiones de cada lado")
        x, y = self.data, other.data
        return Tensor._make(
            x @ y,
            (self, other),
            lambda g: (
                _unbroadcast(g @ np.swapaxes(y, -1, -2), x.shape),
                _unbroadcast(np.swapaxes(x, -1, -2) @ g, y.shape),
            ),
            "@",
        )

    # --- Reducciones -------------------------------------------------------------------------

    def sum(self, axis: int | tuple[int, ...] | None = None, keepdims: bool = False) -> Tensor:
        shape = self.shape

        def backward(g: Array) -> tuple[Array]:
            if axis is not None and not keepdims:
                g = np.expand_dims(g, axis)
            return (np.broadcast_to(g, shape).copy(),)

        out = self.data.sum(axis=axis, keepdims=True) if keepdims else self.data.sum(axis=axis)
        return Tensor._make(np.asarray(out), (self,), backward, "sum")

    def mean(self, axis: int | tuple[int, ...] | None = None, keepdims: bool = False) -> Tensor:
        n = self.data.size if axis is None else int(np.prod([self.shape[a] for a in _axes(axis)]))
        return self.sum(axis, keepdims) * (1.0 / n)

    def var(self, axis: int = -1, keepdims: bool = False) -> Tensor:
        """Varianza (dividida por n, como en la normalización de capas)."""
        centered = self - self.mean(axis, keepdims=True)
        return (centered * centered).mean(axis, keepdims)

    # --- Forma -------------------------------------------------------------------------------

    def reshape(self, *shape: int) -> Tensor:
        old = self.shape
        return Tensor._make(
            self.data.reshape(shape), (self,), lambda g: (g.reshape(old),), "reshape"
        )

    def transpose(self, *axes: int) -> Tensor:
        """Sin ejes, invierte el orden de todos (como NumPy)."""
        perm = axes or tuple(reversed(range(self.ndim)))
        inverse = tuple(np.argsort(perm))
        return Tensor._make(
            self.data.transpose(perm), (self,), lambda g: (g.transpose(inverse),), "transpose"
        )

    def swapaxes(self, a: int, b: int) -> Tensor:
        return Tensor._make(
            np.swapaxes(self.data, a, b), (self,), lambda g: (np.swapaxes(g, a, b),), "swapaxes"
        )

    @property
    def T(self) -> Tensor:
        return self.swapaxes(-1, -2)

    def __getitem__(self, index: Any) -> Tensor:
        """Recortes e índices (también con arreglos de enteros: así se busca un embedding)."""
        shape, dtype = self.shape, self.data.dtype

        def backward(g: Array) -> tuple[Array]:
            full = np.zeros(shape, dtype=dtype)
            # `add.at` suma bien cuando un mismo índice aparece varias veces (una palabra
            # repetida en la frase): `full[index] += g` se quedaría con una sola.
            np.add.at(full, index, g)
            return (full,)

        return Tensor._make(self.data[index], (self,), backward, "getitem")

    # --- Funciones elemento a elemento -------------------------------------------------------

    def exp(self) -> Tensor:
        out = np.exp(self.data)
        return Tensor._make(out, (self,), lambda g: (g * out,), "exp")

    def log(self) -> Tensor:
        x = self.data
        return Tensor._make(np.log(x), (self,), lambda g: (g / x,), "log")

    def tanh(self) -> Tensor:
        out = np.tanh(self.data)
        return Tensor._make(out, (self,), lambda g: (g * (1 - out * out),), "tanh")

    def sigmoid(self) -> Tensor:
        out = 1 / (1 + np.exp(-self.data))
        return Tensor._make(out, (self,), lambda g: (g * out * (1 - out),), "sigmoid")

    def relu(self) -> Tensor:
        mask = self.data > 0
        return Tensor._make(self.data * mask, (self,), lambda g: (g * mask,), "relu")

    # --- Probabilidades (fusionadas) ---------------------------------------------------------

    def softmax(self, axis: int = -1) -> Tensor:
        # Restar el máximo no cambia el resultado y evita que exp() se vaya a infinito.
        e = np.exp(self.data - self.data.max(axis=axis, keepdims=True))
        p = e / e.sum(axis=axis, keepdims=True)
        # d softmax: p * (g - <g, p>)
        return Tensor._make(
            p, (self,), lambda g: (p * (g - (g * p).sum(axis=axis, keepdims=True)),), "softmax"
        )

    def log_softmax(self, axis: int = -1) -> Tensor:
        shifted = self.data - self.data.max(axis=axis, keepdims=True)
        out = shifted - np.log(np.exp(shifted).sum(axis=axis, keepdims=True))
        p = np.exp(out)
        return Tensor._make(
            out, (self,), lambda g: (g - p * g.sum(axis=axis, keepdims=True),), "log_softmax"
        )


def _axes(axis: int | tuple[int, ...]) -> tuple[int, ...]:
    return (axis,) if isinstance(axis, int) else axis


def lift(value: Tensor | ArrayLike) -> Tensor:
    """Un número o un arreglo como tensor constante (sin gradiente)."""
    return value if isinstance(value, Tensor) else Tensor(value)


def tensor(data: ArrayLike, requires_grad: bool = False) -> Tensor:
    """Un tensor de float32 (lo normal para los pesos y las entradas)."""
    return Tensor(np.asarray(data, dtype=DTYPE), requires_grad)


def cross_entropy(logits: Tensor, targets: ArrayLike) -> Tensor:
    """La pérdida de clasificación: el promedio de -log p(clase correcta).

    `logits` es (N, C) (o (..., C), se aplana); `targets`, los índices de las clases (N,).
    Fusionada: su gradiente es (softmax - one_hot) / N, sin pasar por log(softmax)."""
    flat = logits.data.reshape(-1, logits.shape[-1])
    idx = np.asarray(targets).reshape(-1)
    if idx.shape[0] != flat.shape[0]:
        raise ValueError(f"{flat.shape[0]} predicciones y {idx.shape[0]} respuestas")
    n = flat.shape[0]
    shifted = flat - flat.max(axis=1, keepdims=True)
    log_p = shifted - np.log(np.exp(shifted).sum(axis=1, keepdims=True))
    rows = np.arange(n)
    loss = -log_p[rows, idx].mean()
    shape = logits.shape

    def backward(g: Array) -> tuple[Array]:
        d = np.exp(log_p)
        d[rows, idx] -= 1
        return ((d * (g / n)).reshape(shape).astype(logits.data.dtype),)

    return Tensor._make(np.asarray(loss, dtype=logits.data.dtype), (logits,), backward, "xent")


def concat(tensors: Sequence[Tensor], axis: int = -1) -> Tensor:
    """Pega tensores a lo largo de un eje."""
    sizes = [t.shape[axis] for t in tensors]
    cuts = np.cumsum(sizes)[:-1]
    return Tensor._make(
        np.concatenate([t.data for t in tensors], axis=axis),
        tuple(tensors),
        lambda g: tuple(np.split(g, cuts, axis=axis)),
        "concat",
    )
