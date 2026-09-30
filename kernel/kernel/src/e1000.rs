//! Driver de placas de red Intel de la familia e1000 (K13).
//!
//! Hardware: 82540EM (la `e1000` de QEMU y de VirtualBox), 82545EM y 82574L (la `e1000e` de
//! QEMU, PCI Express). Las I217/I218/I219 de las placas madre Intel usan el mismo formato de
//! descriptores, pero su PHY y su modo de bajo consumo piden una inicialización distinta: quedan
//! para más adelante. Descriptores "legacy" de 16 bytes (`jarvis_drivers::nic`), 32 de cada lado.
//! Referencias: "PCI/PCI-X Family of Gigabit Ethernet Controllers Software Developer's Manual"
//! (8254x) §14 (inicialización), datasheet del 82574 §10, <https://wiki.osdev.org/Intel_Ethernet_i217>.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use jarvis_drivers::nic::{e1000_rx_desc, e1000_rx_done, e1000_tx_desc, e1000_tx_done};

use crate::mmio::Mmio;
use crate::nic::Frames;
use crate::{dma, interrupts, pci, serial_println, task, time};

const INTEL: u16 = 0x8086;
/// 82540EM, 82545EM (cobre) y 82574L.
const MODELS: [u16; 3] = [0x100E, 0x100F, 0x10D3];
const MODEL_82574: u16 = 0x10D3;

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EERD: usize = 0x0014;
const ICR: usize = 0x00C0;
const IMS: usize = 0x00D0;
const IMC: usize = 0x00D8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const RDBAL: usize = 0x2800;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const MTA: usize = 0x5200;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_RST: u32 = 1 << 26;

const RING: usize = 32;
const BUF: usize = 2048;

pub struct E1000 {
    regs: Mmio,
    mac: [u8; 6],
    rx: *mut u8,
    rx_bufs: *mut u8,
    rx_next: usize,
    tx: *mut u8,
    tx_bufs: *mut u8,
    tx_next: usize,
}

// SAFETY: los punteros son memoria DMA propia de la placa; la usa solo la tarea de la red.
unsafe impl Send for E1000 {}

pub fn probe() -> Option<E1000> {
    let dev = pci::devices()
        .into_iter()
        .find(|d| d.vendor() == INTEL && MODELS.contains(&d.device_id()))?;
    let n = E1000::start(dev);
    if n.is_none() {
        serial_println!("e1000: no se pudo iniciar {:04x}", dev.device_id());
    }
    n
}

impl E1000 {
    fn start(dev: pci::Device) -> Option<E1000> {
        let bar = dev.bar_address(0)?;
        dev.enable_memory_and_dma();
        let regs = Mmio::map(bar, 0x20000)?;
        regs.w32(IMC, u32::MAX);
        regs.w32(CTRL, regs.r32(CTRL) | CTRL_RST);
        let t = time::millis();
        while regs.r32(CTRL) & CTRL_RST != 0 && time::millis() - t < 100 {}
        regs.w32(IMC, u32::MAX);
        let _ = regs.r32(ICR);
        // Enlace arriba y velocidad automática (lo negocia el PHY).
        regs.w32(CTRL, regs.r32(CTRL) | CTRL_SLU | CTRL_ASDE);

        let mac = read_mac(&regs, dev.device_id())?;
        for i in 0..128 {
            regs.w32(MTA + i * 4, 0);
        }

        let rx = dma::alloc(RING * 16, 128)?;
        let rx_bufs = dma::alloc(RING * BUF, 4096)?;
        let tx = dma::alloc(RING * 16, 128)?;
        let tx_bufs = dma::alloc(RING * BUF, 4096)?;
        for i in 0..RING {
            // SAFETY: cada anillo mide RING descriptores de 16 bytes.
            unsafe {
                write_desc(rx, i, e1000_rx_desc(dma::phys(rx_bufs) + (i * BUF) as u64));
                // Descriptores de envío "terminados" (DD): todos libres al empezar.
                let mut d = [0u8; 16];
                d[12] = 1;
                write_desc(tx, i, d);
            }
        }
        regs.w64_split(RDBAL, dma::phys(rx));
        regs.w32(RDLEN, (RING * 16) as u32);
        regs.w32(RDH, 0);
        regs.w32(RDT, RING as u32 - 1);
        // RCTL: habilitar, broadcast, sacar el CRC, buffers de 2 KiB.
        regs.w32(RCTL, 1 << 1 | 1 << 15 | 1 << 26);
        regs.w64_split(TDBAL, dma::phys(tx));
        regs.w32(TDLEN, (RING * 16) as u32);
        regs.w32(TDH, 0);
        regs.w32(TDT, 0);
        // TCTL: habilitar, rellenar paquetes cortos, umbral de colisiones y distancia.
        regs.w32(TCTL, 1 << 1 | 1 << 3 | 0x0F << 4 | 0x40 << 12);
        regs.w32(TIPG, 0x0060_200A);

        // MSI común (no MSI-X: con MSI-X el 82574 pide mapear cada causa a un vector).
        let msi = interrupts::enable_msi(dev, task::EV_NET, false);
        if msi {
            // Recibió (RXT0), cambió el enlace (LSC), se llenó (RXO, RXDMT0).
            regs.w32(IMS, 1 << 7 | 1 << 2 | 1 << 6 | 1 << 4);
        }
        serial_println!(
            "e1000 {:04x}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, enlace {}, MSI: {}",
            dev.device_id(),
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            if regs.r32(STATUS) & 2 != 0 {
                "arriba"
            } else {
                "abajo"
            },
            if msi {
                "sí"
            } else {
                "no (se revisa cada tanto)"
            }
        );
        Some(E1000 {
            regs,
            mac,
            rx,
            rx_bufs,
            rx_next: 0,
            tx,
            tx_bufs,
            tx_next: 0,
        })
    }
}

/// La MAC: la que dejó el firmware en RAL/RAH, o la de la EEPROM.
fn read_mac(regs: &Mmio, model: u16) -> Option<[u8; 6]> {
    let (lo, hi) = (regs.r32(RAL), regs.r32(RAH));
    if lo != 0 && hi & (1 << 31) != 0 {
        let b = lo.to_le_bytes();
        let h = hi.to_le_bytes();
        return Some([b[0], b[1], b[2], b[3], h[0], h[1]]);
    }
    // EERD: la dirección va en otro lugar y el "listo" es otro bit según el modelo.
    let (shift, done) = if model == MODEL_82574 {
        (2, 1 << 1)
    } else {
        (8, 1 << 4)
    };
    let mut mac = [0u8; 6];
    for word in 0..3u32 {
        regs.w32(EERD, 1 | word << shift);
        let t = time::millis();
        let v = loop {
            let v = regs.r32(EERD);
            if v & done != 0 {
                break v;
            }
            if time::millis() - t > 10 {
                return None;
            }
        };
        mac[word as usize * 2] = (v >> 16) as u8;
        mac[word as usize * 2 + 1] = (v >> 24) as u8;
    }
    // Anotarla en RAL/RAH (con "dirección válida") para que la placa acepte lo que es nuestro.
    regs.w32(RAL, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    regs.w32(RAH, u32::from_le_bytes([mac[4], mac[5], 0, 0]) | 1 << 31);
    Some(mac)
}

/// Escribe un descriptor de 16 bytes.
///
/// # Safety
/// `ring` tiene que tener lugar para el descriptor `i`.
unsafe fn write_desc(ring: *mut u8, i: usize, d: [u8; 16]) {
    // SAFETY: lo garantiza el llamador; se escribe con `volatile` porque lo lee la placa.
    unsafe { core::ptr::write_volatile(ring.add(i * 16) as *mut [u8; 16], d) }
}

fn read_desc(ring: *mut u8, i: usize) -> [u8; 16] {
    // SAFETY: `i < RING` (los índices se toman módulo RING); la placa lo escribe por DMA.
    unsafe { core::ptr::read_volatile(ring.add(i * 16) as *const [u8; 16]) }
}

impl Frames for E1000 {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        // Leer ICR baja la interrupción: así la placa puede mandar el próximo MSI.
        let _ = self.regs.r32(ICR);
        loop {
            let i = self.rx_next;
            let len = e1000_rx_done(&read_desc(self.rx, i))?;
            fence(Ordering::SeqCst);
            // SAFETY: el buffer `i` mide BUF bytes y la placa escribió `len` (≤ BUF).
            let frame = len.map(|len| unsafe {
                core::slice::from_raw_parts(self.rx_bufs.add(i * BUF), len.min(BUF)).to_vec()
            });
            // Devolver el descriptor a la placa.
            // SAFETY: `i < RING`.
            unsafe {
                write_desc(
                    self.rx,
                    i,
                    e1000_rx_desc(dma::phys(self.rx_bufs) + (i * BUF) as u64),
                )
            };
            self.regs.w32(RDT, i as u32);
            self.rx_next = (i + 1) % RING;
            if frame.is_some() {
                return frame;
            }
        }
    }

    fn can_send(&mut self) -> bool {
        e1000_tx_done(&read_desc(self.tx, self.tx_next))
    }

    fn send(&mut self, frame: &[u8]) {
        let i = self.tx_next;
        let len = frame.len().min(BUF);
        // SAFETY: el buffer `i` mide BUF bytes y está libre (`can_send`); `i < RING`.
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), self.tx_bufs.add(i * BUF), len);
            write_desc(
                self.tx,
                i,
                e1000_tx_desc(dma::phys(self.tx_bufs) + (i * BUF) as u64, len as u16),
            );
        }
        fence(Ordering::SeqCst);
        self.tx_next = (i + 1) % RING;
        self.regs.w32(TDT, self.tx_next as u32);
    }
}
