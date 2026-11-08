"""El entendedor de órdenes propio de JARVIS (C2-C3 de `docs/cerebro-propio.md`, ADR 0019).

Lee una frase ("abrime la música", "qué hora es", "buscá recetas de empanadas") y dice qué se
pide (la **intención**) y con qué datos (los **argumentos** de la tool), o se **abstiene** si no
está seguro. No autoriza ni ejecuta nada: si la intención es una tool, la acción pasa por el
mismo `Gate` que las del modelo de lenguaje, con los mismos niveles de permiso.

- `intents`: el catálogo de intenciones y a qué tool corresponde cada una.
- `text`: normalización y n-gramas de caracteres (aguantan errores de tipeo y de transcripción).
- `dataset`: el corpus generado con plantillas en castellano rioplatense, partido por plantilla.
- `slots`: los datos de cada orden (app, ruta, consulta…), con reglas y diccionarios.
- `model`: la red chica (motor propio de `jarvis.neural`), con temperatura y umbral calibrados.
- `baseline`: las reglas de hoy, para comparar.
- `evaluate`: las métricas (F1 por intención, datos, abstención, latencia, cobertura local).
- `router`: el cerebro "local primero" (o en sombra) que envuelve al de siempre.
- `telemetry`: el registro por pedido de C0 (qué llegó, quién contestó, cuánto tardó).
"""
