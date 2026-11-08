# Análisis profundo de JARVIS-OS

**Fecha:** 3 de noviembre de 2026  
**Alcance:** revisión estática del repositorio, documentación de arquitectura/ADR y búsqueda de proyectos públicos comparables. No se ejecutaron pruebas ni se verificó hardware físico durante este análisis.

## 1. Resumen ejecutivo

JARVIS-OS ya no es solamente una idea de distribución Linux con un asistente. El repositorio contiene dos líneas de producto relacionadas pero distintas:

1. Un **kernel y entorno de escritorio propios**, escrito mayormente en Rust `no_std`, que arranca en x86-64/UEFI y documenta hitos K0–K15: memoria, tareas, red, FAT32, interfaz gráfica, ABI Linux parcial, hardware, Wi-Fi y un ejecutor experimental de programas Windows llamado `vino`.
2. Un **servicio de asistente en Python** que conecta el kernel con Claude Agent SDK, varios proveedores/agentes, herramientas con niveles de permisos, voz local, memoria y agentes de codificación ejecutados en el anfitrión.

La arquitectura ya expresa una decisión importante: el kernel no integra Claude ni guarda sus credenciales. La IA corre en el anfitrión y el kernel usa un protocolo de mensajes. Esto reduce complejidad en el kernel y permite usar SDKs y autenticación del anfitrión, pero hace que el producto dependa de que ese anfitrión esté disponible. El soporte Linux/Windows también debe describirse con precisión: no es un kernel Linux modificado; es un kernel nuevo, con una ABI parcial de Linux. `vino` ejecuta un subconjunto de programas Windows x86-64 de consola; no equivale a compatibilidad general con Linux o Windows.

**Evaluación:** la base técnica y el trabajo de prototipado son amplios, pero hay un riesgo de dispersión. El siguiente avance de mayor valor no es agregar más subsistemas vistosos: es cerrar un flujo vertical de uso real, con una matriz de compatibilidad, pruebas reproducibles, modelo de amenazas y un instalador/arranque que otras personas puedan validar. La IA neuronal y un LLM entrenado desde cero son proyectos de investigación de largo plazo; no deben bloquear la utilidad inmediata que ya puede dar Claude Code.

## 2. Qué existe y qué no debe darse por terminado

### Sistema operativo y escritorio

El árbol `kernel/` incluye crates separadas para gráficos, FAT32, escritorio, red sobre `smoltcp`, TLS, imágenes, memoria, tareas, ABI Linux, controladores, sincronización, Wi-Fi, kernel y `xtask`. La separación entre lógica `no_std` testeable en el host y acceso a hardware es una buena decisión para un proyecto educativo y facilita pruebas deterministas. La documentación declara K0–K15 completos y describe pruebas en host, QEMU y simuladores.

Eso demuestra una cantidad notable de lógica implementada; no demuestra por sí solo compatibilidad de producto. La documentación indica expresamente que Wi-Fi debe probarse en PC real. También hay que distinguir entre “el driver/protocolo tiene pruebas” y “el equipo arranca, instala, suspende, recupera errores y funciona en hardware de una lista soportada”. La evidencia física y reproducible debe publicarse por dispositivo, versión de firmware/BIOS y configuración.

El proyecto parece ser un **kernel propio que implementa partes de la ABI de Linux**, no una distribución basada en el kernel Linux upstream. Esta diferencia debe mantenerse clara en README, instalador, lenguaje de compatibilidad y comunicación pública.

### JARVIS, proveedores y herramientas

`src/jarvis/agent/` contiene el cerebro, prompts, consejo de agentes, proveedores y bridges de herramientas. `src/jarvis/service/server.py` maneja sesiones y distingue herramientas de anfitrión de acciones ejecutadas en el kernel. `src/jarvis/policy/levels.py` aplica default-deny y clasifica acciones en tres niveles; `docs/permisos.md` documenta esos niveles. El diseño de consentimiento desde la UI es una buena frontera: el modelo propone una acción y el sistema decide si puede ejecutarse.

La documentación y `providers.py` describen Gemini CLI, Codex CLI y Ollama/DeepSeek, además de Claude. Esta integración depende de CLIs y mecanismos de autenticación que cambian con el tiempo; conviene registrar versión/proveedor, contrato probado, límites y fallos de cada integración. “Varios agentes” no debe significar que cada agente tenga acceso independiente a herramientas: la única ruta de ejecución debe seguir siendo el mismo Gate con identidad del proveedor, origen, sesión y auditoría.

Claude es el proveedor principal y Claude Code es especialmente útil para tareas de programación, pero no conviene confundir el asistente de producto con el agente de desarrollo. Para trabajar en repositorios, Claude Code requiere un sandbox de proyecto, cambios revisables, comandos explícitamente controlados y trazabilidad; el permiso de JARVIS para editar archivos no debe convertirse en ejecución arbitraria de shell.

### Voz

`src/jarvis/voice/` implementa audio local y documenta wake word, segmentación, faster-whisper y Piper. `listener.py` define estados conversacionales de forma independiente del audio, lo que hace esa parte testeable. `voicehub.py` conecta eventos de audio con el protocolo del kernel. Es un buen corte de responsabilidades.

Aún falta demostrar calidad operativa en diversidad de micrófonos, ruido, eco, falsos despertares, latencia total, interrupción de TTS y disponibilidad de dispositivos. La voz debe poder degradarse sin bloquear el asistente textual. Hay que medir falsos positivos/falsos negativos con un conjunto de frases consentido y separar métricas de wake word, STT, razonamiento y acción.

### Red neuronal y LLM propio

`docs/cerebro-propio.md` declara C1 (tensores/autograd/capas/optimizadores en NumPy) completada. C2 (corpus), C3 (NLU), C4 (BPE), C5 (transformer GPT), C6–C10 siguen pendientes. Por tanto, hoy existe un motor de aprendizaje experimental, **no** un NLU funcional ni un LLM propio conectado al producto.

Entrenar un modelo conversacional competitivo desde cero en CPU y con datos limitados es una meta muy costosa, de resultado incierto y legalmente sensible por corpus/licencias. Sí es realista construir un clasificador pequeño de intención/entidades y usarlo para órdenes deterministas offline, con umbral de confianza y fallback a Claude. Conviene completar C0–C3 antes de embarcarse en C5: capturar errores reales, mantener test set separado y medir precisión, abstención y latencia.

### Compartir archivos entre equipos

`kernel/sync/` implementa lógica de estado por archivo, emparejamiento derivando claves con HKDF, cifrado ChaCha20-Poly1305, relojes lógicos y conflictos; `kernel/relay/` reenvía mensajes cifrados y la documentación describe dos máquinas detrás de NAT conectándose salientes al relé.

Esto parece resolver **sincronización de una carpeta**, que es distinto de un **enlace de transferencia para un archivo** con caducidad y acceso revocable. La idea del usuario puede requerir ambos modos: enviar una copia puntual mediante enlace/QR y sincronizar una carpeta entre equipos emparejados. El relé actual, si es un proceso que el usuario debe alojar, no garantiza conectividad entre cualquier par de máquinas sin una infraestructura pública estable, descubrimiento/introducción y política antiabuso. Una capa de transporte tampoco es suficiente: hacen falta autenticación de dispositivo, revocación, protección contra replay, límites de tamaño, reanudación, integridad por chunks, cuotas, expiración y consentimiento de recepción.

La derivación del secreto requiere revisión criptográfica cuidadosa. Un código corto escrito por personas puede tener poca entropía; HKDF no convierte una contraseña débil en un intercambio autenticado robusto. Definir formato y entropía del código, PAKE o confirmación fuera de banda, rotación/revocación de claves, nonce único y persistencia del contador. No diseñar criptografía nueva sin revisión externa.

### Compatibilidad con Linux y Windows

La ABI de Linux lista ~90 syscalls y tiene pruebas documentadas con binarios musl estáticos; la cobertura de glibc/dinámicos y compatibilidad por aplicación debe medirse. “Ejecuta aplicaciones Linux” necesita una matriz concreta: estático/dinámico, libc, threads, señales, sockets, mmap, ioctl, GUI y dependencias.

`vino` soporta, según documentación, PE32+ de consola y ciertas DLL/API, hilos, Python Windows y SEH de C. La propia hoja de ruta enumera excepciones C++, 32 bits y ventanas como trabajo futuro. Presentarlo como compatibilidad Windows general sería engañoso. Es preferible un catálogo de programas conocidos con porcentaje de éxito y diagnóstico de imports ausentes.

## 3. Fortalezas

- Arquitectura modular en crates pequeñas; límites claros entre host-testable y hardware.
- Tests cruzados contra implementaciones independientes en áreas como FAT, TLS, codecs y Wi-Fi, según la documentación.
- Protocolo kernel–cerebro explícito y separación de credenciales del modelo respecto del disco del SO.
- Herramientas registradas con default-deny, niveles de riesgo y confirmación humana.
- El sistema contempla fallos y modo degradado: sin cerebro, algunas órdenes locales siguen disponibles.
- ADRs documentan decisiones y cambios de dirección, evitando que los prototipos se confundan con la arquitectura aceptada.
- Existen QEMU, simulación y pruebas de integración como estrategia central de desarrollo.

## 4. Riesgos y brechas prioritarias

### P0 — Definir alcance del producto y vertical de referencia

El repositorio mezcla kernel educativo, escritorio, distribución instalable, asistente multiagente, red neuronal propia, sync, compatibilidad de dos plataformas y Android. Cada uno es un programa grande. Definir un MVP medible: arrancar en QEMU y una PC objetivo; voz→texto→Claude→acción segura aprobada; editar un proyecto con diff revisable; sincronizar un archivo entre dos instalaciones por Internet; arrancar una app Linux estática soportada. Dejar lo demás como experimental.

### P0 — Estado real, CI y artefactos reproducibles

README, `docs/kernel.md`, ADRs y `CLAUDE.md` contienen narrativas extensas que pueden divergir. Consolidar una fuente de verdad por componente con estados `implementado`, `experimental`, `pendiente`, `validado en QEMU`, `validado en hardware`. En CI ejecutar formato, lint, unit tests host, integración QEMU cuando disponible y tests de Python. Publicar artefactos con hashes, toolchain fijado y pasos de reproducción. El análisis no ejecutó tests; no se atribuye un estado verde actual.

### P0 — Modelo de seguridad del agente

Documentar límites y amenazas: prompt injection en archivos/páginas, tool injection de proveedores, escapes de rutas/symlinks, carreras TOCTOU, sesión remota robada, tokens en logs/argv, salida de datos hacia proveedores, acciones concurrentes, aprobación engañosa y daño por agente de coding. Aplicar allowlist canónica de rutas en servidor, descriptor handles cuando sea posible, límites por tool, validación de argumentos estructurada, registro de decisión y resultado, confirmación ligada a contenido exacto y expiración, y sandbox de proyectos. Un resultado leído de Internet o de archivo siempre es dato no confiable.

El proyecto ya pasa tokens por `fw_cfg` y por variables de entorno según ADR; revisar exposición local, permisos de procesos, logs/crash dumps y rotación. El servicio escucha en loopback, una frontera razonable para QEMU, pero el modo de red pública de Brave/relay requiere análisis independiente.

### P1 — Sincronización/transferencia real por Internet

Especificar si “enlace” es transferencia puntual, sync persistente o ambos. Elegir primero un flujo de dos dispositivos. Usar QUIC/TLS/Noise o biblioteca estable cuando sea posible; separar señalización, conexión directa y relay. Implementar transferencia resumible por chunks con manifiestos hash, control de flujo, límites, nombre/ruta segura, archivo temporal y rename atómico, aceptación local y revocación. Probar NAT simétrico, desconexión, reloj desfasado, disco lleno y relé malicioso. Definir quién opera relay, costo, disponibilidad, logs y retención.

### P1 — Matriz de compatibilidad y hardware

Construir fixtures de ELF y PE de aplicaciones reales, clasificados por arquitectura, libc, syscalls/imports, GUI y dependencias. Comparar resultados con Linux/Wine de referencia. Limitar ABI con documentación de compatibilidad. Para hardware, comenzar con una lista corta (QEMU, una notebook/desktop Intel/AMD conocida) y publicar resultados de boot, almacenamiento, red, Wi-Fi, audio, suspensión y apagado.

### P1 — Voz y privacidad

Documentar qué audio queda en memoria, si algo se persiste, política de telemetría y en qué momento el texto se envía a proveedor remoto. Proveer mute físico/UI, indicador siempre visible y botón de interrupción. Medir latencia y precisión con benchmarks locales; prevenir que una transcripción incierta dispare una acción. Toda acción sensible usa aprobación visual independiente de la voz.

### P2 — Cerebro propio pragmático

Priorizar C0–C3 (diagnóstico, corpus sintético/revisado, clasificador/entidades) como función offline pequeña. Comparar con baseline no neuronal (reglas) y no adoptar la red si no gana en métricas. Mantener un split por plantilla/origen para evitar fuga de datos. C5 solo después de demostrar dataset/licencias, equipo, tiempo, calidad mínima y beneficio específico. El modelo pequeño puede clasificar/extraer, no sustituir razonamiento de Claude.

### P2 — Actualizaciones, paquetes y recuperación

Para un SO booteable hacen falta actualizaciones firmadas, rollback, recovery, migración de datos y manejo de fallas de energía. El rootfs/paquetes deben tener firmas y hashes verificables, procedencia, SBOM y licencias. Probar un update interrumpido y recuperación desde USB/ISO. No instalar arbitrariamente binarios `.deb`, `.snap` o `.exe` si no hay aislamiento, firma/procedencia y política definida.

## 5. Recomendación de ruta de trabajo

### Fase A: fiabilidad del camino actual (1–2 iteraciones)

1. Verificar estado git y fuente de verdad; hacer que README enlace a una tabla concisa de estado.
2. Documentar arquitectura y límites de seguridad del protocolo kernel–host.
3. Preparar una CI reproducible con pruebas host y QEMU, sin llamar servicios pagos.
4. Ejecutar manualmente el flujo vertical actual en QEMU y registrar fallos/latencias.

### Fase B: asistente útil y seguro (2–4 iteraciones)

1. Asegurar el Gate y la política de rutas para herramientas de archivos/proyectos.
2. Integrar edición de código mediante Claude Code en carpeta explícita, con diff, aprobación por cambio y comandos controlados.
3. Mejorar voz con mediciones y degradación al modo texto.
4. Versionar contratos de proveedor; Claude principal, agentes auxiliares opcionales, una sola puerta de ejecución.

### Fase C: compartir equipos (2–4 iteraciones)

1. Aclarar transferencia por enlace vs sincronización.
2. Especificar identidad/pareado, claves y threat model.
3. Entregar transferencia puntual resumible con relay administrable, recepción aprobada y hashes.
4. Después completar sync bidireccional, conflictos, borrados y revocación.

### Fase D: compatibilidad de aplicaciones y hardware

1. Publicar matriz mínima de ELF Linux y ampliar syscalls motivadas por apps objetivo.
2. Validar en hardware de referencia con logs y límites conocidos.
3. Expandir `vino` basado en aplicaciones prioritarias; no perseguir “compatibilidad Windows completa”.

### Fase E: IA local

Completar diagnóstico y NLU offline. Mantener Claude como IA principal. Evaluar Ollama/local LLM como opción offline, comparando calidad/recursos; entrenar un LLM propio solo como línea de investigación con criterios de parada.

## 6. Proyectos GitHub comparables

Estas referencias son inspiración de arquitectura; no prueban compatibilidad directa ni que convenga copiar código. Revisar licencias y estado antes de adoptar dependencias.

| Proyecto | Qué aporta | Cómo aplicarlo a JARVIS-OS |
|---|---|---|
| [Veda](https://github.com/vahmoh25/Veda) | OS Rust con voz/agente, acciones tipadas declaradas por apps, aprobación por el sistema, pruebas de agente offline; delega drivers Linux dentro de una VM en vez de recrearlos todos. | La referencia más cercana en UX de agente. Estudiar el contrato de acciones tipadas, consentimiento central y tests con proveedor simulado. Contrastar su decisión de VM de drivers con el objetivo educativo del kernel propio. |
| [NeuraOS](https://github.com/neura-spheres/NeuraOS) | Demo/experimento de OS terminal Rust con apps y proveedores IA (Gemini/OpenAI/Ollama). El repo muestra una historia muy pequeña según su metadata visible. | Útil como comparación de alcance de prototipo AI-native; no tomar claims de README como evidencia de madurez ni como base de kernel. |
| [Redox OS](https://github.com/redox-os/redox) | Sistema operativo Rust de mayor trayectoria, con separación de servicios y componentes del OS. | Comparar organización, proceso de release, portabilidad, paquetes y gobernanza; calibrar que kernel + servicios requieren un ecosistema, no solo syscall coverage. |
| [Syncthing](https://github.com/syncthing/syncthing) | Sincronización continua, discovery, versionado y operación de nodos madura. | Referencia de comportamiento para sync offline-first, estados y conflictos. JARVIS puede conservar UI e integración propias sin reimplementar toda la infraestructura antes de validar el caso de uso. |
| [ZWormHole](https://github.com/TheHolyOneZ/ZWormHole) | Transferencia directa P2P cifrada con emparejamiento y foco en transferencia puntual. | Comparar flujo de aceptación, confianza de dispositivos y reanudación. Su README afirma no relay; no resolverá todos los NAT. |
| [Lightning P2P](https://github.com/Kerim-Sabic/lightning-p2p) | Rust, QUIC/iroh, transferencia por chunks, hashes, relay opcional y clientes multiplataforma. | Estudiar modelo de ticket/identidad y transporte para la idea de enlace compartible. Verificar madurez/licencia y adaptar amenaza/UX al SO. |
| [Rucio](https://github.com/ogarcia/rucio) | P2P Rust con DHT y transferencias directas. | Referencia de descubrimiento descentralizado, aunque el requisito de compartir entre usuarios conocidos puede favorecer invitaciones autenticadas frente a DHT pública. |

### Lectura crítica de las referencias

Veda es la comparación más útil para el patrón agente↔aplicaciones y consentimiento. Redox sirve para entender la escala y la disciplina que implica un OS escrito en Rust. Syncthing muestra el alcance real de sincronización fiable; ZWormHole/Lightning muestran transferencia puntual. NeuraOS parece más cercano a una demo de IA dentro de shell que a una base de kernel. No recomiendo fusionar repositorios ni importar arquitectura entera: extraer contratos, amenazas, métricas y flujos de usuario.

## 7. Conclusión

El proyecto ya tiene una base de investigación implementada considerable, especialmente en el kernel y su entorno de pruebas. El riesgo mayor es presentar hitos de laboratorio como compatibilidad general y tratar demasiadas apuestas grandes como si avanzaran en paralelo. JARVIS puede ser valioso antes de tener un LLM propio: Claude/Claude Code, herramientas acotadas, permisos sólidos, buena voz y colaboración entre equipos resuelven utilidad inmediata. La red neuronal propia debe empezar por NLU medible; el LLM propio es una meta posterior.

La próxima sesión de Claude Code debería inspeccionar el estado del árbol, validar la arquitectura real y proponer un plan corto para cerrar el flujo vertical asistente+proyecto. No debería comenzar una reescritura ni sumar otro subsistema sin enlazarlo a un criterio de aceptación y una prueba reproducible.
