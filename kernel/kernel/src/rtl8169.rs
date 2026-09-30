//! Driver de las placas de red Realtek RTL8168/8111 (K13), la familia del driver `r8169` de
//! Linux.
//!
//! Hardware: 10EC:8168 (RTL8111/8168 en todas sus revisiones, PCI Express), la placa de red de
//! la B550M DS3H AC de Roman, y 10EC:8161/8169. **QEMU no la emula**: este driver sigue el
//! datasheet y la secuencia mínima de `rtl_hw_start_8168` de Linux, y se verifica recién en la PC
//! (el registro del arranque dice la revisión del chip y el estado del enlace). Las revisiones
//! nuevas (8168h/8111h) tienen ajustes de consumo y del PHY que Linux aplica y acá no: si el
//! enlace no sube, eso es lo primero a revisar.
//! Descriptores de 16 bytes (`jarvis_drivers::nic`), 32 de cada lado, avisos por MSI.
//! Referencias: "RTL8168/8111 Programming Guide" (Realtek), `drivers/net/ethernet/realtek/r8169_main.c`
//! de Linux y <https://wiki.osdev.org/RTL8169>.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use jarvis_drivers::nic::{R8169_EOR, R8169_OWN, r8169_rx_desc, r8169_rx_done, r8169_tx_desc};

use crate::mmio::Mmio;
use crate::nic::Frames;
use crate::{dma, interrupts, pci, serial_println, task, time};

const REALTEK: u16 = 0x10EC;
const MODELS: [u16; 3] = [0x8168, 0x8161, 0x8169];

const MAC0: usize = 0x00;
const MAR0: usize = 0x08;
const TX_DESC: usize = 0x20;
const CHIP_CMD: usize = 0x37;
const TX_POLL: usize = 0x38;
const INTR_MASK: usize = 0x3C;
const INTR_STATUS: usize = 0x3E;
const TX_CONFIG: usize = 0x40;
const RX_CONFIG: usize = 0x44;
const CFG9346: usize = 0x50;
const PHY_STATUS: usize = 0x6C;
const RX_MAX_SIZE: usize = 0xDA;
const INTR_MITIGATE: usize = 0xE2;
const RX_DESC: usize = 0xE4;
const MAX_TX_PACKET: usize = 0xEC;

const CMD_RESET: u8 = 0x10;
const CMD_RX: u8 = 0x08;
const CMD_TX: u8 = 0x04;

const RING: usize = 32;
const BUF: usize = 2048;

pub struct Rtl8169 {
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
unsafe impl Send for Rtl8169 {}

pub fn probe() -> Option<Rtl8169> {
    let dev = pci::devices()
        .into_iter()
        .find(|d| d.vendor() == REALTEK && MODELS.contains(&d.device_id()))?;
    let n = Rtl8169::start(dev);
    if n.is_none() {
        serial_println!("RTL8168: no se pudo iniciar");
    }
    n
}

fn opts1(ring: *mut u8, i: usize) -> u32 {
    // SAFETY: `i < RING`; la placa escribe el descriptor por DMA.
    unsafe { core::ptr::read_volatile(ring.add(i * 16) as *const u32) }
}

/// Escribe un descriptor: primero la dirección, después `opts1` (con el bit OWN), para que la
/// placa nunca vea un descriptor suyo a medio escribir.
fn write_desc(ring: *mut u8, i: usize, d: [u8; 16]) {
    // SAFETY: `i < RING`; cada anillo mide RING × 16 bytes.
    unsafe {
        let p = ring.add(i * 16);
        core::ptr::write_volatile(
            p.add(4) as *mut [u8; 12],
            d[4..].try_into().unwrap_or([0; 12]),
        );
        fence(Ordering::SeqCst);
        core::ptr::write_volatile(p as *mut u32, u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
    }
}

impl Rtl8169 {
    fn start(dev: pci::Device) -> Option<Rtl8169> {
        // Los registros en memoria: el BAR 2 en las 8168 (el 0 es de puertos), el 1 en la 8169.
        let bar = dev.bar_address(2).or_else(|| dev.bar_address(1))?;
        dev.enable_memory_and_dma();
        let regs = Mmio::map(bar, 0x100)?;
        // TxConfig bits 20–30 dicen la revisión del chip (la "XID" de Linux).
        let xid = (regs.r32(TX_CONFIG) >> 20) & 0x7CF;
        regs.w8(CHIP_CMD, CMD_RESET);
        let t = time::millis();
        while regs.r8(CHIP_CMD) & CMD_RESET != 0 && time::millis() - t < 100 {}
        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = regs.r8(MAC0 + i);
        }

        let rx = dma::alloc(RING * 16, 256)?;
        let rx_bufs = dma::alloc(RING * BUF, 4096)?;
        let tx = dma::alloc(RING * 16, 256)?;
        let tx_bufs = dma::alloc(RING * BUF, 4096)?;
        for i in 0..RING {
            let last = i == RING - 1;
            write_desc(
                rx,
                i,
                r8169_rx_desc(dma::phys(rx_bufs) + (i * BUF) as u64, BUF as u16, last),
            );
            // Envío: libres (sin OWN); el último marca el fin del anillo.
            let eor = if last { R8169_EOR } else { 0 };
            write_desc(tx, i, jarvis_drivers::nic::r8169_desc(eor, 0));
        }

        regs.w8(CFG9346, 0xC0); // desbloquear la configuración
        regs.w16(RX_MAX_SIZE, 1536);
        regs.w8(MAX_TX_PACKET, 0x3B);
        regs.w16(INTR_MITIGATE, 0);
        regs.w64_split(TX_DESC, dma::phys(tx));
        regs.w64_split(RX_DESC, dma::phys(rx));
        // Algunas revisiones piden habilitar recepción y envío antes de configurarlos.
        regs.w8(CHIP_CMD, CMD_RX | CMD_TX);
        // TxConfig: separación estándar entre tramas (IFG), ráfagas de DMA sin límite.
        regs.w32(TX_CONFIG, 0x0300_0700);
        // RxConfig: umbral de FIFO y DMA sin límite; nuestra MAC, multicast y broadcast.
        regs.w32(RX_CONFIG, 0x0000_E70E);
        regs.w32(MAR0, u32::MAX);
        regs.w32(MAR0 + 4, u32::MAX);
        regs.w8(CFG9346, 0x00);

        let msi = interrupts::enable_msi(dev, task::EV_NET, false);
        regs.w16(INTR_STATUS, 0xFFFF);
        // Recibió, error al recibir, mandó, error al mandar, sin descriptores, cambió el enlace.
        regs.w16(INTR_MASK, if msi { 0x3F } else { 0 });
        serial_println!(
            "RTL8168 {:02x}:{:02x}.{}: revisión {:#05x}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, enlace {}, MSI: {}",
            dev.bus,
            dev.slot,
            dev.function,
            xid,
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            if regs.r8(PHY_STATUS) & 0x02 != 0 {
                "arriba"
            } else {
                "abajo (puede tardar unos segundos)"
            },
            if msi { "sí" } else { "no" }
        );
        Some(Rtl8169 {
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

impl Frames for Rtl8169 {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        // Bajar los avisos pendientes: si no, no llega el próximo MSI.
        let status = self.regs.r16(INTR_STATUS);
        if status != 0 {
            self.regs.w16(INTR_STATUS, status);
        }
        loop {
            let i = self.rx_next;
            let len = r8169_rx_done(opts1(self.rx, i))?;
            fence(Ordering::SeqCst);
            // SAFETY: el buffer `i` mide BUF bytes y la placa escribió `len` (≤ BUF).
            let frame = len.map(|len| unsafe {
                core::slice::from_raw_parts(self.rx_bufs.add(i * BUF), len.min(BUF)).to_vec()
            });
            let last = i == RING - 1;
            write_desc(
                self.rx,
                i,
                r8169_rx_desc(dma::phys(self.rx_bufs) + (i * BUF) as u64, BUF as u16, last),
            );
            self.rx_next = (i + 1) % RING;
            if frame.is_some() {
                return frame;
            }
        }
    }

    fn can_send(&mut self) -> bool {
        opts1(self.tx, self.tx_next) & R8169_OWN == 0
    }

    fn send(&mut self, frame: &[u8]) {
        let i = self.tx_next;
        let len = frame.len().clamp(60, BUF);
        // SAFETY: el buffer `i` mide BUF bytes y está libre (`can_send`).
        unsafe {
            core::ptr::write_bytes(self.tx_bufs.add(i * BUF), 0, len);
            core::ptr::copy_nonoverlapping(
                frame.as_ptr(),
                self.tx_bufs.add(i * BUF),
                frame.len().min(BUF),
            );
        }
        write_desc(
            self.tx,
            i,
            r8169_tx_desc(
                dma::phys(self.tx_bufs) + (i * BUF) as u64,
                len as u16,
                i == RING - 1,
            ),
        );
        self.tx_next = (i + 1) % RING;
        self.regs.w8(TX_POLL, 0x40); // "hay tramas nuevas en la cola normal"
    }
}
