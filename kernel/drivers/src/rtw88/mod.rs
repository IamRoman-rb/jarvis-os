//! La placa Wi-Fi Realtek RTL8821CE (K14, ADR 0012): todo lo que no es tocar la memoria.
//!
//! Es la placa de la PC de Roman (PCI 10EC:C821, 802.11ac de una antena). El port sigue al
//! driver `rtw88` de Linux, que Realtek escribió y publicó con licencia dual GPL-2.0 /
//! BSD-3-Clause (acá se usa bajo la BSD-3-Clause; los valores de las tablas y de los registros
//! vienen de ahí). Una placa así es *SoftMAC*: hace la radio y poco más, y para eso necesita:
//!
//! 1. **Encenderse** con una secuencia de escrituras y esperas ([`pwrseq`]).
//! 2. **Leer su efuse** ([`efuse`]): la dirección MAC y la calibración de fábrica (potencia,
//!    cristal, tipo de antena).
//! 3. **Cargar su firmware** ([`fw`]): el procesador de la placa corre un programa de Realtek
//!    (lo baja `cargo xtask`, no está en el repositorio). Se le manda por la cola de beacons y un
//!    DMA interno lo copia a su memoria; después habla con el sistema por mensajes H2C/C2H.
//! 4. **Configurar la MAC, la banda base y la radio** con las tablas de Realtek ([`phy`],
//!    `tablas.rs`).
//! 5. Mandar y recibir tramas 802.11 con un descriptor adelante ([`desc`]).
//!
//! [`chip::Rtw8821c`] junta todo sobre un [`Bus`]: el kernel lo implementa con los registros de
//! verdad y los anillos de DMA (kernel/src/rtw88.rs); los tests, con un chip simulado. **QEMU
//! no emula ninguna placa Wi-Fi**: el driver se verifica en la PC, con el registro del arranque.

pub mod chip;
pub mod desc;
pub mod efuse;
pub mod fw;
pub mod phy;
pub mod pwrseq;
pub mod regs;
mod tablas;

/// Una cola de transmisión de la placa (cada una con su anillo de descriptores en la PCI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    /// Mejor esfuerzo: los datos.
    Be,
    /// Gestión: sondeo, autenticación, asociación.
    Mgmt,
    /// Comandos para el firmware (H2C en paquete).
    H2c,
    /// La de beacons: sirve para escribir en la memoria reservada de la placa (el firmware).
    Bcn,
}

/// Lo que el driver necesita de la placa: registros, esperas y las colas de transmisión.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, v: u8);
    fn write16(&mut self, addr: u32, v: u16);
    fn write32(&mut self, addr: u32, v: u32);
    fn delay_us(&mut self, us: u32);
    /// Pone `packet` (descriptor de 48 bytes + datos) en la cola y avisa a la placa. `false` si
    /// la cola está llena.
    fn tx(&mut self, queue: Queue, packet: &[u8]) -> bool;
    /// Vuelve a dar los anillos de DMA a la placa (direcciones y largos, índices en cero). Se
    /// hace al encender y después de cargar el firmware (`rtw_pci_setup` en Linux).
    fn hci_setup(&mut self);
}

/// Atajos sobre un [`Bus`] (los `rtw_write32_mask` y compañía de Linux).
pub(crate) trait BusExt: Bus {
    fn set8(&mut self, a: u32, bits: u8) {
        let v = self.read8(a);
        self.write8(a, v | bits);
    }
    fn clr8(&mut self, a: u32, bits: u8) {
        let v = self.read8(a);
        self.write8(a, v & !bits);
    }
    fn set32(&mut self, a: u32, bits: u32) {
        let v = self.read32(a);
        self.write32(a, v | bits);
    }
    fn clr32(&mut self, a: u32, bits: u32) {
        let v = self.read32(a);
        self.write32(a, v & !bits);
    }
    /// El campo `mask` de un registro, corrido a la derecha.
    fn read32_mask(&mut self, a: u32, mask: u32) -> u32 {
        (self.read32(a) & mask) >> mask.trailing_zeros()
    }
    /// Escribe `v` en el campo `mask` (sin tocar el resto del registro).
    fn write32_mask(&mut self, a: u32, mask: u32, v: u32) {
        if mask == u32::MAX {
            self.write32(a, v);
            return;
        }
        let old = self.read32(a);
        self.write32(a, (old & !mask) | ((v << mask.trailing_zeros()) & mask));
    }
    fn write8_mask(&mut self, a: u32, mask: u8, v: u8) {
        let old = self.read8(a);
        self.write8(a, (old & !mask) | ((v << mask.trailing_zeros()) & mask));
    }
    /// Espera hasta 10 ms a que el campo `mask` valga `target` (`check_hw_ready`).
    fn wait32(&mut self, a: u32, mask: u32, target: u32) -> bool {
        for _ in 0..1000 {
            if self.read32_mask(a, mask) == target {
                return true;
            }
            self.delay_us(10);
        }
        false
    }
}

impl<B: Bus + ?Sized> BusExt for B {}

/// Por qué no arrancó la placa (para el registro del arranque).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Un paso de la secuencia de encendido no respondió.
    Power(u16),
    /// El efuse no se pudo leer o está mal formado.
    Efuse,
    /// El firmware: archivo inválido, la copia falló o no arrancó.
    Firmware(&'static str),
    /// La configuración de la MAC (colas, memoria de la placa).
    Mac(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Power(at) => write!(f, "no encendió (paso {at:#06x})"),
            Error::Efuse => write!(f, "no se pudo leer el efuse"),
            Error::Firmware(why) => write!(f, "firmware: {why}"),
            Error::Mac(why) => write!(f, "MAC: {why}"),
        }
    }
}
