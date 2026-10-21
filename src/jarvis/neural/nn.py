"""Capas de redes neuronales armadas con `Tensor`: lineal, embedding, normalización, MLP.

Un `Module` es cualquier cosa con parámetros (tensores que aprenden). `parameters()` los
encuentra solos, recorriendo los atributos, así el optimizador sabe qué ajustar y `save`/`load`
qué guardar.
"""

from __future__ import annotations

import itertools
import math
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import numpy as np

from jarvis.neural.tensor import DTYPE, Array, Tensor


def _rng(rng: np.random.Generator | None) -> np.random.Generator:
    return rng if rng is not None else np.random.default_rng()


def parameter(data: Array) -> Tensor:
    return Tensor(data.astype(DTYPE), requires_grad=True)


class Module:
    """La base de las capas: sus parámetros y los de sus submódulos."""

    def forward(self, *args: Any) -> Tensor:
        raise NotImplementedError

    def __call__(self, *args: Any) -> Tensor:
        return self.forward(*args)

    def named_parameters(self, prefix: str = "") -> Iterator[tuple[str, Tensor]]:
        for name, value in vars(self).items():
            yield from _named(f"{prefix}{name}", value)

    def parameters(self) -> list[Tensor]:
        return [p for _, p in self.named_parameters()]

    def zero_grad(self) -> None:
        for p in self.parameters():
            p.grad = None

    def num_parameters(self) -> int:
        return sum(p.data.size for p in self.parameters())

    def state_dict(self) -> dict[str, Array]:
        return {name: p.data.copy() for name, p in self.named_parameters()}

    def load_state_dict(self, state: dict[str, Array]) -> None:
        params = dict(self.named_parameters())
        if set(params) != set(state):
            faltan = sorted(set(params) - set(state))
            sobran = sorted(set(state) - set(params))
            raise ValueError(f"los pesos no son de esta red (faltan {faltan}, sobran {sobran})")
        for name, p in params.items():
            if p.shape != state[name].shape:
                raise ValueError(f"{name}: forma {state[name].shape}, se esperaba {p.shape}")
            p.data = state[name].astype(p.data.dtype)

    def save(self, path: Path) -> None:
        """Los pesos en un `.npz` (un arreglo por parámetro, con su nombre)."""
        path.parent.mkdir(parents=True, exist_ok=True)
        # (Por `Any`: mypy cree que un nombre del diccionario podría ser `allow_pickle`.)
        savez: Any = np.savez
        savez(path, **self.state_dict())

    def load(self, path: Path) -> None:
        # allow_pickle=False: un archivo de pesos no puede ejecutar código al abrirlo.
        with np.load(path, allow_pickle=False) as data:
            self.load_state_dict({k: data[k] for k in data.files})


def _named(name: str, value: Any) -> Iterator[tuple[str, Tensor]]:
    if isinstance(value, Tensor):
        if value.requires_grad:
            yield name, value
    elif isinstance(value, Module):
        yield from value.named_parameters(f"{name}.")
    elif isinstance(value, list | tuple):
        for i, item in enumerate(value):
            yield from _named(f"{name}.{i}", item)


class Linear(Module):
    """y = x W + b. Los pesos arrancan al azar en ±1/√entradas (como PyTorch): así la salida
    tiene más o menos la misma escala que la entrada, capa tras capa."""

    def __init__(
        self, n_in: int, n_out: int, bias: bool = True, rng: np.random.Generator | None = None
    ) -> None:
        bound = 1 / math.sqrt(n_in)
        r = _rng(rng)
        self.weight = parameter(r.uniform(-bound, bound, (n_in, n_out)))
        self.bias = parameter(r.uniform(-bound, bound, (n_out,))) if bias else None

    def forward(self, x: Tensor) -> Tensor:
        y = x @ self.weight
        return y + self.bias if self.bias is not None else y


class Embedding(Module):
    """Una tabla: a cada símbolo (palabra, pedazo de palabra) le corresponde un vector que
    aprende. `forward` recibe los índices (enteros) y devuelve sus vectores."""

    def __init__(self, n: int, dim: int, rng: np.random.Generator | None = None) -> None:
        self.weight = parameter(_rng(rng).normal(0.0, 0.02, (n, dim)))

    def forward(self, idx: Array) -> Tensor:
        return self.weight[np.asarray(idx)]


class LayerNorm(Module):
    """Normaliza cada vector a media 0 y varianza 1 y después lo escala y corre con dos
    parámetros que aprenden. Estabiliza el entrenamiento de las redes profundas."""

    def __init__(self, dim: int, eps: float = 1e-5) -> None:
        self.gain = parameter(np.ones(dim))
        self.shift = parameter(np.zeros(dim))
        self.eps = eps

    def forward(self, x: Tensor) -> Tensor:
        centered = x - x.mean(-1, keepdims=True)
        std = (x.var(-1, keepdims=True) + self.eps) ** 0.5
        return centered / std * self.gain + self.shift


class Sequential(Module):
    """Capas (o funciones, como `Tensor.relu`) una detrás de otra."""

    def __init__(self, *layers: Module | Callable[[Tensor], Tensor]) -> None:
        self.layers = list(layers)

    def forward(self, x: Tensor) -> Tensor:
        for layer in self.layers:
            x = layer(x)
        return x


def mlp(
    sizes: list[int],
    activation: Callable[[Tensor], Tensor] = Tensor.relu,
    rng: np.random.Generator | None = None,
) -> Sequential:
    """Un perceptrón multicapa: lineal → activación → lineal → ... → lineal (sin activación al
    final: la salida son los logits)."""
    layers: list[Module | Callable[[Tensor], Tensor]] = []
    for i, (a, b) in enumerate(itertools.pairwise(sizes)):
        layers.append(Linear(a, b, rng=rng))
        if i < len(sizes) - 2:
            layers.append(activation)
    return Sequential(*layers)
