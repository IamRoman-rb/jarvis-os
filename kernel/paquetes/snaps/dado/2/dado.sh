#!/bin/jsh
# dado (snap): tira un dado; `dado 2` tira dos.
echo "Salió: $(expr $RANDOM % 6 + 1)"
test "$1" = 2 && echo "Y el otro: $(expr $RANDOM % 6 + 1)"
