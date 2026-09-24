# Manual de JARVIS-OS

JARVIS-OS es un sistema operativo propio, escrito en Rust desde cero: kernel, drivers, sistema
de archivos FAT32, red TCP/IP, escritorio con ventanas, navegador y terminal.

## El escritorio

- **Win** abre el menú de inicio: escribí para buscar una app o una dirección web.
- **Alt+Tab** cambia de ventana. **Win+D** muestra el escritorio (JARVIS).
- **Win+Flechas** acoplan y maximizan. **Win+Ctrl+D** crea un escritorio virtual.
- **Win+X** abre los enlaces rápidos, **Win+A** la configuración rápida y **Win+N** las
  notificaciones con el calendario.
- **F1** muestra todos los atajos.

## La terminal (Ctrl+Alt+T)

La shell se llama `jsh` y se parece a `bash`:

    ls -l                     lista la carpeta actual
    cd Documentos             entra a una carpeta (cd .. sube, cd - vuelve)
    cat notas.txt | grep hola busca "hola" en un archivo
    echo hola > saludo.txt    escribe un archivo (>> agrega al final)
    cp -r Proyectos /Copia    copia una carpeta entera
    rm viejo.txt              lo manda a la Papelera (nada se borra para siempre sin preguntar)
    find / -name "*.txt"      busca archivos
    wget https://sitio/x.zip  descarga un archivo a la carpeta actual

`help` lista todos los comandos y `man <comando>` explica uno.

## Programas: apt

    apt update                baja la lista de paquetes
    apt list                  muestra lo que se puede instalar
    apt install neofetch      instala un programa
    apt remove neofetch       lo desinstala (los archivos van a la Papelera)
    apt upgrade               actualiza todo

Los programas de JARVIS-OS son scripts de `jsh` que quedan en `/Programas/bin`. Para hacer el
tuyo: `nano /Programas/bin/miprograma`, escribí los comandos (uno por línea) y después
ejecutalo escribiendo su nombre. `$1`, `$2`… son los argumentos.

## Programas de Windows y Linux

Se pueden **descargar** (con el navegador o con `wget`) e **inspeccionar** (`file`, `strings`,
`xxd`), pero todavía no **ejecutar**: para eso el kernel necesita espacio de usuario, un
cargador de programas y la API que esos programas esperan (Win32 o las llamadas al sistema de
Linux). Está en el plan.

## Configuración (Win+I)

Fondo de pantalla, zona horaria, formato del reloj, red, navegador (buscador, página de inicio,
páginas claras u oscuras), sonido, mouse y teclado (latinoamericano o de EE. UU.), programas
instalados, uso del disco y PIN de bloqueo. Todo queda guardado en `/Sistema/config.ini`.
