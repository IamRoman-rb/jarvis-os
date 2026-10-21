"""Optimizadores: cómo se mueven los pesos con el gradiente para que la pérdida baje.

- `SGD`: paso = -lr · gradiente (con momento: una "inercia" que suaviza el camino).
- `AdamW`: cada peso con su propio tamaño de paso, según la media y la varianza de sus
  gradientes recientes; es el que se usa para los transformers. El decaimiento de pesos va
  aparte del gradiente (la "W"), que es lo que mejor generaliza.
"""

from __future__ import annotations

import math
from collections.abc import Sequence

import numpy as np

from jarvis.neural.tensor import Array, Tensor


def clip_grad_norm(params: Sequence[Tensor], max_norm: float) -> float:
    """Si el gradiente (todos juntos) es más largo que `max_norm`, lo achica a ese largo.
    Evita que un lote raro dé un salto enorme. Devuelve el largo antes de recortar."""
    total = math.sqrt(sum(float((p.grad * p.grad).sum()) for p in params if p.grad is not None))
    if total > max_norm > 0:
        scale = max_norm / (total + 1e-6)
        for p in params:
            if p.grad is not None:
                p.grad = p.grad * scale
    return total


class SGD:
    def __init__(self, params: Sequence[Tensor], lr: float, momentum: float = 0.0) -> None:
        self.params = list(params)
        self.lr = lr
        self.momentum = momentum
        self._velocity: list[Array | None] = [None] * len(self.params)

    def step(self) -> None:
        for i, p in enumerate(self.params):
            if p.grad is None:
                continue
            v = p.grad
            if self.momentum:
                prev = self._velocity[i]
                v = v if prev is None else self.momentum * prev + v
                self._velocity[i] = v
            # Sin cambiar el tipo: un gradiente en float64 no tiene que agrandar los pesos.
            p.data = (p.data - self.lr * v).astype(p.data.dtype)

    def zero_grad(self) -> None:
        for p in self.params:
            p.grad = None


class AdamW:
    def __init__(
        self,
        params: Sequence[Tensor],
        lr: float = 1e-3,
        betas: tuple[float, float] = (0.9, 0.999),
        eps: float = 1e-8,
        weight_decay: float = 0.0,
    ) -> None:
        self.params = list(params)
        self.lr = lr
        self.b1, self.b2 = betas
        self.eps = eps
        self.weight_decay = weight_decay
        self.t = 0
        self._m = [np.zeros_like(p.data) for p in self.params]
        self._v = [np.zeros_like(p.data) for p in self.params]

    def step(self) -> None:
        self.t += 1
        # Al principio m y v arrancan en 0 y subestiman: se corrigen dividiendo por (1 - βᵗ).
        c1 = 1 - self.b1**self.t
        c2 = 1 - self.b2**self.t
        for i, p in enumerate(self.params):
            if p.grad is None:
                continue
            g = p.grad
            self._m[i] = self.b1 * self._m[i] + (1 - self.b1) * g
            self._v[i] = self.b2 * self._v[i] + (1 - self.b2) * g * g
            update = (self._m[i] / c1) / (np.sqrt(self._v[i] / c2) + self.eps)
            if self.weight_decay and p.ndim >= 2:
                # Solo a las matrices: los sesgos y las ganancias no se achican.
                update = update + self.weight_decay * p.data
            p.data = (p.data - self.lr * update).astype(p.data.dtype)

    def zero_grad(self) -> None:
        for p in self.params:
            p.grad = None
