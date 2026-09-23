#!/usr/bin/env bash
# Bloquea ediciones que parezcan contener una API key (Anthropic/Tailscale).
# Salir con código 2 hace que Claude Code cancele la acción. Requiere jq.
CONTENIDO=$(jq -r '.tool_input.content // .tool_input.new_string // ""')
if printf '%s' "$CONTENIDO" | grep -qE 'sk-ant-[A-Za-z0-9_-]{10,}|tskey-[A-Za-z0-9-]{10,}'; then
  echo "Bloqueado: el cambio contiene lo que parece una API key (Anthropic/Tailscale)." >&2
  exit 2
fi
exit 0
