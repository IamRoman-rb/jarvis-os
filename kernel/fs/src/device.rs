//! Dispositivo de bloques: lo único que el sistema de archivos necesita del hardware.

use alloc::vec::Vec;

/// Tamaño de sector. FAT32 admite otros, pero todos los discos que usamos tienen 512.
pub const SECTOR_SIZE: usize = 512;

/// Error del dispositivo (el driver ya logueó el detalle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// Un disco que se lee y escribe de a sectores. `buf` siempre mide un múltiplo de 512.
pub trait BlockDevice {
    fn sector_count(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError>;
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError>;
}

impl<D: BlockDevice + ?Sized> BlockDevice for &mut D {
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        (**self).read(lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        (**self).write(lba, buf)
    }
}

/// Un disco elegido al arrancar (virtio, SATA, NVMe, USB…) sin saber de qué tipo es.
impl<D: BlockDevice + ?Sized> BlockDevice for alloc::boxed::Box<D> {
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        (**self).read(lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        (**self).write(lba, buf)
    }
}

/// Disco en memoria: para tests y para probar la app de archivos en el host.
pub struct MemDisk {
    data: Vec<u8>,
}

impl MemDisk {
    pub fn new(data: Vec<u8>) -> Self {
        MemDisk { data }
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    fn range(&self, lba: u64, len: usize) -> Result<core::ops::Range<usize>, IoError> {
        if !len.is_multiple_of(SECTOR_SIZE) {
            return Err(IoError);
        }
        let start = (lba as usize).checked_mul(SECTOR_SIZE).ok_or(IoError)?;
        let end = start.checked_add(len).ok_or(IoError)?;
        if end > self.data.len() {
            return Err(IoError);
        }
        Ok(start..end)
    }
}

impl BlockDevice for MemDisk {
    fn sector_count(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let r = self.range(lba, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let r = self.range(lba, buf.len())?;
        self.data[r].copy_from_slice(buf);
        Ok(())
    }
}
