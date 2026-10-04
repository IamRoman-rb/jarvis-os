//! Drivers de hardware real de JARVIS-OS (hito K13, ADR 0011): todo lo que se puede decidir sin
//! tocar un registro.
//!
//! Un driver tiene dos mitades. Una **habla con el hardware**: lee y escribe registros, espera
//! interrupciones, reserva memoria para el DMA. Esa vive en el binario del kernel
//! (`kernel/src/ahci.rs`, `xhci.rs`…). La otra **interpreta**: arma un comando con el formato
//! que pide la especificación, recorre una tabla del firmware, decide qué hacer con un anillo de
//! descriptores. Esa va acá, sin `unsafe`, y se prueba en el host con datos armados a mano o
//! capturados de hardware real.
//!
//! - [`acpi`]: las tablas fijas de ACPI (RSDP, XSDT, MADT, FADT, MCFG, HPET). El AML (el
//!   bytecode de la DSDT) lo interpreta la crate `acpi` en el kernel.
//! - [`apic`]: el formato de las entradas del IOAPIC y de los mensajes MSI/MSI-X.
//! - [`gpt`]: la tabla de particiones de UEFI (leer, crear, ver una partición como disco).
//! - [`ahci`] y [`nvme`]: los comandos de los discos SATA y NVMe y lo que responden.
//! - [`nic`]: los anillos de descriptores de las placas de red Intel y Realtek.
//! - [`usb`]: descriptores, TRB y contextos de xHCI, teclado y mouse HID, BOT y SCSI.
//! - [`hda`]: los verbos de los codecs de audio y el camino del DAC al parlante.
//! - [`rtw88`]: la placa Wi-Fi Realtek RTL8821CE (K14): encendido, efuse, firmware, tablas,
//!   canales y descriptores.
//! - [`sensors`]: la temperatura del procesador (AMD por SMN) y de las zonas térmicas de ACPI.
//!
//! Referencias: especificación ACPI 6.5 (cap. 5), Intel SDM vol. 3A cap. 11 (APIC) y la
//! especificación PCI Local Bus 3.0 §6.8 (MSI).

#![no_std]

extern crate alloc;

pub mod acpi;
pub mod ahci;
pub mod apic;
pub mod gpt;
pub mod hda;
pub mod nic;
pub mod nvme;
pub mod rtw88;
pub mod sensors;
pub mod usb;

/// Lee un entero en little endian de `b` en `at`. Fuera de rango da 0: las tablas del firmware
/// pueden venir cortas, y un campo que falta es un campo vacío.
pub(crate) fn le(b: &[u8], at: usize, n: usize) -> u64 {
    let Some(bytes) = b.get(at..at + n) else {
        return 0;
    };
    bytes
        .iter()
        .rev()
        .fold(0u64, |acc, &x| (acc << 8) | x as u64)
}

/// Un texto ASCII de un campo fijo del hardware (modelo, número de serie): lo que no se puede
/// mostrar pasa a espacio, y se recortan los espacios de relleno.
pub(crate) fn ascii(bytes: &[u8]) -> alloc::string::String {
    let s: alloc::string::String = bytes
        .iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                ' '
            }
        })
        .collect();
    s.trim().into()
}
