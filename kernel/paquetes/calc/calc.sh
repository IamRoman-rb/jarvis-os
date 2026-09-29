#!/bin/jsh
# calc: cuentas enteras. Ejemplo: calc 2 + 3 x 4
# (x multiplica; * también, pero entre comillas: sin comillas la shell lo toma como comodín)
test -z "$1" && echo "Uso: calc 2 + 3 x 4   (enteros: + - x / %)" && exit
expr $@
