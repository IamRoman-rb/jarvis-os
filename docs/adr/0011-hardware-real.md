# ADR 0011: hardware real

- **Estado:** aceptada (K13 terminado el 2026-09-30)
- **Fecha:** 2026-09-29

## Contexto

Hasta K12, JARVIS-OS solo conoce lo que emula QEMU con virtio (disco, red, video, sonido),
además del PS/2, el PIC 8259 y el PIT. Una PC real no tiene nada virtio. La PC destino (la de
Roman) es una Gigabyte B550M DS3H AC con un Ryzen 5 5600GT:

| Qué | Hardware | Driver que hace falta |
|---|---|---|
| Disco | SSD SATA (controladora AHCI AMD 1022:43EB), **con Windows** | AHCI |
| Red | Realtek RTL8168 (10EC:8168) | r8169 |
| Teclado y mouse | USB, en controladoras xHCI AMD | xHCI + HID |
| Sonido | HDA AMD con codec Realtek | HDA |
| Wi-Fi | Realtek 8821CE | K14 |

K13 es que JARVIS-OS arranque ahí desde un pendrive y se pueda instalar en un disco vacío.

## Decisión

1. **La lógica de los drivers va en una crate `no_std` testeable, `jarvis-drivers`**, igual que
   `jarvis-net` o `jarvis-linux`: el formato de las tablas ACPI, las entradas del IOAPIC y los
   mensajes MSI, GPT, los comandos AHCI y NVMe, los anillos de descriptores de las placas de red,
   los descriptores USB y los reportes HID, SCSI y los verbos de HDA. El binario del kernel solo
   toca registros, en un módulo por driver (reglas 3 y 4 de CLAUDE.md).
2. **ACPI: tablas propias y AML con la crate `acpi` (rust-osdev).** Las tablas fijas (RSDP, XSDT,
   MADT, FADT, MCFG, HPET) son estructuras con formato fijo: se leen con un parser propio, para
   entenderlas. El AML (la DSDT y las SSDT) es un lenguaje completo, con métodos, variables y
   acceso a registros: escribir un intérprete que ande con las DSDT reales es un proyecto en sí
   mismo, como lo eran TCP/IP (smoltcp) o TLS (rustls). La crate `acpi` 6.x lo trae. El kernel le
   presta memoria física, puertos, espacio de configuración PCI y tiempo (`kernel/acpi.rs`).
3. **APIC en vez del PIC, y MSI para los dispositivos nuevos.** En una PC con UEFI, la línea INTx
   de un dispositivo PCI no tiene un número confiable para el PIC. Si hay MADT, el kernel enmascara
   el PIC y usa el APIC local y los IOAPIC. El timer, el teclado y el mouse conservan sus vectores
   (32, 33 y 44), con los *overrides* de la MADT. Las líneas INTx del bus 0 (los virtio de QEMU) se
   enrutan según `_PRT`, después de avisar `\_PIC(1)`. Los drivers nuevos usan MSI o MSI-X, con un
   vector propio cada uno. Sin MADT se sigue con el PIC, y sin MSI los drivers esperan revisando
   su anillo con plazo.
4. **Disco con particiones GPT, y nunca una partición ajena.** El sistema monta la partición con
   el GUID de tipo de JARVIS. Si el disco no tiene GPT, lo monta entero como FAT32 (el `disco.img`
   de siempre). Las particiones que no son de JARVIS no se montan ni se escriben.
5. **El instalador solo escribe en discos vacíos.** Un disco es elegible si no tiene firma de MBR
   ni cabecera GPT. En el disco escribe un MBR protector, una GPT, una partición de sistema EFI
   (FAT32, con lo mismo que el medio de arranque) y la partición de datos de JARVIS. Pide doble
   confirmación. El SSD con Windows de Roman nunca aparece como destino.
6. **Lo que QEMU no emula se escribe según el datasheet y se prueba en la PC.** El RTL8168 y el
   codec real de HDA no existen en QEMU. Sus drivers siguen el datasheet y el driver de Linux, y
   quedan verificados recién cuando corren en la PC. Para diagnosticar ahí (no hay puerto serie),
   el registro del arranque se muestra en pantalla y se guarda en `/Sistema/arranque.log`.

## Consecuencias

- Una crate externa más (`acpi`) en el kernel. Un error de su intérprete con una DSDT rara se
  anota y el sistema sigue: no se puede apagar por ACPI, pero arranca.
- `cargo xtask test` sigue con virtio y suma corridas con el hardware que QEMU sí emula (AHCI,
  NVMe, e1000e, RTL8139, xHCI, HDA).
- El PIC y las líneas compartidas quedan como respaldo, no como el camino normal.
- **S3 (dormir en RAM) queda fuera de K13.** Al despertar, la CPU vuelve en modo real y todos los
  dispositivos, reseteados; la placa de video no la reinicia el firmware, y en la APU de la PC
  destino eso necesita un driver nativo de GPU. Suspender sigue siendo la pantalla negra con la
  CPU en `hlt`.
