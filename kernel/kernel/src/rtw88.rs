//! Driver de la placa Wi-Fi Realtek RTL8821CE (K14, ADR 0012): la mitad que toca el hardware.
//!
//! Hardware: PCI 10EC:C821, la placa Wi-Fi de la PC de Roman. Sus registros están en el BAR 2
//! (64 KiB de memoria) y se habla con ella por anillos de *buffer descriptors* en la RAM: uno
//! por cola de transmisión (datos, gestión, comandos H2C, beacons) y uno de recepción. Todo lo
//! demás (encendido, efuse, firmware, tablas, canales, descriptores) está en
//! `jarvis_drivers::rtw88`; acá solo se implementa su [`Bus`]. **QEMU no la emula**: se
//! verifica en la PC, con el registro del arranque.
//!
//! La placa solo ve direcciones de 32 bits: los anillos salen de `dma::alloc32`.
//! Referencia: `drivers/net/wireless/realtek/rtw88/pci.c` de Linux.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use jarvis_drivers::rtw88::desc::{self, RX_BUF_SIZE};
use jarvis_drivers::rtw88::{Bus, Queue, regs};

use crate::mmio::Mmio;
use crate::{dma, interrupts, pci, serial_println, task, time};

const REALTEK: u16 = 0x10ec;
const RTL8821CE: u16 = 0xc821;

/// Tamaño de cada buffer de transmisión (descriptor + la trama más grande, o un pedazo de 4 KiB
/// del firmware en la cola de beacons).
const TX_BUF: usize = 2048;
const BCN_BUF: usize = desc::TX_DESC_SIZE + 4096;
const RX_BUF: usize = RX_BUF_SIZE.next_multiple_of(64);
const RX_RING: usize = 32;

/// Un anillo de transmisión: los *buffer descriptors* (16 bytes cada uno) y sus buffers.
struct TxRing {
    bd: *mut u8,
    bufs: *mut u8,
    len: usize,
    buf_size: usize,
    wp: usize,
    /// Registros: dirección del anillo, cantidad e índices.
    desa: u32,
    num: u32,
    idx: u32,
}

struct RxRing {
    bd: *mut u8,
    bufs: *mut u8,
    rp: usize,
}

pub struct PciBus {
    regs: Mmio,
    be: TxRing,
    mgmt: TxRing,
    h2c: TxRing,
    bcn: TxRing,
    rx: RxRing,
}

// SAFETY: los punteros son memoria DMA propia de la placa; la usa solo la tarea de la red.
unsafe impl Send for PciBus {}

fn tx_ring(len: usize, buf_size: usize, desa: u32, num: u32, idx: u32) -> Option<TxRing> {
    Some(TxRing {
        bd: dma::alloc32(len * 16, 256)?,
        bufs: dma::alloc32(len * buf_size, 4096)?,
        len,
        buf_size,
        wp: 0,
        desa,
        num,
        idx,
    })
}

impl PciBus {
    fn new(regs: Mmio) -> Option<PciBus> {
        let rx = RxRing {
            bd: dma::alloc32(RX_RING * 8, 256)?,
            bufs: dma::alloc32(RX_RING * RX_BUF, 4096)?,
            rp: 0,
        };
        let mut bus = PciBus {
            regs,
            be: tx_ring(
                64,
                TX_BUF,
                regs::PCI_TXBD_DESA_BEQ,
                regs::PCI_TXBD_NUM_BEQ,
                regs::PCI_TXBD_IDX_BEQ,
            )?,
            mgmt: tx_ring(
                16,
                TX_BUF,
                regs::PCI_TXBD_DESA_MGMTQ,
                regs::PCI_TXBD_NUM_MGMTQ,
                regs::PCI_TXBD_IDX_MGMTQ,
            )?,
            h2c: tx_ring(
                16,
                256,
                regs::PCI_TXBD_DESA_H2CQ,
                regs::PCI_TXBD_NUM_H2CQ,
                regs::PCI_TXBD_IDX_H2CQ,
            )?,
            bcn: tx_ring(1, BCN_BUF, regs::PCI_TXBD_DESA_BCNQ, 0, 0)?,
            rx,
        };
        for i in 0..RX_RING {
            bus.arm_rx(i);
        }
        Some(bus)
    }

    fn arm_rx(&mut self, i: usize) {
        // SAFETY: `i < RX_RING`: el buffer y su descriptor son de este anillo.
        let buf = unsafe { self.rx.bufs.add(i * RX_BUF) };
        let bd = desc::rx_bd(dma::phys(buf));
        // SAFETY: el descriptor `i` mide 8 bytes dentro del anillo.
        unsafe { core::ptr::write_volatile(self.rx.bd.add(i * 8) as *mut [u8; 8], bd) };
    }

    /// Las tramas que llegaron desde la última vez: (copia del buffer, hasta lo que ocupa).
    pub fn take_rx(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let hw = (self.regs.r32(regs::PCI_RXBD_IDX_MPDUQ as usize) >> 16) & 0xfff;
        while self.rx.rp != hw as usize % RX_RING {
            fence(Ordering::SeqCst);
            let i = self.rx.rp;
            // SAFETY: la placa terminó de escribir el buffer `i` (avanzó su índice).
            let buf = unsafe { core::slice::from_raw_parts(self.rx.bufs.add(i * RX_BUF), RX_BUF) };
            // Solo lo que ocupa: descriptor + estado de la PHY + trama.
            let w0 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let used = desc::RX_DESC_SIZE
                + ((w0 >> 16) & 0xf) as usize * 8
                + ((w0 >> 24) & 3) as usize
                + (w0 & 0x3fff) as usize;
            out.push(buf[..used.min(RX_BUF)].to_vec());
            self.arm_rx(i);
            self.rx.rp = (i + 1) % RX_RING;
            self.regs
                .w16(regs::PCI_RXBD_IDX_MPDUQ as usize, self.rx.rp as u16);
        }
        out
    }

    /// Baja los avisos pendientes (con MSI, si quedara alguno no llegaría el próximo).
    pub fn ack_irqs(&mut self) {
        for (isr, mask) in [
            (regs::PCI_HISR0, regs::IMR0),
            (regs::PCI_HISR1, regs::IMR1),
            (regs::PCI_HISR3, regs::IMR3),
        ] {
            let v = self.regs.r32(isr as usize) & mask;
            if v != 0 {
                self.regs.w32(isr as usize, v);
            }
        }
    }

    pub fn enable_irqs(&mut self, on: bool) {
        let m = |v: u32| if on { v } else { 0 };
        self.regs.w32(regs::PCI_HIMR0 as usize, m(regs::IMR0));
        self.regs.w32(regs::PCI_HIMR1 as usize, m(regs::IMR1));
        self.regs.w32(regs::PCI_HIMR3 as usize, m(regs::IMR3));
    }

    /// Ajustes de la PCIe de la 8821CE (`rtw_pci_phy_cfg`): un parámetro de la PHY de Gen1 por
    /// MDIO.
    fn phy_cfg(&mut self) {
        self.regs.w16(0x03f4, 0x6380);
        self.regs.w8(0x03f8, 0x09);
        self.regs.w8(0x03f8 + 3, 0);
        let v = self.regs.r32(0x03f8);
        self.regs.w32(0x03f8, v | (1 << 5));
        for _ in 0..20 {
            if self.regs.r32(0x03f8) & (1 << 5) == 0 {
                break;
            }
            self.delay_us(10);
        }
    }
}

impl Bus for PciBus {
    fn read8(&mut self, a: u32) -> u8 {
        self.regs.r8(a as usize)
    }
    fn read16(&mut self, a: u32) -> u16 {
        self.regs.r16(a as usize)
    }
    fn read32(&mut self, a: u32) -> u32 {
        self.regs.r32(a as usize)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.regs.w8(a as usize, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.regs.w16(a as usize, v)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.regs.w32(a as usize, v)
    }

    fn delay_us(&mut self, us: u32) {
        let end = time::nanos() + u64::from(us) * 1000;
        while time::nanos() < end {
            core::hint::spin_loop();
        }
    }

    fn tx(&mut self, q: Queue, packet: &[u8]) -> bool {
        let regs = self.regs;
        let ring = match q {
            Queue::Be => &mut self.be,
            Queue::Mgmt => &mut self.mgmt,
            Queue::H2c => &mut self.h2c,
            Queue::Bcn => &mut self.bcn,
        };
        if packet.len() > ring.buf_size {
            return false;
        }
        let beacon = q == Queue::Bcn;
        let i = if beacon { 0 } else { ring.wp };
        if !beacon {
            // Lugar libre: entre lo que la placa ya leyó (rp) y lo que escribimos (wp), dejando
            // uno vacío para distinguir "lleno" de "vacío".
            let rp = (regs.r16(ring.idx as usize + 2) & 0xfff) as usize;
            let free = if rp > ring.wp {
                rp - ring.wp - 1
            } else {
                ring.len - ring.wp + rp - 1
            };
            if free == 0 {
                return false;
            }
        }
        // SAFETY: el buffer `i` mide `buf_size` ≥ `packet.len()` y la placa no lo está usando
        // (hay lugar libre en el anillo, o es la página reservada, que se manda de a una).
        let buf = unsafe { ring.bufs.add(i * ring.buf_size) };
        // SAFETY: ídem.
        unsafe { core::ptr::copy_nonoverlapping(packet.as_ptr(), buf, packet.len()) };
        let bd = desc::tx_bd(dma::phys(buf), packet.len(), beacon);
        // SAFETY: el descriptor `i` mide 16 bytes dentro del anillo.
        unsafe { core::ptr::write_volatile(ring.bd.add(i * 16) as *mut [u8; 16], bd) };
        fence(Ordering::SeqCst);
        if !beacon {
            ring.wp = (ring.wp + 1) % ring.len;
            regs.w16(ring.idx as usize, ring.wp as u16);
        }
        true
    }

    fn hci_setup(&mut self) {
        // `rtw_pci_reset_buf_desc` + `rtw_pci_dma_reset`.
        let v = self.regs.r8(regs::PCI_CTRL as usize + 3);
        self.regs.w8(regs::PCI_CTRL as usize + 3, v | 0xf7);
        self.regs.w32(
            regs::PCI_TXBD_DESA_BCNQ as usize,
            dma::phys(self.bcn.bd) as u32,
        );
        let regs = self.regs;
        for ring in [&mut self.h2c, &mut self.be, &mut self.mgmt] {
            ring.wp = 0;
            regs.w16(ring.num as usize, ring.len as u16);
            regs.w32(ring.desa as usize, dma::phys(ring.bd) as u32);
        }
        self.rx.rp = 0;
        self.regs
            .w16(regs::PCI_RXBD_NUM_MPDUQ as usize, RX_RING as u16);
        self.regs.w32(
            regs::PCI_RXBD_DESA_MPDUQ as usize,
            dma::phys(self.rx.bd) as u32,
        );
        self.regs
            .w32(regs::PCI_TXBD_RWPTR_CLR as usize, 0xffff_ffff);
        let v = self.regs.r32(regs::PCI_TXBD_H2CQ_CSR as usize);
        self.regs.w32(
            regs::PCI_TXBD_H2CQ_CSR as usize,
            v | regs::CLR_H2CQ_HOST_IDX | regs::CLR_H2CQ_HW_IDX,
        );
        let v = self.regs.r32(regs::PCI_CTRL as usize);
        self.regs.w32(
            regs::PCI_CTRL as usize,
            v | regs::RST_TRXDMA_INTF | regs::RX_TAG_EN,
        );
    }
}

/// Busca la placa y prepara su bus (sin encenderla todavía).
pub fn probe() -> Option<(PciBus, pci::Device)> {
    let dev = pci::find(REALTEK, RTL8821CE)?;
    let bar = dev.bar_address(2)?;
    dev.enable_memory_and_dma();
    let regs = Mmio::map(bar, 0x10000)?;
    let Some(mut bus) = PciBus::new(regs) else {
        serial_println!("RTL8821CE: no hay memoria de DMA de 32 bits");
        return None;
    };
    bus.phy_cfg();
    // Sin tiempo máximo para las respuestas de la PCIe (`PCI_EXP_DEVCTL2_COMP_TMOUT_DIS`):
    // Linux lo apaga en esta placa.
    if let Some(&(_, cap)) = dev.capabilities().iter().find(|(id, _)| *id == 0x10) {
        let v = dev.read(cap + 0x28);
        dev.write(cap + 0x28, v | (1 << 4));
    }
    Some((bus, dev))
}

/// Pide la interrupción MSI de la placa (avisa a la tarea de la red).
pub fn enable_msi(dev: pci::Device) -> bool {
    interrupts::enable_msi(dev, task::EV_NET, false)
}
