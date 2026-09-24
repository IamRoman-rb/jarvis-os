//! Caché de sectores entre el sistema de archivos y el disco.
//!
//! Abrir una carpeta relee siempre los mismos sectores (la FAT, la carpeta raíz, las carpetas
//! del camino). Con un disco real eso es lo más lento que hay; la caché guarda los últimos
//! sectores leídos en RAM y responde sin ir al disco.
//!
//! Es **write-through**: cada escritura va al disco en el momento (y se actualiza la copia en
//! la caché). Así un corte de luz nunca deja el disco con menos de lo que el sistema cree que
//! escribió. El reemplazo es LRU aproximado: se descarta el sector usado hace más tiempo.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::device::{BlockDevice, IoError, SECTOR_SIZE};

struct Slot {
    lba: u64,
    /// Momento del último uso (un contador que crece).
    used: u64,
    data: Box<[u8; SECTOR_SIZE]>,
}

pub struct BlockCache<D: BlockDevice> {
    dev: D,
    slots: Vec<Slot>,
    capacity: usize,
    clock: u64,
    pub hits: u64,
    pub misses: u64,
}

impl<D: BlockDevice> BlockCache<D> {
    /// `capacity`: cuántos sectores guarda (512 bytes cada uno).
    pub fn new(dev: D, capacity: usize) -> Self {
        BlockCache {
            dev,
            slots: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn device(&self) -> &D {
        &self.dev
    }

    pub fn into_inner(self) -> D {
        self.dev
    }

    fn find(&mut self, lba: u64) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.lba == lba)
    }

    fn store(&mut self, lba: u64, data: &[u8]) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(slot) = self.find(lba) {
            slot.data.copy_from_slice(data);
            slot.used = clock;
            return;
        }
        if self.slots.len() < self.capacity {
            let mut buf = Box::new([0u8; SECTOR_SIZE]);
            buf.copy_from_slice(data);
            self.slots.push(Slot {
                lba,
                used: clock,
                data: buf,
            });
        } else if let Some(oldest) = self.slots.iter_mut().min_by_key(|s| s.used) {
            oldest.lba = lba;
            oldest.used = clock;
            oldest.data.copy_from_slice(data);
        }
    }
}

impl<D: BlockDevice> BlockDevice for BlockCache<D> {
    fn sector_count(&self) -> u64 {
        self.dev.sector_count()
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let sectors = buf.len() / SECTOR_SIZE;
        // ¿Están todos en la caché?
        let mut all = true;
        for i in 0..sectors {
            self.clock += 1;
            let clock = self.clock;
            match self.find(lba + i as u64) {
                Some(slot) => {
                    slot.used = clock;
                    buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE].copy_from_slice(&slot.data[..]);
                }
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            self.hits += 1;
            return Ok(());
        }
        self.misses += 1;
        self.dev.read(lba, buf)?;
        for i in 0..sectors {
            self.store(lba + i as u64, &buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        self.dev.write(lba, buf)?;
        for i in 0..buf.len() / SECTOR_SIZE {
            self.store(lba + i as u64, &buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemDisk;
    use alloc::vec;

    #[test]
    fn lee_de_la_cache_y_escribe_en_el_disco() {
        let mut data = vec![0u8; 8 * SECTOR_SIZE];
        data[SECTOR_SIZE] = 7;
        let mut c = BlockCache::new(MemDisk::new(data), 2);
        let mut buf = [0u8; SECTOR_SIZE];
        c.read(1, &mut buf).unwrap();
        c.read(1, &mut buf).unwrap();
        assert_eq!((buf[0], c.hits, c.misses), (7, 1, 1));

        buf[0] = 9;
        c.write(1, &buf).unwrap();
        let mut again = [0u8; SECTOR_SIZE];
        c.read(1, &mut again).unwrap();
        assert_eq!(again[0], 9, "la caché tiene lo último escrito");

        // Con capacidad 2, leer otros dos sectores desaloja el 1; el disco igual lo tiene.
        c.read(2, &mut again).unwrap();
        c.read(3, &mut again).unwrap();
        c.read(1, &mut again).unwrap();
        assert_eq!(again[0], 9);
        assert_eq!(c.into_inner().into_inner()[SECTOR_SIZE], 9, "write-through");
    }

    #[test]
    fn lecturas_de_varios_sectores() {
        let data: alloc::vec::Vec<u8> = (0..4 * SECTOR_SIZE)
            .map(|i| (i / SECTOR_SIZE) as u8)
            .collect();
        let mut c = BlockCache::new(MemDisk::new(data), 8);
        let mut buf = vec![0u8; 3 * SECTOR_SIZE];
        c.read(1, &mut buf).unwrap();
        c.read(1, &mut buf).unwrap();
        assert_eq!((buf[0], buf[SECTOR_SIZE], buf[2 * SECTOR_SIZE]), (1, 2, 3));
        assert_eq!((c.hits, c.misses), (1, 1));
    }
}
