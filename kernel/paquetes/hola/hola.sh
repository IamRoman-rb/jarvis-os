#!/bin/jsh
# hola: el programa más chico de JARVIS-OS.
# Los programas de JARVIS-OS son scripts de jsh (la shell de la terminal): cada línea es un
# comando. $1, $2... son los argumentos; $USER, tu usuario.
# Para hacer el tuyo: nano /Programas/bin/miprograma  y después escribí miprograma.
echo -e "\e[1;96mHola, $USER.\e[0m Soy un programa instalado con apt."
test -n "$1" && echo "Me pasaste: $@"
echo "Mi código está en $0 (cat $0 para verlo)."
