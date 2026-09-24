#!/bin/jsh
# fortune: una frase al azar de /Programas/share/fortune/frases.txt
ARCHIVO=/Programas/share/fortune/frases.txt
N=$(wc -l < $ARCHIVO)
LINEA=$(expr $RANDOM % $N + 1)
head -n $LINEA $ARCHIVO | tail -n 1
