#!/bin/jsh
# notas (snap): anotá algo con `notas comprar pan`; `notas` solo las muestra.
ARCHIVO=/Documentos/notas.txt
test -z "$1" && test -f $ARCHIVO && cat -n $ARCHIVO && exit
test -z "$1" && echo "Todavía no hay notas. Probá: notas comprar pan" && exit
echo "$@" >> $ARCHIVO
echo "Anotado en $ARCHIVO"
