# ADR 0019: Un cerebro propio para JARVIS, hecho desde cero

- **Estado:** aceptada
- **Fecha:** 2026-10-21
- **Complementa:** [ADR 0008](0008-cerebro-en-el-anfitrion.md) (el cerebro en el anfitrión)

## Contexto

Hoy JARVIS entiende y contesta gracias a modelos de otros: Claude (Agent SDK), Gemini, ChatGPT
y DeepSeek (ADR 0008). Si se queda sin uso, sin red o un proveedor cambia algo, JARVIS deja de
entender. Roman quiere que JARVIS sea **independiente**: una red neuronal propia, con
procesamiento del lenguaje natural y un modelo de lenguaje propio, sin depender de Claude,
Gemini ni de otros modelos.

La PC donde se entrena es un Ryzen 5 5600GT (6 núcleos, 12 hilos), 23 GB de RAM y la gráfica
integrada (sin CUDA ni ROCm en Windows). Eso marca lo que se puede entrenar acá.

## Decisión

1. **Todo desde cero.** El motor de redes neuronales (`src/jarvis/neural/`: tensores con
   autograd, capas, optimizadores) es propio, sobre **NumPy** (solo álgebra lineal; ni PyTorch
   ni modelos preentrenados). Cada gradiente se verifica contra el numérico en los tests. Los
   pesos que aprende JARVIS son de JARVIS.
2. **Dos cerebros especializados en vez de uno gigante.**
   - **El entendedor de órdenes** (NLU): una red que clasifica la intención ("abrir app",
     "buscar en la web", "recordar"...) y saca los datos (qué app, qué buscar). Es lo que hace
     que JARVIS *entienda* lo que se le pide; es chica, se entrena en minutos en esta PC y
     contesta en milisegundos, sin red. Las acciones siguen pasando por el `Gate` con los
     niveles de `docs/permisos.md`: el cerebro propio no tiene más permisos que Claude.
   - **El modelo de lenguaje** (un transformer tipo GPT, propio): para conversar y redactar.
     Se entrena con texto en castellano de dominio público o con licencia libre, más las
     conversaciones de JARVIS.
3. **Se construye por metas** (`docs/cerebro-propio.md`), cada una con su criterio de "listo"
   medible y sus tests, como los hitos del kernel.
4. **Los agentes externos pasan a ser opcionales.** Cuando el cerebro propio esté conectado
   (meta C7), Roman elige el modo: *propio* (sin red, sin terceros), *propio primero* (lo que
   el propio no sabe, lo consulta a un externo vinculado) o como hoy.
5. **Los datos son datos.** Lo que aprende sale de corpus elegidos y de las conversaciones de
   Roman (con su OK); el texto de entrenamiento nunca se trata como instrucciones (regla 7 de
   seguridad del cerebro).

## Consecuencias

- (+) JARVIS entiende las órdenes sin red, sin cuenta y sin límite de uso.
- (+) Se aprende cómo funciona un LLM por dentro: autograd, atención, tokenización, entrenamiento.
- (+) Un motor chico y sin dependencias pesadas se puede llevar después a Rust, adentro del
  kernel (meta C10): JARVIS-OS entendiendo órdenes sin el anfitrión.
- (−) **Un modelo de lenguaje entrenado en esta PC no va a conversar como Claude.** Un GPT-2
  chico (124 M de parámetros) llevaría semanas de CPU; acá lo razonable son 5–30 M de
  parámetros: frases coherentes en castellano y respuestas cortas, no razonamiento largo. Por
  eso las órdenes las resuelve la red especializada, que sí puede ser muy buena. Si aparece una
  GPU, el mismo código escala.
- (−) NumPy en CPU es más lento que PyTorch; alcanza para los tamaños de arriba. Si el
  entrenamiento del modelo de lenguaje se vuelve el cuello de botella, se optimiza el motor
  (operaciones fusionadas, float32, lotes) antes de cambiarlo.
- (−) La voz sigue usando modelos locales de terceros (faster-whisper, Piper, openWakeWord).
  Reemplazarlos es la meta C9, la última.
