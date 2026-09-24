#!/bin/jsh
# saludo (snap, canal beta, revisión 5): te saluda y te dice la fecha.
NOMBRE="$1"
test -z "$NOMBRE" && NOMBRE=$(whoami)
echo "¡Hola, $NOMBRE! Bienvenido a JARVIS-OS."
echo "(beta) Hoy es: $(date)"
