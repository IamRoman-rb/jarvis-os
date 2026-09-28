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
| `escribir_archivo` | 2 | Cambia el disco, pero es reversible y se ve. |
| `crear_carpeta` | 2 | Reversible. |
| `copiar` | 2 | Reversible. |
| `mover` | 2 | Reversible. |
| `cerrar_ventana` | 2 | Se puede perder lo que no se guardó (la app pregunta si hay cambios). |
| `a_papelera` | 3 | Borrado (aunque va a la Papelera, nunca definitivo). |
| `ejecutar_comando` | 3 | Un comando puede instalar, borrar o usar la red; se muestra entero. |

La confirmación se pide en la pantalla de JARVIS-OS (el diálogo muestra la acción completa). En el
nivel 3, el teclado y la voz solo pueden **rechazar**: aprobar requiere un clic en "Permitir".
