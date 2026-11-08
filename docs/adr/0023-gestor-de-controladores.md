# ADR 0023: gestor de controladores

- **Estado:** aceptada
- **Fecha:** 2026-11-08

## Contexto

Roman pidió "un instalador de drivers como Driver Booster", para que JARVIS-OS consiga los
drivers que le falten en una máquina. Driver Booster (y Snappy Driver Installer Origin, su
equivalente libre) instalan **drivers de Windows**: archivos `.sys` para el kernel de Windows.
En JARVIS-OS no sirven, y tampoco los módulos del kernel Linux (es otro kernel, ADR 0003): un
driver de JARVIS-OS es código Rust compilado adentro del kernel.

Lo que sí sirve de afuera es el **firmware**: el programa que corre adentro de la placa (Wi-Fi,
algunas placas de red y de video), el mismo para cualquier sistema operativo, publicado en
`linux-firmware`. Y saber qué hardware falta soportar.

Referencias: Snappy Driver Installer Origin (inventario por IDs de hardware contra una base),
`pciutils`/`pci.ids` y `usb.ids` (nombres), `ubuntu-drivers` (qué driver conviene a cada
dispositivo) y `linux-firmware`.

## Decisión

1. **Inventario**: el kernel recorre el bus PCI al arrancar (fabricante, modelo y clase de cada
   función) y anota cada USB que se conecta (`hw.rs`, `xhci.rs`); va en `SystemStats.hardware`.
2. **Catálogo** (`kernel/paquetes/controladores/catalogo.txt`): qué driver de JARVIS-OS atiende
   cada dispositivo (por fabricante:modelo o por clase) y qué firmware necesita, con su URL fija
   de `linux-firmware` (un commit) y su **SHA-256**. Viene adentro del sistema y se actualiza con
   `sudo controladores actualizar` (solo si se entiende).
3. **`controladores`**: la tabla de cada dispositivo (ok, no necesita, falta firmware, SIN
   DRIVER). **`sudo controladores instalar`** baja el firmware que falta, lo verifica y lo deja
   en `/Sistema/firmware/` (el kernel ya lo busca ahí al arrancar). **`controladores reporte`**
   escribe `/Documentos/hardware.txt` con lo que no tiene driver (IDs y clase), para pedirlo.
   También `lspci` y `lsusb`, y la fila "Controladores" en Configuración → Hardware.
4. Un driver nuevo llega con una versión nueva de JARVIS-OS: no hay drivers cargables. Si algún
   día los hay (programas de usuario que manejan hardware, como en los micronúcleos), entrarían
   en este mismo catálogo.

## Consecuencias

- (+) Se sabe, en cualquier PC, qué anda y qué no, y el firmware que falta se instala solo y
  verificado.
- (+) El reporte da exactamente lo que hace falta para escribir un driver (IDs y clase).
- (−) No "instala drivers" como en Windows: el kernel tiene los que tiene.
- (−) Los nombres de dispositivos son un pedazo de `pci.ids` (los fabricantes más comunes).
