//! Driver de la placa de red Realtek RTL8139 (K13).
//!
//! Hardware: 10EC:8139, una placa PCI de los 2000 que QEMU emula (`-device rtl8139`). No es la
//! Realtek de la PC de Roman (esa es la 8168, rtl8169.rs), pero sirve para probar en QEMU una
//! placa sin MSI: el driver no usa interrupciones y la tarea de la red la revisa cada tanto.
//! Recibe en un anillo de 8 KiB (no por descriptores) y manda por 4 ranuras fijas.
//! Referencias: RTL8139D datasheet y <https://wiki.osdev.org/RTL8139>.

use alloc::vec::Vec;

use jarvis_drivers::nic::{RTL8139_RX_ALLOC, rtl8139_capr, rtl8139_rx};
use x86_64::instructions::port::Port;

use crate::nic::Frames;
use crate::{dma, pci, serial_println, time};

const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CMD: u16 = 0x37;
const CAPR: u16 = 0x38;
const IMR: u16 = 0x3C;
const ISR: u16 = 0x3E;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;

const CMD_BUFE: u8 = 1 << 0;
const CMD_TE: u8 = 1 << 2;
const CMD_RE: u8 = 1 << 3;
const CMD_RST: u8 = 1 << 4;
/// TSD: la placa terminó de copiar la trama (el búfer vuelve a ser nuestro).
const TSD_OWN: u32 = 1 << 13;

const TX_SLOTS: usize = 4;
const TX_BUF: usize = 1536;

pub struct Rtl8139 {
    io: u16,
    mac: [u8; 6],
    rx: *mut u8,
    rx_offset: usize,
    tx: *mut u8,
    tx_next: usize,
}

// SAFETY: los punteros son memoria DMA propia de la placa; la usa solo la tarea de la red.
unsafe impl Send for Rtl8139 {}

pub fn probe() -> Option<Rtl8139> {
    let dev = pci::find(0x10EC, 0x8139)?;
    let bar = dev.bar(0);
    if bar & 1 == 0 {
        return None;
    }
    dev.enable_io_and_dma();
    Rtl8139::start(dev, (bar & 0xFFFC) as u16)
}

impl Rtl8139 {
    fn out8(&self, reg: u16, v: u8) {
        // SAFETY: `io + reg` es un registro de esta placa (BAR0, puertos de E/S).
        unsafe { Port::<u8>::new(self.io + reg).write(v) }
    }
    fn out16(&self, reg: u16, v: u16) {
        // SAFETY: ídem `out8`.
        unsafe { Port::<u16>::new(self.io + reg).write(v) }
    }
    fn out32(&self, reg: u16, v: u32) {
        // SAFETY: ídem `out8`.
        unsafe { Port::<u32>::new(self.io + reg).write(v) }
    }
    fn in8(&self, reg: u16) -> u8 {
        // SAFETY: ídem `out8`.
        unsafe { Port::<u8>::new(self.io + reg).read() }
    }
    fn in32(&self, reg: u16) -> u32 {
        // SAFETY: ídem `out8`.
        unsafe { Port::<u32>::new(self.io + reg).read() }
    }

    fn start(dev: pci::Device, io: u16) -> Option<Rtl8139> {
        let rx = dma::alloc(RTL8139_RX_ALLOC, 4096)?;
        let tx = dma::alloc(TX_SLOTS * TX_BUF, 4096)?;
        // El 8139 solo tiene direcciones de 32 bits.
        if (dma::phys(rx) | dma::phys(tx)) >> 32 != 0 {
            serial_println!("RTL8139: los buffers quedaron arriba de 4 GiB");
            return None;
        }
        let mut n = Rtl8139 {
            io,
            mac: [0; 6],
            rx,
            rx_offset: 0,
            tx,
            tx_next: 0,
        };
        n.out8(CONFIG1, 0); // encender
        n.out8(CMD, CMD_RST);
        let t = time::millis();
        while n.in8(CMD) & CMD_RST != 0 && time::millis() - t < 100 {}
        for i in 0..6 {
            n.mac[i] = n.in8(IDR0 + i as u16);
        }
        n.out32(RBSTART, dma::phys(rx) as u32);
        for i in 0..TX_SLOTS {
            n.out32(
                TSAD0 + 4 * i as u16,
                (dma::phys(tx) + (i * TX_BUF) as u64) as u32,
            );
        }
        n.out16(IMR, 0); // sin interrupciones (no hay MSI en el 8139)
        // RCR: a nuestra MAC, multicast y broadcast; WRAP: la trama que no entra al final del
        // anillo sigue de largo (por eso el margen de 1536 bytes) en vez de dar la vuelta.
        n.out32(RCR, 0b1110 | 1 << 7);
        n.out8(CMD, CMD_RE | CMD_TE);
        serial_println!(
            "RTL8139 {:02x}:{:02x}.{}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dev.bus,
            dev.slot,
            dev.function,
            n.mac[0],
            n.mac[1],
            n.mac[2],
            n.mac[3],
            n.mac[4],
            n.mac[5]
        );
        Some(n)
    }

    /// Reinicia la recepción (después de una cabecera inválida en el anillo).
    fn reset_rx(&mut self) {
        self.out8(CMD, CMD_TE);
        self.rx_offset = 0;
        self.out32(RBSTART, dma::phys(self.rx) as u32);
        self.out8(CMD, CMD_RE | CMD_TE);
    }
}

impl Frames for Rtl8139 {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.out16(ISR, 0xFFFF);
        if self.in8(CMD) & CMD_BUFE != 0 {
            return None;
        }
        // SAFETY: el anillo mide RTL8139_RX_ALLOC bytes y la placa lo escribe por DMA.
        let ring = unsafe { core::slice::from_raw_parts(self.rx, RTL8139_RX_ALLOC) };
        let Some((start, len, next)) = rtl8139_rx(ring, self.rx_offset) else {
            serial_println!("RTL8139: cabecera inválida en el anillo; se reinicia la recepción");
            self.reset_rx();
            return None;
        };
        let frame = ring[start..start + len].to_vec();
        self.rx_offset = next;
        self.out16(CAPR, rtl8139_capr(next));
        Some(frame)
    }

    fn can_send(&mut self) -> bool {
        self.in32(TSD0 + 4 * self.tx_next as u16) & TSD_OWN != 0
    }

    fn send(&mut self, frame: &[u8]) {
        let i = self.tx_next;
        let len = frame.len().clamp(60, TX_BUF);
        // SAFETY: la ranura `i` mide TX_BUF bytes y está libre (`can_send`).
        unsafe {
            core::ptr::write_bytes(self.tx.add(i * TX_BUF), 0, len);
            core::ptr::copy_nonoverlapping(
                frame.as_ptr(),
                self.tx.add(i * TX_BUF),
                frame.len().min(TX_BUF),
            );
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // Escribir el largo (con OWN en 0) arranca la copia.
        self.out32(TSD0 + 4 * i as u16, len as u32);
        self.tx_next = (i + 1) % TX_SLOTS;
    }
}
