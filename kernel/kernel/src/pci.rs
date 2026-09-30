//! Bus PCI: cómo se encuentran los dispositivos (disco, red, video…) conectados a la máquina.
//!
//! Cada dispositivo PCI tiene un "espacio de configuración" de 256 bytes con su fabricante,
//! modelo, recursos (BAR: rangos de puertos o memoria) y un registro de comando. Se accede con el
//! mecanismo clásico de dos puertos: se escribe la dirección (bus/dispositivo/función/registro) en
//! 0xCF8 y se lee o escribe el dato en 0xCFC.
//! Desde K13 también se configuran las interrupciones por mensaje (MSI y MSI-X): el dispositivo
//! avisa escribiendo en la memoria del APIC local en vez de levantar una línea compartida.
//! Referencia: <https://wiki.osdev.org/PCI> y PCI Local Bus 3.0 §6.8.

use alloc::vec::Vec;

use jarvis_drivers::apic::{
    MsiLayout, MsixTable, msi_address, msi_data, msi_enable, msix_enable, msix_entry,
};
use x86_64::instructions::port::Port;

use crate::paging;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
/// Bit 10 del registro de comando: el dispositivo no usa su línea de interrupción (INTx).
const INTX_DISABLE: u32 = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
}

fn address(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | (bus as u32) << 16
        | (slot as u32) << 11
        | (function as u32) << 8
        | (offset as u32 & 0xFC)
}

fn read32(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    // SAFETY: 0xCF8/0xCFC son los puertos del mecanismo de configuración PCI #1; leer el espacio
    // de configuración no tiene efectos secundarios.
    unsafe {
        Port::<u32>::new(CONFIG_ADDRESS).write(address(bus, slot, function, offset));
        Port::<u32>::new(CONFIG_DATA).read()
    }
}

fn write32(bus: u8, slot: u8, function: u8, offset: u8, value: u32) {
    // SAFETY: ídem `read32`; solo se escribe el registro de comando de un dispositivo que el
    // llamador ya identificó.
    unsafe {
        Port::<u32>::new(CONFIG_ADDRESS).write(address(bus, slot, function, offset));
        Port::<u32>::new(CONFIG_DATA).write(value);
    }
}

/// Capacidades PCI que usa el kernel.
#[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
const CAP_MSI: u8 = 0x05;
#[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
const CAP_MSIX: u8 = 0x11;

impl Device {
    pub fn read(&self, offset: u8) -> u32 {
        read32(self.bus, self.slot, self.function, offset)
    }

    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    pub fn write(&self, offset: u8, value: u32) {
        write32(self.bus, self.slot, self.function, offset, value);
    }

    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    /// Escribe 16 bits sin pisar los otros 16 de la misma palabra.
    pub fn write16(&self, offset: u8, value: u16) {
        let aligned = offset & !3;
        let shift = (offset & 2) as u32 * 8;
        let old = self.read(aligned) & !(0xFFFF << shift);
        self.write(aligned, old | (value as u32) << shift);
    }

    pub fn vendor(&self) -> u16 {
        self.read(0) as u16
    }

    pub fn device_id(&self) -> u16 {
        (self.read(0) >> 16) as u16
    }

    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    /// (clase, subclase, interfaz): 01/06/01 = AHCI, 01/08/02 = NVMe, 0C/03/30 = xHCI…
    pub fn class(&self) -> (u8, u8, u8) {
        let c = self.read(0x08);
        ((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8)
    }

    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    /// Configura MSI-X (si lo tiene) o MSI para que el dispositivo dispare `vector` en la CPU
    /// `apic_id`. Con MSI-X, todas las entradas de la tabla van a ese vector (los drivers usan
    /// una sola cola de eventos). Devuelve `false` si no tiene ninguno de los dos.
    pub fn enable_msi(&self, apic_id: u8, vector: u8) -> bool {
        let caps = self.capabilities();
        if let Some(&(_, at)) = caps.iter().find(|c| c.0 == CAP_MSIX)
            && self.enable_msix_at(at, apic_id, vector)
        {
            return true;
        }
        let Some(&(_, at)) = caps.iter().find(|c| c.0 == CAP_MSI) else {
            return false;
        };
        let control = (self.read(at) >> 16) as u16;
        let layout = MsiLayout::new(control);
        let address = msi_address(apic_id);
        self.write(at + 4, address as u32);
        if let Some(hi) = layout.address_high {
            self.write(at + hi, (address >> 32) as u32);
        }
        self.write16(at + layout.data, msi_data(vector) as u16);
        if let Some(mask) = layout.mask {
            self.write(at + mask, 0);
        }
        self.write16(at + 2, msi_enable(control));
        let command = self.read(0x04) & 0xFFFF;
        self.write(0x04, command | INTX_DISABLE);
        true
    }

    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    fn enable_msix_at(&self, at: u8, apic_id: u8, vector: u8) -> bool {
        let control = (self.read(at) >> 16) as u16;
        let t = MsixTable::new(control, self.read(at + 4));
        let Some(bar) = self.bar_address(t.bar) else {
            return false;
        };
        let bytes = t.entries as usize * 16;
        let Some(table) = paging::map_mmio(bar + t.offset as u64, bytes) else {
            return false;
        };
        let entry = msix_entry(apic_id, vector);
        for i in 0..t.entries as usize {
            for (w, value) in entry.iter().enumerate() {
                // SAFETY: la tabla de MSI-X está en el BAR del dispositivo y se acaba de mapear
                // sin caché; cada entrada son 4 palabras de 32 bits alineadas.
                unsafe {
                    core::ptr::write_volatile(table.add(i * 16 + w * 4) as *mut u32, *value);
                }
            }
        }
        self.write16(at + 2, msix_enable(control));
        let command = self.read(0x04) & 0xFFFF;
        self.write(0x04, command | INTX_DISABLE);
        true
    }

    /// BAR `n` (Base Address Register): dónde quedaron mapeados sus registros.
    pub fn bar(&self, n: u8) -> u32 {
        self.read(0x10 + n * 4)
    }

    pub fn read8(&self, offset: u8) -> u8 {
        (self.read(offset & !3) >> ((offset & 3) * 8)) as u8
    }

    /// Dirección física de un BAR de memoria (de 32 o 64 bits). `None` si es de puertos o está
    /// vacío.
    pub fn bar_address(&self, n: u8) -> Option<u64> {
        let lo = self.bar(n);
        if lo & 1 != 0 {
            return None; // puertos de E/S
        }
        let addr = match (lo >> 1) & 0b11 {
            // 64 bits: la parte alta está en el BAR siguiente.
            0b10 => (lo as u64 & !0xF) | ((self.bar(n + 1) as u64) << 32),
            _ => lo as u64 & !0xF,
        };
        (addr != 0).then_some(addr)
    }

    /// Lista de "capacidades" del dispositivo: (id, desplazamiento en el espacio de
    /// configuración). virtio moderno describe ahí dónde están sus registros.
    pub fn capabilities(&self) -> alloc::vec::Vec<(u8, u8)> {
        let mut out = alloc::vec::Vec::new();
        // Bit 4 del registro de estado: hay lista de capacidades.
        if (self.read(0x04) >> 16) & 0x10 == 0 {
            return out;
        }
        let mut ptr = self.read8(0x34) & !3;
        while ptr != 0 && out.len() < 48 {
            out.push((self.read8(ptr), ptr));
            ptr = self.read8(ptr + 1) & !3;
        }
        out
    }

    /// Habilita el acceso a sus registros en memoria (bit 1) y el DMA (bus master, bit 2). Estos
    /// dispositivos (video, micrófono) se atienden por polling: se les apaga la interrupción
    /// INTx (bit 10), porque pueden compartir la línea con el disco o la red y una interrupción
    /// "por nivel" que nadie atiende se repetiría sin fin.
    pub fn enable_memory_and_dma(&self) {
        let command = self.read(0x04) & 0xFFFF;
        write32(
            self.bus,
            self.slot,
            self.function,
            0x04,
            command | 0b110 | INTX_DISABLE,
        );
    }

    /// Habilita el acceso por puertos de E/S (bit 0) y que el dispositivo lea/escriba la memoria
    /// por su cuenta, sin pasar por la CPU (bus master, bit 2): eso es DMA. Deja la interrupción
    /// INTx encendida (disco y red avisan por ahí, K9).
    pub fn enable_io_and_dma(&self) {
        let command = self.read(0x04) & 0xFFFF;
        write32(
            self.bus,
            self.slot,
            self.function,
            0x04,
            (command | 0b101) & !INTX_DISABLE,
        );
    }

    /// La línea del PIC por la que avisa (la anotó el firmware en el registro 0x3C). `None` si
    /// no tiene (0xFF) o no es una línea que el kernel pueda compartir.
    pub fn interrupt_line(&self) -> Option<u8> {
        let line = self.read8(0x3C);
        (line < 16).then_some(line)
    }
}

/// Todos los dispositivos del bus (recorre los 256 buses; los vacíos responden 0xFFFF).
pub fn devices() -> Vec<Device> {
    let mut out = Vec::new();
    for bus in 0..=255u8 {
        for slot in 0..32u8 {
            if read32(bus, slot, 0, 0) as u16 == 0xFFFF {
                continue;
            }
            let functions = if read32(bus, slot, 0, 0x0C) >> 16 & 0x80 != 0 {
                8
            } else {
                1
            };
            for function in 0..functions {
                if read32(bus, slot, function, 0) as u16 != 0xFFFF {
                    out.push(Device {
                        bus,
                        slot,
                        function,
                    });
                }
            }
        }
    }
    out
}

/// Busca el primer dispositivo con ese fabricante y modelo.
pub fn find(vendor: u16, device: u16) -> Option<Device> {
    devices()
        .into_iter()
        .find(|d| d.vendor() == vendor && d.device_id() == device)
}

#[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
/// Los dispositivos de esa clase, subclase e interfaz.
pub fn find_class(class: u8, subclass: u8, prog_if: u8) -> Vec<Device> {
    devices()
        .into_iter()
        .filter(|d| d.class() == (class, subclass, prog_if))
        .collect()
}

/// Lee el espacio de configuración con el mecanismo de puertos (los primeros 256 bytes).
pub fn read_config(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    read32(bus, slot, function, offset)
}

/// Escribe el espacio de configuración (los primeros 256 bytes).
pub fn write_config(bus: u8, slot: u8, function: u8, offset: u8, value: u32) {
    write32(bus, slot, function, offset, value);
}
