# Instalar JARVIS-OS en VirtualBox

JARVIS-OS arranca en VirtualBox 7 desde la ISO, se instala con el asistente en un disco vacío y
después arranca solo desde ese disco. `cargo xtask vbox-instalar` lo prueba de punta a punta con
una máquina descartable (`JARVISOS-prueba`, que se borra al terminar; nunca toca otra).

## 1. Armar la ISO

```bash
cd kernel
cargo xtask iso            # target/jarvis-os.iso (UEFI de 64 bits, El Torito)
```

## 2. Crear la máquina

En VirtualBox → Nueva:

| Opción | Valor | Por qué |
|---|---|---|
| Tipo / versión | Other / Other/Unknown (64-bit) | JARVIS-OS no es Linux ni Windows para VirtualBox |
| Memoria | 2048 MB o más | el escritorio, la red y los programas de Linux |
| Procesadores | 2 | |
| **EFI** | **activado** (Sistema → Placa base → "Habilitar EFI") | JARVIS-OS arranca solo por UEFI |
| Chipset | PIIX3 o ICH9 | los dos andan |
| Disco | VDI de 512 MB o más, **vacío** | el asistente solo instala en un disco vacío |
| Controladora | IDE (PIIX4) o SATA (AHCI) | hay driver para las dos (y para NVMe) |
| Red | NAT, placa **PCnet-FAST III** o **Intel PRO/1000** | drivers `pcnet` y `e1000` |
| Video | VBoxSVGA o VBoxVGA | se usa el framebuffer de UEFI |
| USB | Controlador USB 3.0 (xHCI) | teclado, mouse, pendrives y la webcam |
| Lectora | `target/jarvis-os.iso` | |

Lo mismo por la línea de comandos (es lo que hace `cargo xtask vbox-instalar`):

```bash
VBoxManage createvm --name JARVISOS --ostype Other_64 --register
VBoxManage modifyvm JARVISOS --firmware efi64 --memory 2048 --cpus 2 --chipset piix3 \
  --graphicscontroller vboxsvga --vram 32 --nic1 nat --nictype1 Am79C973 --usbxhci on
VBoxManage storagectl JARVISOS --name Discos --add ide --controller PIIX4
VBoxManage createmedium disk --filename JARVISOS.vdi --size 512 --format VDI
VBoxManage storageattach JARVISOS --storagectl Discos --port 0 --device 0 --type hdd --medium JARVISOS.vdi
VBoxManage storageattach JARVISOS --storagectl Discos --port 1 --device 0 --type dvddrive --medium kernel/target/jarvis-os.iso
```

## 3. Instalar

Al arrancar desde la ISO, JARVIS-OS entra en **modo en vivo** y abre el asistente: nombre de
usuario, contraseña (dos veces), el disco vacío y confirmar. Al terminar, apagá la máquina, sacá
la ISO (Almacenamiento → la lectora → "Quitar disco") y volvé a prenderla: arranca desde el
disco, pide la contraseña y JARVIS presenta el sistema.

## 4. JARVIS con el cerebro

`cargo xtask vbox [MÁQUINA]` prende la máquina con el cerebro de la PC (Claude): le pasa el puerto
y el token por la línea de comandos de fw_cfg y habilita que la NAT llegue al `localhost` de la
PC. `--iso` arma la ISO y la monta; `--simulado`, sin Claude.

## 5. La webcam en la máquina virtual

VirtualBox puede pasarle la webcam de la PC a la máquina como una cámara USB (UVC). Hace falta el
**Extension Pack** de Oracle (licencia PUEL: gratis para uso personal y educativo; lo instalás
vos desde Archivo → Herramientas → Extension Pack Manager) y el controlador USB 3.0 (xHCI):

```bash
VBoxManage list webcams                         # .1 "Web Camera"
VBoxManage controlvm JARVISOS webcam attach .1  # con la máquina prendida
```

JARVIS-OS la ve como un dispositivo USB de video (ver ADR 0022).

## Problemas conocidos

- Sin EFI la máquina no arranca (pantalla negra o el shell de BIOS).
- "No hay discos vacíos": el disco tiene que estar en blanco (uno nuevo de VirtualBox lo está).
- La red tarda unos segundos en tener IP (DHCP de la NAT de VirtualBox).
- Con Hyper-V activo en Windows, VirtualBox anda más lento (usa la API de Hyper-V).
