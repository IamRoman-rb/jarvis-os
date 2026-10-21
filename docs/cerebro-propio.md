# El cerebro propio de JARVIS: metas

La decisión está en el [ADR 0019](adr/0019-cerebro-propio.md). Acá van las metas en orden, cada
una con qué es, por qué va en ese lugar y cuándo está **lista** (un criterio que se puede medir).
Una meta a la vez, como los hitos del kernel (`docs/kernel.md`).

Código: `src/jarvis/neural/` (Python, sobre NumPy). Tests: `tests/unit/test_neural*.py`.

| Meta | Qué | Estado |
|------|-----|--------|
| C0 | Diagnóstico: por qué hoy JARVIS entiende mal | pendiente |
| C1 | Motor de redes neuronales: tensores, autograd, capas, optimizadores | **lista** |
| C2 | Datos: el corpus de órdenes de JARVIS | pendiente |
| C3 | El entendedor de órdenes (NLU): intención + datos | pendiente |
| C4 | Tokenizador propio (BPE) | pendiente |
| C5 | Modelo de lenguaje propio: un transformer tipo GPT, preentrenado | pendiente |
| C6 | Ajuste a conversación y a usar tools | pendiente |
| C7 | Conectado al cerebro: modo propio / propio primero / externos | pendiente |
| C8 | Memoria semántica con embeddings propios | pendiente |
| C9 | Voz propia (palabra de activación, voz → texto) | pendiente |
| C10 | El entendedor de órdenes adentro del kernel (Rust, sin anfitrión) | pendiente |

## C0. Diagnóstico

Antes de reemplazar nada, saber qué falla hoy cuando "no entiende": ¿la voz transcribe mal
(faster-whisper `small`)? ¿Contesta otro agente porque Claude se quedó sin uso? ¿La consola
del kernel toma como orden local algo que era para el cerebro?

- **Lista cuando:** `cerebro.log` registra, por pedido, qué texto llegó (lo transcripto), qué
  agente contestó y si hubo cambio de agente; y hay una lista de 20 frases reales de Roman con
  dónde falló cada una. Esa lista es el primer conjunto de prueba de C3.

## C1. Motor de redes neuronales ✅

`neural/tensor.py` (tensores con autograd en modo reverso: suma, producto, matmul por lotes,
reducciones, forma, índices, exp/log/tanh/sigmoide/ReLU, softmax y entropía cruzada
fusionadas), `neural/nn.py` (`Module`, `Linear`, `Embedding`, `LayerNorm`, `Sequential`,
`mlp`, guardar y cargar pesos en `.npz` sin pickle), `neural/optim.py` (`SGD` con momento,
`AdamW`, recorte del gradiente).

- **Lista cuando:** cada operación pasa el chequeo contra el gradiente numérico; una red
  aprende XOR, una recta y tres espirales entrelazadas (>95 %). ✅ (45 tests)

Lo que se aprendió:
- Hacia atrás, un tensor usado dos veces suma los gradientes de los dos usos; un índice repetido
  (la misma palabra dos veces) necesita `np.add.at`, porque `x[i] += g` se queda con uno solo.
- El broadcasting de NumPy tiene su reverso: el gradiente del operando que se estiró es la suma
  sobre las copias (`_unbroadcast`).
- Softmax y entropía cruzada se fusionan: el gradiente es `softmax − one_hot`, estable aunque
  los logits sean enormes (se resta el máximo antes de `exp`).
- El orden topológico va iterativo: un grafo de miles de operaciones pasaría el límite de
  recursión de Python.

## C2. Datos: el corpus de órdenes

Una red aprende de ejemplos. Para las órdenes, JARVIS necesita miles de frases rotuladas.

- Las intenciones salen de las tools de JARVIS (`tools/system.py`, con sus niveles de
  `docs/permisos.md`) más charla ("hola", "gracias", "qué hora es") y "fuera de lugar" (lo que
  no es una orden y va al modelo de lenguaje).
- Un generador con plantillas en castellano rioplatense (voseo, sinónimos, errores de tipeo y
  de transcripción de voz, con y sin "JARVIS"), con los datos marcados (`abrí [app:el
  navegador]`).
- Las frases reales de Roman (de C0 y de la memoria, con su OK) solo para probar, nunca para
  entrenar: así la medición es honesta.
- **Lista cuando:** ≥ 5000 ejemplos de entrenamiento, ≥ 30 intenciones, un conjunto de prueba
  separado y un test que verifica que las dos partes no se pisan.

## C3. El entendedor de órdenes (NLU)

Una red que lee la frase y dice qué se pide y con qué datos. Primero con letras y pedazos de
palabras (n-gramas de caracteres: aguanta los errores de tipeo y de transcripción) → embedding →
una red chica con dos salidas: la intención (una clase) y, por cada palabra, si es parte de un
dato (etiquetas BIO: `B-app`, `I-app`, `O`).

- Con confianza baja dice "no estoy seguro" y la frase va al modelo de lenguaje (o a un agente
  externo), en vez de hacer cualquier cosa.
- **Lista cuando:** ≥ 95 % de intenciones correctas en el conjunto de prueba, ≥ 90 % de las
  frases reales de C0, < 20 ms por frase en esta PC, y los tests de política y de prompt
  injection siguen pasando (una frase no puede saltarse el `Gate`).

## C4. Tokenizador propio (BPE)

El modelo de lenguaje no lee letras sueltas ni palabras enteras: lee pedazos ("abr", "í",
" naveg", "ador"). Byte-Pair Encoding: arranca con los 256 bytes y va uniendo los pares más
frecuentes del corpus hasta tener el vocabulario (unos 8000 pedazos).

- **Lista cuando:** codificar y decodificar es la identidad para cualquier texto (también
  emojis y acentos); en castellano, ≤ 1,6 pedazos por palabra en promedio.

## C5. Modelo de lenguaje propio (preentrenamiento)

Un transformer decodificador, como GPT, escrito con el motor de C1: embeddings de pedazos y de
posición, bloques de atención causal con varias cabezas, MLP con GELU, normalización de capas y
conexiones residuales. Aprende a predecir el pedazo siguiente.

- Corpus: castellano de dominio público o licencia libre (libros del Proyecto Gutenberg,
  Wikipedia en español con CC BY-SA, citando la fuente), unos cientos de MB.
- Tamaño para esta PC: 5–30 M de parámetros, contexto de 256–512 pedazos. Entrenamiento por
  partes con checkpoints (se puede cortar y seguir).
- Antes del corpus grande: el mismo modelo tiene que aprender a repetir un texto chico casi de
  memoria (si no, hay un error).
- **Lista cuando:** la perplejidad en texto que no vio baja de forma sostenida y genera frases
  gramaticales en castellano; hay una muestra de textos generados en el doc.

## C6. Ajuste a conversación y a tools

El modelo preentrenado sabe continuar texto; hay que enseñarle a **contestar**: ejemplos de
pedido → respuesta con el tono de JARVIS, y pedido → llamada a una tool (el formato de C7).

- **Lista cuando:** en un conjunto de 100 pedidos de charla contesta algo pertinente en ≥ 70 %
  (lo evalúa Roman) y nunca inventa una tool que no existe.

## C7. Conectado al cerebro

Un `OwnBrain` al lado de `ClaudeBrain` (`agent/brain.py`): primero el entendedor de órdenes; si
es una orden con confianza alta, la tool por el `Gate`; si no, el modelo de lenguaje. En
Configuración → Asistente, el modo: **propio** (sin red ni terceros), **propio primero** (lo que
no sabe lo pasa a un externo vinculado) o **externos** (como hoy). Va a `docs/permisos.md` y a
`policy/levels.py` como cualquier agente.

- **Lista cuando:** en modo propio, `cargo xtask test` pasa sin red y sin `--simulado` para las
  órdenes del guion, y se puede usar JARVIS por voz sin ningún modelo externo de lenguaje.

## C8. Memoria semántica

Hoy `buscar_memoria` busca palabras. Con los embeddings de C3/C5, cada recuerdo tiene un vector
y se busca por significado ("lo del kernel de ayer" encuentra "K15: vino corre Python").

- **Lista cuando:** en 30 consultas armadas, el recuerdo correcto está entre los 3 primeros en
  ≥ 80 %.

## C9. Voz propia

La voz usa modelos locales de terceros. Primero la palabra de activación ("JARVIS": una red chica
sobre el espectrograma, entrenada con grabaciones de Roman), después voz → texto para órdenes
cortas. La síntesis de voz queda para el final.

- **Lista cuando:** la palabra de activación propia reemplaza a openWakeWord con ≤ 1 falsa
  activación por hora y ≥ 95 % de detección.

## C10. Adentro del kernel

El entendedor de órdenes es chico: sus pesos (unos MB) y la pasada hacia adelante se pueden
llevar a una crate `no_std` en Rust (punto fijo, como pide la regla 5 del kernel). Así JARVIS-OS
entiende órdenes en una PC real sin el anfitrión (hoy, sin él, solo las órdenes locales; ADR 0008).

- **Lista cuando:** la crate da las mismas intenciones que la versión en Python en todo el
  conjunto de prueba, con tests en el host, y la consola del kernel la usa.
