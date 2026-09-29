#!/bin/jsh
# neofetch para JARVIS-OS: el logo y los datos del sistema.
# Es un script de jsh: lee /proc y usa comandos comunes (uname, free, df, uptime).
CPU=$(grep "model name" /proc/cpuinfo | cut -d: -f2)
MEM=$(free -h | grep Heap | tr -s " " | cut -d" " -f3)
MEMT=$(free -h | grep Heap | tr -s " " | cut -d" " -f2)
DISCO=$(df -h | tail -n 1 | tr -s " " | cut -d" " -f3)
DISCOT=$(df -h | tail -n 1 | tr -s " " | cut -d" " -f2)
PAQ=$(dpkg -l | grep "^ii" | wc -l)
echo -e "\e[96m       _____       \e[0m  \e[1;92m$USER\e[0m@\e[1;92m$HOSTNAME\e[0m"
echo -e "\e[96m    .-'     '-.    \e[0m  ------------------"
echo -e "\e[96m   /   .---.   \   \e[0m  \e[1;96mSO:\e[0m JARVIS-OS 0.1 x86_64"
echo -e "\e[96m  |   / \e[94m(*)\e[96m \   |  \e[0m  \e[1;96mKernel:\e[0m $(uname -r) (Rust, no_std)"
echo -e "\e[96m  |   \     /   |  \e[0m  \e[1;96mEncendido:\e[0m $(uptime | cut -d, -f1 | cut -d" " -f4-)"
echo -e "\e[96m   \   '---'   /   \e[0m  \e[1;96mPaquetes:\e[0m $PAQ (apt)"
echo -e "\e[96m    '-._____.-'    \e[0m  \e[1;96mShell:\e[0m jsh"
echo -e "\e[96m                   \e[0m  \e[1;96mCPU:\e[0m$CPU"
echo -e "\e[96m      J A R V I S  \e[0m  \e[1;96mMemoria:\e[0m $MEM / $MEMT (heap)"
echo -e "\e[96m                   \e[0m  \e[1;96mDisco:\e[0m $DISCO / $DISCOT"
echo
echo -e "   \e[41m   \e[42m   \e[43m   \e[44m   \e[45m   \e[46m   \e[47m   \e[0m"
