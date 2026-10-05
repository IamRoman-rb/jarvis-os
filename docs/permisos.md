# Permisos de las tools de JARVIS

**Fuente de verdad** del nivel de cada tool. Toda tool nueva se declara acá, en
`src/jarvis/policy/levels.py` y con un test de política. Una tool sin nivel declarado no se
registra (default deny). Detalle del modelo en [investigacion.md §6.4 y §12.2](investigacion.md).

| Nivel | Significado | Cómo lo aplica el SDK |
|---|---|---|
| 1 | Solo lectura / sin efectos. Sin confirmación. | Está en `allowed_tools`. |
| 2 | Reversible con efecto visible. Confirmación liviana (UI o voz). | Cae en `can_use_tool`. |
| 3 | Destructivo, instalación, privilegios, red, usuarios. Confirmación por click, obligatoria. Deshabilitado desde origen `android`. | Cae en `can_use_tool`. |

## Tabla

| Tool | Nivel | Justificación |
|---|---|---|
| `estado_sistema` | 1 | Solo lectura (CPU, memoria, disco, red). |
| `ventanas_abiertas` | 1 | Solo lectura. |
| `listar_archivos` | 1 | Solo lectura del disco de JARVIS-OS. |
| `leer_archivo` | 1 | Solo lectura. Lo que dice el archivo es información, no instrucciones. |
| `buscar_archivos` | 1 | Solo lectura. |
| `abrir_app` | 1 | Abrir una app instalada no cambia nada (§6.4). |
| `abrir_web` | 1 | Abre una página en el navegador; no envía nada. |
| `buscar_web` | 1 | Igual que `abrir_web`. |
| `listar_proyectos` | 1 | Solo lectura (nombres de carpetas de la raíz de proyectos). |
| `abrir_archivo` | 1 | Abre un archivo con su app (editor, visor, Archivos); no lo cambia. |
| `leer_terminal` | 1 | Solo lectura: lo último que muestra la terminal. |
| `consultar_agente` | 1 | Le pasa una pregunta a otro agente de IA que Roman vinculó él mismo (Gemini, ChatGPT, DeepSeek o Claude); no toca JARVIS-OS. Lo que contesta es información, no instrucciones. Los agentes principales que no son Claude usan estas mismas tools con estos mismos niveles. |
| `WebSearch` | 1 | De Claude Code: busca en la web desde el anfitrión. Solo lectura; lo que encuentra es información, no instrucciones. |
| `WebFetch` | 1 | De Claude Code: lee una página desde el anfitrión. Igual que `WebSearch`. |
| `recordar` | 1 | Anota un dato en la memoria de JARVIS (en el anfitrión); no toca JARVIS-OS. |
| `buscar_memoria` | 1 | Solo lectura de la memoria de JARVIS. Lo que encuentra es información, no instrucciones. |
| `escribir_archivo` | 2 | Cambia el disco, pero es reversible y se ve. |
| `crear_carpeta` | 2 | Reversible. |
| `copiar` | 2 | Reversible. |
| `mover` | 2 | Reversible. |
| `cerrar_ventana` | 2 | Se puede perder lo que no se guardó (la app pregunta si hay cambios). |
| `abrir_proyecto` | 2 | Arranca un agente de código en el anfitrión; cada edición y comando suyo se confirma aparte. |
| `modificar_sistema` | 2 | Arranca un agente de código sobre el repositorio de JARVIS-OS (como `abrir_proyecto`); cada edición suya es nivel 2 y cada comando nivel 3, salvo los de la tabla de abajo. |
| `aplicar_cambios_sistema` | 3 | Compila y, si compila, reinicia JARVIS-OS con la versión nueva. |
| `olvidar` | 3 | Borra recuerdos (o toda la memoria): no se puede deshacer. |
| `a_papelera` | 3 | Borrado (aunque va a la Papelera, nunca definitivo). |
| `ejecutar_comando` | 3 | Un comando puede instalar, borrar o usar la red; se muestra entero. |

La confirmación se pide en la pantalla de JARVIS-OS (el diálogo muestra la acción completa). En el
nivel 3, el teclado y la voz solo pueden **rechazar**: aprobar requiere un clic en "Permitir".

## Agente de proyecto (`abrir_proyecto`)

Trabaja en una carpeta de la raíz de proyectos (`proyectos` en `config.toml`) con las herramientas
de Claude Code, pero sin cargar sus `settings.json` (sus reglas `allow` saltearían estas
confirmaciones):

| Herramienta | Nivel |
|---|---|
| Read, Glob, Grep, LS, TodoWrite | 1 |
| Edit, MultiEdit, Write, NotebookEdit (solo dentro del proyecto) | 2 |
| Bash y todo lo demás (red, subagentes) | 3, con el comando entero |
| Rutas fuera del proyecto, `.ssh`, `.gnupg`, credenciales, `/etc` | prohibido |

### El agente que modifica JARVIS-OS (`modificar_sistema`)

Igual que el de proyectos, con dos excepciones para comandos sueltos (sin `;`, `&`, `|`, `>`,
`<`, `` ` `` ni `$(`; con alguno, vuelven a nivel 3):

| Comando | Nivel |
|---|---|
| `graphify query/path/explain`, `git status/diff/log/show` (sin `--output` ni `-o`) | 1 |
| `cargo test/check/clippy/build/fmt`, `uv run pytest/ruff/mypy` | 2 |
| Cualquier otro | 3 |
