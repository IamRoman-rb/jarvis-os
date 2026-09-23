# ADR 0002 — Claude Agent SDK como cerebro, aislado y con una única compuerta de permisos

- **Estado:** aceptada
- **Fecha:** 2026-09-22
- **Modifica:** el boceto de [investigacion.md §12.2–12.3](../investigacion.md#122-decisión-agent-sdk-en-vez-de-la-messages-api-a-mano)

## Contexto

La investigación eligió el Claude Agent SDK (`claude-agent-sdk`) en vez de programar el bucle
de tool use sobre la Messages API, y propuso poner las tools de nivel 1 en `allowed_tools` y
mandar las de nivel 2/3 al callback `can_use_tool`. Al implementar la fase 1a contra el SDK
instalado (v0.2.157) aparecieron tres detalles que el boceto no contemplaba.

## Decisión

1. **Agent SDK con `ClaudeSDKClient`** (no `query()`): `can_use_tool` requiere el modo streaming.
   Las tools son un servidor MCP in-process (`create_sdk_mcp_server`) llamado `system`.
2. **`tools=[]`**: sin tools built-in (Bash, Read, Edit…). Claude solo actúa con nuestras tools.
3. **`setting_sources=[]`**: con el valor por defecto (`None`) el SDK carga
   `~/.claude/settings.json`, `.claude/settings.json` y `.claude/settings.local.json`. Una regla
   `permissions.allow` pensada para Claude Code (por ejemplo `mcp__*`) aprobaría tools de nivel
   2/3 **sin pasar por `can_use_tool`**. JARVIS no debe heredar permisos de otra herramienta.
4. **`allowed_tools=[]`: toda tool pasa por `can_use_tool` → `policy/gate.py`**, que aprueba
   sola el nivel 1 y confirma 2 y 3. Cambia el boceto (que ponía el nivel 1 en `allowed_tools`)
   por dos motivos: (a) hay un único punto de decisión y auditoría, así que no hay dos listas que
   mantener sincronizadas; (b) con el boceto, el SDK emite un `CanUseToolShadowedWarning` en cada
   ejecución, y silenciarlo de forma global también podía tapar el aviso de `bypassPermissions`.
   El costo es una llamada local extra por tool de nivel 1: despreciable.
5. **Hook `PreToolUse` solo para auditoría, devolviendo `{}`**. Si devolviera `allow`, el SDK
   también saltearía `can_use_tool`.
6. **`permission_mode="default"`** siempre. Nunca `bypassPermissions` ni `acceptEdits`.
7. **`max_budget_usd` y `max_turns` configurables**: tope de gasto y de vueltas por pedido.

Todo lo anterior está cubierto por tests en `tests/policy/`.

## Consecuencias

- (+) La superficie de acción es explícita: 4 tools, cada una con nivel, y una sola compuerta.
- (+) Los tests simulan el orden de permisos del SDK sin red (`tests/helpers.py`).
- (−) Dependemos de que el SDK respete su orden de evaluación documentado. Si una versión nueva
  lo cambia, los tests de `tests/policy/` con el SDK mockeado no lo detectan: el test `live`
  (`uv run pytest -m live`) sí ejercita el SDK real y conviene correrlo al actualizarlo.
- (−) El SDK trae un binario de Claude Code empaquetado; hay que actualizarlo junto con el paquete.
