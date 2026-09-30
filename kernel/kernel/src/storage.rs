//! Los discos de la máquina (K13): se buscan todos al arrancar y se elige el del sistema.
//!
//! El disco del sistema es el que tiene una partición GPT de tipo `JARVIS_DATA` (el que dejó el
//! instalador). El FAT32 va adentro de esa partición, que se ve como un disco aparte
//! (`PartitionDevice`): lo que escriba el sistema de archivos no puede salirse de ella. Los
//! discos sin esa partición (el SSD con Windows, un pendrive) **no se montan** (ADR 0011); quedan
//! guardados acá para el instalador.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_drivers::gpt::{self, JARVIS_DATA, PartitionDevice};
use jarvis_fs::BlockDevice;
use spin::Mutex;

use crate::{ahci, nvme, serial_println, xhci};

pub type AnyDisk = Box<dyn BlockDevice + Send>;

/// Un disco encontrado.
pub struct Found {
    /// Para mostrar: "SATA 0: WD Green 2.5 480GB".
    pub name: String,
    pub disk: AnyDisk,
}

/// Los discos que no son el del sistema.
static SPARE: Mutex<Vec<Found>> = Mutex::new(Vec::new());

/// Busca los discos SATA, NVMe y USB (esto último arranca también el teclado y el mouse USB).
pub fn probe() -> Vec<Found> {
    let mut found = Vec::new();
    for d in ahci::probe() {
        found.push(Found {
            name: d.name(),
            disk: Box::new(d),
        });
    }
    for d in nvme::probe() {
        found.push(Found {
            name: d.name(),
            disk: Box::new(d),
        });
    }
    found.extend(xhci::init());
    found
}

/// Saca de la lista el primer disco con partición de JARVIS y devuelve esa partición.
pub fn take_system(found: &mut Vec<Found>) -> Option<(String, PartitionDevice<AnyDisk>)> {
    for i in 0..found.len() {
        let Some(parts) = gpt::read(&mut *found[i].disk) else {
            serial_println!("DISCO {}: sin GPT", found[i].name);
            continue;
        };
        for p in &parts {
            serial_println!(
                "DISCO {}: partición \"{}\" ({} MiB)",
                found[i].name,
                p.name,
                p.sectors() / 2048
            );
        }
        if let Some(p) = parts.iter().find(|p| p.kind == JARVIS_DATA) {
            let (first, sectors) = (p.first, p.sectors());
            let f = found.remove(i);
            return Some((f.name, PartitionDevice::new(f.disk, first, sectors)));
        }
    }
    None
}

/// Guarda los discos que no se usaron (para el instalador).
pub fn park(found: Vec<Found>) {
    SPARE.lock().extend(found);
}
