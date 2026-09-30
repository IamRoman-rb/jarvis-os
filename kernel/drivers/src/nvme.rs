//! NVMe: los SSD que se enchufan directo a PCI Express (M.2).
//!
//! NVMe se diseñó para memoria flash: en vez de una lista de 32 comandos, **colas** en memoria
//! compartida. Cada cola de envío (SQ) tiene su cola de terminación (CQ). El driver escribe
//! comandos de 64 bytes en la SQ y avisa escribiendo el nuevo final en un "timbre" (doorbell).
//! El disco escribe resultados de 16 bytes en la CQ, marcando cada uno con un **bit de fase**
//! que se invierte en cada vuelta de la cola. Así el driver sabe cuáles son nuevos sin que el
//! disco tenga que avisar cuántos hay.
//!
//! Hay una cola de **administración** (la 0: crear colas, IDENTIFY) y las de E/S (leer,
//! escribir). JARVIS-OS usa una de cada una.
//! Referencias: especificación NVM Express 1.4 (base) y NVM Command Set, y
//! <https://wiki.osdev.org/NVMe>.

use alloc::string::String;

use crate::{ascii, le};

// Comandos de administración.
pub const ADMIN_CREATE_SQ: u8 = 0x01;
pub const ADMIN_CREATE_CQ: u8 = 0x05;
pub const ADMIN_IDENTIFY: u8 = 0x06;
// Comandos de E/S.
pub const IO_FLUSH: u8 = 0x00;
pub const IO_WRITE: u8 = 0x01;
pub const IO_READ: u8 = 0x02;

/// Un comando (entrada de la SQ, 64 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub opcode: u8,
    pub id: u16,
    pub nsid: u32,
    /// PRP1 y PRP2: las páginas de datos (la segunda solo si el pedido cruza una página).
    pub prp: [u64; 2],
    /// Palabras 10–15, que dependen del comando.
    pub cdw: [u32; 6],
}

impl Command {
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut b = [0u8; 64];
        b[0] = self.opcode;
        b[2..4].copy_from_slice(&self.id.to_le_bytes());
        b[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        b[24..32].copy_from_slice(&self.prp[0].to_le_bytes());
        b[32..40].copy_from_slice(&self.prp[1].to_le_bytes());
        for (i, w) in self.cdw.iter().enumerate() {
            b[40 + i * 4..44 + i * 4].copy_from_slice(&w.to_le_bytes());
        }
        b
    }

    /// IDENTIFY: `cns` 1 = el controlador, 0 = el namespace `nsid`. Devuelve 4 KiB en `page`.
    pub fn identify(id: u16, cns: u32, nsid: u32, page: u64) -> Command {
        Command {
            opcode: ADMIN_IDENTIFY,
            id,
            nsid,
            prp: [page, 0],
            cdw: [cns, 0, 0, 0, 0, 0],
        }
    }

    /// Crea la cola de terminación `qid` de `size` entradas en `phys` (contigua, con
    /// interrupción en el vector 0).
    pub fn create_cq(id: u16, qid: u16, size: u16, phys: u64) -> Command {
        Command {
            opcode: ADMIN_CREATE_CQ,
            id,
            nsid: 0,
            prp: [phys, 0],
            // CDW10: tamaño − 1 | id. CDW11: bit 0 contigua, bit 1 interrupciones, vector 0.
            cdw: [((size as u32 - 1) << 16) | qid as u32, 0b11, 0, 0, 0, 0],
        }
    }

    /// Crea la cola de envío `qid` asociada a la de terminación `cqid`.
    pub fn create_sq(id: u16, qid: u16, size: u16, phys: u64, cqid: u16) -> Command {
        Command {
            opcode: ADMIN_CREATE_SQ,
            id,
            nsid: 0,
            prp: [phys, 0],
            // CDW11: la CQ asociada | bit 0 contigua.
            cdw: [
                ((size as u32 - 1) << 16) | qid as u32,
                (cqid as u32) << 16 | 1,
                0,
                0,
                0,
                0,
            ],
        }
    }

    /// Leer o escribir `count` bloques desde `lba`. `prp`: las páginas de los datos.
    pub fn rw(opcode: u8, id: u16, nsid: u32, lba: u64, count: u16, prp: [u64; 2]) -> Command {
        Command {
            opcode,
            id,
            nsid,
            prp,
            cdw: [lba as u32, (lba >> 32) as u32, (count - 1) as u32, 0, 0, 0],
        }
    }
}

/// Una entrada de la CQ (16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub sq_head: u16,
    pub id: u16,
    pub phase: bool,
    /// Código de estado (0 = bien): tipo (bits 9–11) y código (1–8) del campo de estado.
    pub status: u16,
}

pub fn parse_completion(b: &[u8]) -> Completion {
    let s = le(b, 14, 2) as u16;
    Completion {
        sq_head: le(b, 8, 2) as u16,
        id: le(b, 12, 2) as u16,
        phase: s & 1 != 0,
        status: (s >> 1) & 0x7FF,
    }
}

/// Registros del controlador: `CAP` dice el paso entre timbres (DSTRD) y el máximo de
/// entradas por cola (MQES).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub max_queue_entries: u32,
    pub doorbell_stride: u32,
    /// Cuánto esperar a que el controlador esté listo, en ms.
    pub timeout_ms: u32,
    /// Tamaño de página mínimo que acepta (en bytes).
    pub min_page: u32,
}

pub fn parse_cap(cap: u64) -> Capabilities {
    Capabilities {
        max_queue_entries: (cap & 0xFFFF) as u32 + 1,
        doorbell_stride: 4 << ((cap >> 32) & 0xF),
        timeout_ms: ((cap >> 24) & 0xFF) as u32 * 500,
        min_page: 1 << (12 + ((cap >> 48) & 0xF)),
    }
}

/// Desplazamiento del timbre de la cola `qid` (`completion` = el de su CQ).
pub fn doorbell(qid: u16, completion: bool, stride: u32) -> usize {
    0x1000 + (2 * qid as usize + completion as usize) * stride as usize
}

/// Lo que dice IDENTIFY del controlador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub serial: String,
    pub model: String,
    /// Máximo de datos por pedido, en páginas mínimas como potencia de 2 (0 = sin límite).
    pub mdts: u8,
    pub namespaces: u32,
}

pub fn parse_controller(d: &[u8]) -> Option<Controller> {
    (d.len() >= 4096).then(|| Controller {
        serial: ascii(&d[4..24]),
        model: ascii(&d[24..64]),
        mdts: d[77],
        namespaces: le(d, 516, 4) as u32,
    })
}

/// Lo que dice IDENTIFY de un namespace (un "disco" dentro del SSD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Namespace {
    pub blocks: u64,
    pub block_size: u32,
}

pub fn parse_namespace(d: &[u8]) -> Option<Namespace> {
    if d.len() < 4096 {
        return None;
    }
    let blocks = le(d, 0, 8);
    // FLBAS (byte 26), bits 0–3: qué formato de la tabla LBAF (desde el byte 128) está en uso.
    let format = (d[26] & 0xF) as usize;
    let lbads = d[128 + format * 4 + 2];
    (blocks != 0 && (9..=16).contains(&lbads)).then_some(Namespace {
        blocks,
        block_size: 1 << lbads,
    })
}
