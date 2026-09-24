//! Bus PCI: cómo se encuentran los dispositivos (disco, red, video…) conectados a la máquina.
//!
//! Cada dispositivo PCI tiene un "espacio de configuración" de 256 bytes con su fabricante,
//! modelo, recursos (BAR: rangos de puertos o memoria) y un registro de comando. Se accede con el
//! mecanismo clásico de dos puertos: se escribe la dirección (bus/dispositivo/función/registro) en
//! 0xCF8 y se lee o escribe el dato en 0xCFC.
//! Referencia: <https://wiki.osdev.org/PCI>.

use x86_64::instructions::port::Port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Clone, Copy, Debug)]
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

impl Device {
    pub fn read(&self, offset: u8) -> u32 {
        read32(self.bus, self.slot, self.function, offset)
    }

    /// BAR `n` (Base Address Register): dónde quedaron mapeados sus registros.
    pub fn bar(&self, n: u8) -> u32 {
        self.read(0x10 + n * 4)
    }

    /// Habilita el acceso por puertos de E/S (bit 0) y que el dispositivo lea/escriba la memoria
    /// por su cuenta, sin pasar por la CPU (bus master, bit 2): eso es DMA.
    pub fn enable_io_and_dma(&self) {
        let command = self.read(0x04) & 0xFFFF;
        write32(self.bus, self.slot, self.function, 0x04, command | 0b101);
    }
}

/// Busca el primer dispositivo con ese fabricante y modelo.
pub fn find(vendor: u16, device: u16) -> Option<Device> {
    for bus in 0..=255u8 {
        for slot in 0..32u8 {
            let functions = if read32(bus, slot, 0, 0x0C) >> 16 & 0x80 != 0 {
                8
            } else {
                1
            };
            for function in 0..functions {
                let id = read32(bus, slot, function, 0);
                if id as u16 == vendor && (id >> 16) as u16 == device {
                    return Some(Device {
                        bus,
                        slot,
                        function,
                    });
                }
            }
        }
    }
    None
}
