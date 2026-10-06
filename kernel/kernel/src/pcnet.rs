//! Driver de las placas de red AMD PCnet (Am79C970A "PCnet-PCI II" y Am79C973 "PCnet-FAST III").
//!
//! Hardware: 1022:2000, la placa que VirtualBox pone por defecto en las máquinas de tipo
//! "Other" (y la que emula QEMU con `-device pcnet`). Sin interrupciones: la tarea de la red la
//! revisa cada tanto, como a la RTL8139.
//!
//! Cómo se arranca: después del reinicio la placa está en modo de 16 bits (WIO); una escritura de
//! 32 bits en RDP la pasa a 32 (DWIO). Los registros se leen de a uno por un puerto índice (RAP)
//! y uno de datos (RDP para los CSR, BDP para los BCR). Con SWSTYLE 2 se le da un "bloque de
//! inicialización" en memoria (MAC y dónde están los anillos), se pone INIT en CSR0, se espera
//! IDON y se arranca con STRT. Los formatos están en `jarvis_drivers::nic`.
//! Referencias: AMD Am79C973/Am79C975 datasheet (§"Software Access", "Initialization Block",
//! "Descriptor Rings"), <https://wiki.osdev.org/AMD_PCNET>, driver `pcnet32` de Linux.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use jarvis_drivers::nic::{
    pcnet_init_block, pcnet_rx_desc, pcnet_rx_done, pcnet_tx_desc, pcnet_tx_free,
};
use x86_64::instructions::port::Port;

use crate::nic::Frames;
use crate::{dma, pci, serial_println, time};

// Registros en modo DWIO (desde el BAR0 de E/S).
const APROM: u16 = 0x00;
const RDP: u16 = 0x10;
const RAP: u16 = 0x14;
const RESET: u16 = 0x18;
const BDP: u16 = 0x1C;
/// En modo de 16 bits (justo después de encender), el reinicio está acá.
const RESET16: u16 = 0x14;

const CSR0_INIT: u32 = 1 << 0;
const CSR0_STRT: u32 = 1 << 1;
const CSR0_TDMD: u32 = 1 << 3;
const CSR0_IDON: u32 = 1 << 8;
/// CSR4: rellenar solo las tramas cortas hasta 64 bytes (APAD_XMT).
const CSR4_APAD_XMT: u32 = 1 << 11;
/// BCR20: SWSTYLE 2 (descriptores de 32 bits) con SSIZE32.
const BCR20_STYLE2: u32 = 0x0102;

const RX_LOG2: u8 = 5;
const TX_LOG2: u8 = 3;
const RX_COUNT: usize = 1 << RX_LOG2;
const TX_COUNT: usize = 1 << TX_LOG2;
const BUF: usize = 1536;
const DESC: usize = 16;

pub struct Pcnet {
    io: u16,
    mac: [u8; 6],
    rx_ring: *mut u8,
    tx_ring: *mut u8,
    rx_bufs: *mut u8,
    tx_bufs: *mut u8,
    rx_next: usize,
    tx_next: usize,
}

// SAFETY: los punteros son memoria DMA propia de la placa; la usa solo la tarea de la red.
unsafe impl Send for Pcnet {}

pub fn probe() -> Option<Pcnet> {
    let dev = pci::find(0x1022, 0x2000)?;
    let bar = dev.bar(0);
    if bar & 1 == 0 {
        return None;
    }
    dev.enable_io_and_dma();
    Pcnet::start(dev, (bar & 0xFFFC) as u16)
}

impl Pcnet {
    fn in32(&self, reg: u16) -> u32 {
        // SAFETY: `io + reg` es un registro de esta placa (BAR0, puertos de E/S).
        unsafe { Port::<u32>::new(self.io + reg).read() }
    }
    fn out32(&self, reg: u16, v: u32) {
        // SAFETY: ídem `in32`.
        unsafe { Port::<u32>::new(self.io + reg).write(v) }
    }
    fn csr(&self, n: u32) -> u32 {
        self.out32(RAP, n);
        self.in32(RDP)
    }
    fn set_csr(&self, n: u32, v: u32) {
        self.out32(RAP, n);
        self.out32(RDP, v);
    }
    fn set_bcr(&self, n: u32, v: u32) {
        self.out32(RAP, n);
        self.out32(BDP, v);
    }

    fn start(dev: pci::Device, io: u16) -> Option<Pcnet> {
        // La placa solo maneja direcciones de 32 bits.
        let (Some(init), Some(rx_ring), Some(tx_ring), Some(rx_bufs), Some(tx_bufs)) = (
            dma::alloc32(28, 16),
            dma::alloc32(RX_COUNT * DESC, 16),
            dma::alloc32(TX_COUNT * DESC, 16),
            dma::alloc32(RX_COUNT * BUF, 16),
            dma::alloc32(TX_COUNT * BUF, 16),
        ) else {
            serial_println!("PCnet: no hay memoria de DMA de 32 bits");
            return None;
        };
        let mut n = Pcnet {
            io,
            mac: [0; 6],
            rx_ring,
            tx_ring,
            rx_bufs,
            tx_bufs,
            rx_next: 0,
            tx_next: 0,
        };
        // Reinicio: leer el registro de reinicio (en los dos modos, por las dudas) y pasar a 32
        // bits escribiendo 0 en RDP.
        // SAFETY: registros de reinicio de esta placa; leerlos la reinicia.
        unsafe {
            Port::<u32>::new(io + RESET).read();
            Port::<u16>::new(io + RESET16).read();
        }
        let t = time::millis();
        while time::millis() - t < 2 {}
        n.out32(RDP, 0);
        for (i, b) in n.mac.iter_mut().enumerate() {
            // SAFETY: la PROM con la MAC (bytes 0–5 del BAR0).
            *b = unsafe { Port::<u8>::new(io + APROM + i as u16).read() };
        }
        n.set_bcr(20, BCR20_STYLE2);

        // Los anillos: la recepción, de la placa; la transmisión, nuestra (OWN en 0).
        for i in 0..RX_COUNT {
            let buf = dma::phys(rx_bufs) as u32 + (i * BUF) as u32;
            n.write_desc(rx_ring, i, &pcnet_rx_desc(buf, BUF));
        }
        // SAFETY: el anillo de transmisión mide TX_COUNT descriptores.
        unsafe { core::ptr::write_bytes(tx_ring, 0, TX_COUNT * DESC) };
        let block = pcnet_init_block(
            n.mac,
            RX_LOG2,
            TX_LOG2,
            dma::phys(rx_ring) as u32,
            dma::phys(tx_ring) as u32,
        );
        // SAFETY: `init` mide 28 bytes.
        unsafe { core::ptr::copy_nonoverlapping(block.as_ptr(), init, block.len()) };
        fence(Ordering::SeqCst);
        let addr = dma::phys(init) as u32;
        n.set_csr(1, addr & 0xFFFF);
        n.set_csr(2, addr >> 16);
        n.set_csr(4, n.csr(4) | CSR4_APAD_XMT);
        n.set_csr(0, CSR0_INIT);
        let t = time::millis();
        while n.csr(0) & CSR0_IDON == 0 {
            if time::millis() - t > 200 {
                serial_println!("PCnet: no terminó de inicializarse");
                return None;
            }
        }
        // Arrancar (sin IENA: sin interrupciones) y bajar IDON.
        n.set_csr(0, CSR0_STRT | CSR0_IDON);
        let m = n.mac;
        serial_println!(
            "PCnet {:02x}:{:02x}.{}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dev.bus,
            dev.slot,
            dev.function,
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5]
        );
        Some(n)
    }

    fn write_desc(&self, ring: *mut u8, i: usize, d: &[u8; 16]) {
        // Primero los primeros 6 bytes y después el estado (con OWN), así la placa nunca ve un
        // descriptor a medio escribir.
        // SAFETY: `ring` tiene al menos `i + 1` descriptores de 16 bytes.
        unsafe {
            let p = ring.add(i * DESC);
            core::ptr::copy_nonoverlapping(d.as_ptr().add(8), p.add(8), 8);
            core::ptr::copy_nonoverlapping(d.as_ptr(), p, 6);
            fence(Ordering::SeqCst);
            core::ptr::copy_nonoverlapping(d.as_ptr().add(6), p.add(6), 2);
        }
        fence(Ordering::SeqCst);
    }

    fn desc(&self, ring: *mut u8, i: usize) -> [u8; 16] {
        let mut d = [0u8; 16];
        fence(Ordering::SeqCst);
        for (k, b) in d.iter_mut().enumerate() {
            // SAFETY: ídem `write_desc`; la placa lo escribe por DMA (por eso volatile).
            *b = unsafe { ring.add(i * DESC + k).read_volatile() };
        }
        d
    }
}

impl Frames for Pcnet {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        loop {
            let i = self.rx_next;
            let done = pcnet_rx_done(&self.desc(self.rx_ring, i))?;
            // SAFETY: el buffer `i` mide BUF bytes y la placa ya lo devolvió.
            let frame = done.map(|len| unsafe {
                core::slice::from_raw_parts(self.rx_bufs.add(i * BUF), len.min(BUF)).to_vec()
            });
            let buf = dma::phys(self.rx_bufs) as u32 + (i * BUF) as u32;
            self.write_desc(self.rx_ring, i, &pcnet_rx_desc(buf, BUF));
            self.rx_next = (i + 1) % RX_COUNT;
            if frame.is_some() {
                return frame;
            }
        }
    }

    fn can_send(&mut self) -> bool {
        pcnet_tx_free(&self.desc(self.tx_ring, self.tx_next))
    }

    fn send(&mut self, frame: &[u8]) {
        let i = self.tx_next;
        let len = frame.len().clamp(60, BUF);
        // SAFETY: la ranura `i` mide BUF bytes y está libre (`can_send`).
        unsafe {
            let dst = self.tx_bufs.add(i * BUF);
            core::ptr::write_bytes(dst, 0, len);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), dst, frame.len().min(BUF));
        }
        let buf = dma::phys(self.tx_bufs) as u32 + (i * BUF) as u32;
        self.write_desc(self.tx_ring, i, &pcnet_tx_desc(buf, len));
        // "Mirá la cola ya" (sin esperar el sondeo de la placa).
        self.set_csr(0, CSR0_TDMD);
        self.tx_next = (i + 1) % TX_COUNT;
    }
}
