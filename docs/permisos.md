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
| | | |
