#!/bin/jsh
# saludo (snap, revisión 3): te saluda. Ejemplo: saludo Ana
NOMBRE="$1"
test -z "$NOMBRE" && NOMBRE=$(whoami)
echo "¡Hola, $NOMBRE! Bienvenido a JARVIS-OS."
