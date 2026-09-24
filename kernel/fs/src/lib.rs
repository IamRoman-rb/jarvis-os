//! FAT32 propio de JARVIS-OS.
//!
//! FAT32 es el sistema de archivos de los pendrives: simple, documentado y legible desde
//! cualquier sistema (el disco de JARVIS-OS se puede abrir desde Windows). Estructura en disco:
//!
//! ```text
//! | sector de arranque (BPB) | reservados | FAT 1 | FAT 2 | datos: clusters 2, 3, 4, … |
//! ```
//!
//! - Los datos se guardan en **clusters** (bloques de N sectores).
//! - La **FAT** es una tabla con una entrada por cluster: dice cuál es el cluster siguiente del
//!   mismo archivo (una lista enlazada), o si está libre, o si es el último. Hay dos copias.
//! - Una **carpeta** es un archivo más, lleno de entradas de 32 bytes: nombre corto 8.3,
//!   atributos, fechas, primer cluster y tamaño. Los nombres largos (con minúsculas, espacios y
//!   acentos) se guardan en entradas extra "LFN" antes de la entrada corta.
//!
//! Referencia: especificación "Microsoft FAT32 File System" y <https://wiki.osdev.org/FAT>.
//!
//! La crate es `no_std` (la usa el kernel) y funciona sobre cualquier [`BlockDevice`]: en el
//! kernel es el driver virtio-blk y en los tests un disco en memoria ([`MemDisk`]).

#![no_std]

extern crate alloc;

mod device;
mod dirent;
mod error;
mod fat32;
mod time;

pub use device::{BlockDevice, IoError, MemDisk, SECTOR_SIZE};
pub use error::{FsError, Result};
pub use fat32::{DirEntry, FileSystem};
pub use time::Timestamp;
