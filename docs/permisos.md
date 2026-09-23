# Permisos de las tools de JARVIS

**Fuente de verdad** del nivel de cada tool. Toda tool nueva se declara acá, en
`src/jarvis/policy/levels.py` y con un test de política. Una tool sin nivel declarado no se
registra (default deny). Detalle del modelo en [investigacion.md §6.4 y §12.2](investigacion.md).

Todas las tools pasan por `can_use_tool` → `policy/gate.py` (`allowed_tools` queda vacío; ver
[ADR 0002](adr/0002-agent-sdk.md)). La compuerta decide según el nivel:

| Nivel | Significado | Qué hace la compuerta |
|---|---|---|
| 1 | Solo lectura / sin efectos. | Aprueba sin preguntar. |
| 2 | Reversible con efecto visible. | Pide confirmación liviana (terminal; UI en la fase 1b). |
| 3 | Destructivo, instalación, privilegios, red, usuarios. | Pide confirmación explícita. Desde origen `android`, rechaza sin preguntar. |

## Tabla

| Tool | Nivel | Justificación |
|---|---|---|
| `find_files` | 1 | Solo lista nombres de archivos dentro de la allowlist. No lee contenido ni sigue symlinks hacia afuera. |
| `system_info` | 1 | Solo lectura de métricas del sistema (psutil). |
| `open_app` | 2 | Efecto visible pero reversible (se cierra la ventana). Solo ids `.desktop` instalados, sin argumentos. |
| `trash_file` | 3 | Borrado: aunque va a la papelera (`gio trash`, recuperable), es la acción más destructiva disponible. Deshabilitada desde Android. |

Hay un test (`tests/policy/test_levels_options.py`) que falla si esta tabla y
`src/jarvis/policy/levels.py` no coinciden.
