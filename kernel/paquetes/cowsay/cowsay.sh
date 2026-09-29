#!/bin/jsh
# cowsay: la vaca dice lo que le pases (o "Muuu" si no le pasás nada).
# Ejemplos: cowsay hola · fortune | cowsay
TEXTO="$@"
test -z "$TEXTO" && TEXTO="$(cat)"
test -z "$TEXTO" && TEXTO="Muuu. Probá: cowsay hola"
ARRIBA=$(echo "$TEXTO" | sed "s/./_/g")
ABAJO=$(echo "$TEXTO" | sed "s/./-/g")
echo " _${ARRIBA}_"
echo "< $TEXTO >"
echo " -${ABAJO}-"
echo "        \   ^__^"
echo "         \  (oo)\_______"
echo "            (__)\       )\/\\"
echo "                ||----w |"
echo "                ||     ||"
