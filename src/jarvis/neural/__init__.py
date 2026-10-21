"""El cerebro propio de JARVIS (ADR 0019): redes neuronales hechas desde cero, sin modelos de
terceros. Esta etapa (C1, `docs/cerebro-propio.md`) es el motor: tensores con autograd, capas
y optimizadores, sobre NumPy."""

from jarvis.neural.nn import Embedding, LayerNorm, Linear, Module, Sequential, mlp
from jarvis.neural.optim import SGD, AdamW, clip_grad_norm
from jarvis.neural.tensor import Tensor, concat, cross_entropy, no_grad, tensor

__all__ = [
    "SGD",
    "AdamW",
    "Embedding",
    "LayerNorm",
    "Linear",
    "Module",
    "Sequential",
    "Tensor",
    "clip_grad_norm",
    "concat",
    "cross_entropy",
    "mlp",
    "no_grad",
    "tensor",
]
