//! El allocator de marcos físicos: un bit por marco de 4 KiB (1 = ocupado).
//!
//! Empieza con todo ocupado y se liberan solo las regiones que el firmware dice que son RAM
//! usable: así, lo que nadie declaró (agujeros, registros de dispositivos, lo del firmware) nunca
//! se entrega. Con 4 GiB de RAM el mapa ocupa 128 KiB. `next` recuerda dónde se encontró el
//! último libre, para no recorrer desde el principio cada vez.

use alloc::vec;
use alloc::vec::Vec;

use crate::PAGE;

pub struct FrameAllocator {
    /// Un bit por marco; el marco `i` empieza en `i * PAGE`.
    bits: Vec<u64>,
    frames: usize,
    free: usize,
    next: usize,
}

impl FrameAllocator {
    /// Para una RAM que llega hasta `max_addr` (sin incluir), toda ocupada.
    pub fn new(max_addr: u64) -> FrameAllocator {
        let frames = max_addr.div_ceil(PAGE) as usize;
        FrameAllocator {
            bits: vec![u64::MAX; frames.div_ceil(64)],
            frames,
            free: 0,
            next: 0,
        }
    }

    fn used(&self, i: usize) -> bool {
        self.bits[i / 64] & (1 << (i % 64)) != 0
    }

    fn set(&mut self, i: usize, used: bool) {
        let was = self.used(i);
        if used {
            self.bits[i / 64] |= 1 << (i % 64);
        } else {
            self.bits[i / 64] &= !(1 << (i % 64));
        }
        match (was, used) {
            (true, false) => self.free += 1,
            (false, true) => self.free -= 1,
            _ => {}
        }
    }

    /// Los marcos que caen enteros en `[start, end)` (una región que no empieza alineada pierde
    /// el pedazo del principio: no se puede entregar medio marco).
    fn whole(&self, start: u64, end: u64) -> core::ops::Range<usize> {
        let first = start.div_ceil(PAGE) as usize;
        let last = ((end / PAGE) as usize).min(self.frames);
        first.min(last)..last
    }

    /// Marca como libre la RAM de `[start, end)`.
    pub fn add_free(&mut self, start: u64, end: u64) {
        for i in self.whole(start, end) {
            self.set(i, false);
        }
    }

    /// Marca como ocupado todo marco que toque `[start, end)` (aunque sea en parte).
    pub fn reserve(&mut self, start: u64, end: u64) {
        let first = (start / PAGE) as usize;
        let last = (end.div_ceil(PAGE) as usize).min(self.frames);
        for i in first.min(last)..last {
            self.set(i, true);
        }
    }

    /// Un marco libre (su dirección física), ya marcado como ocupado.
    pub fn alloc(&mut self) -> Option<u64> {
        let words = self.bits.len();
        for k in 0..words {
            let w = (self.next / 64 + k) % words;
            if self.bits[w] != u64::MAX {
                let i = w * 64 + (!self.bits[w]).trailing_zeros() as usize;
                if i >= self.frames {
                    continue;
                }
                self.set(i, true);
                self.next = i;
                return Some(i as u64 * PAGE);
            }
        }
        None
    }

    /// `n` marcos seguidos (para un dispositivo que necesita memoria física contigua).
    pub fn alloc_contiguous(&mut self, n: usize) -> Option<u64> {
        if n == 0 {
            return None;
        }
        let mut run = 0;
        for i in 0..self.frames {
            if self.used(i) {
                run = 0;
                continue;
            }
            run += 1;
            if run == n {
                let first = i + 1 - n;
                for j in first..=i {
                    self.set(j, true);
                }
                return Some(first as u64 * PAGE);
            }
        }
        None
    }

    /// Devuelve un marco. Liberar uno que ya estaba libre es un bug del llamador: se ignora.
    pub fn free(&mut self, addr: u64) {
        let i = (addr / PAGE) as usize;
        if i < self.frames && addr.is_multiple_of(PAGE) {
            self.set(i, false);
            self.next = self.next.min(i);
        }
    }

    pub fn free_frames(&self) -> usize {
        self.free
    }

    pub fn is_free(&self, addr: u64) -> bool {
        let i = (addr / PAGE) as usize;
        i < self.frames && !self.used(i)
    }
}
