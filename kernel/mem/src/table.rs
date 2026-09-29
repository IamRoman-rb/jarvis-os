//! Tablas de páginas de x86_64: 4 niveles de 512 entradas de 8 bytes.
//!
//! Una dirección virtual de 48 bits se parte en 4 índices de 9 bits (uno por nivel: PML4, PDPT,
//! PD, PT) y 12 bits de desplazamiento dentro de la página. Cada entrada tiene la dirección física
//! de la tabla (o del marco) siguiente y unos bits de permisos. Una entrada del nivel 3 o 2 con el
//! bit `HUGE` ya es la hoja: mapea 1 GiB o 2 MiB de una vez (el mapeo de toda la RAM las usa, así
//! gasta pocas tablas).
//!
//! Las tablas están en memoria física; el código las lee y escribe por [`PhysMem`], que en el
//! kernel es "física + offset" y en los tests un `Vec`.

use crate::PAGE;
use crate::frames::FrameAllocator;

/// Bits de una entrada (Intel SDM vol. 3A, tabla 4-19).
pub mod flags {
    pub const PRESENT: u64 = 1;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2;
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const NO_CACHE: u64 = 1 << 4;
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY: u64 = 1 << 6;
    /// En los niveles 3 y 2: la entrada es la hoja (1 GiB o 2 MiB).
    pub const HUGE: u64 = 1 << 7;
    pub const GLOBAL: u64 = 1 << 8;
    /// No ejecutar (necesita EFER.NXE).
    pub const NO_EXECUTE: u64 = 1 << 63;
}

/// Dónde va la dirección física en una entrada (bits 12..52).
pub const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// 4 KiB (una entrada del nivel 1).
    Small,
    /// 2 MiB (una entrada del nivel 2 con HUGE).
    Large,
    /// 1 GiB (una entrada del nivel 3 con HUGE).
    Huge,
}

impl Size {
    pub const fn bytes(self) -> u64 {
        match self {
            Size::Small => PAGE,
            Size::Large => 512 * PAGE,
            Size::Huge => 512 * 512 * PAGE,
        }
    }

    /// En qué nivel está su hoja (1 = PT, 2 = PD, 3 = PDPT).
    const fn level(self) -> usize {
        match self {
            Size::Small => 1,
            Size::Large => 2,
            Size::Huge => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// No hay marcos para una tabla nueva.
    NoFrames,
    /// Ya había algo mapeado ahí (o una página grande lo cubre).
    AlreadyMapped,
    /// Dirección no alineada al tamaño de página.
    Unaligned,
}

/// La memoria física, vista de a entradas de 8 bytes.
pub trait PhysMem {
    fn read(&self, phys: u64) -> u64;
    fn write(&mut self, phys: u64, value: u64);
}

/// Los 9 bits del índice del nivel `level` (4 = PML4 … 1 = PT).
pub const fn index(virt: u64, level: usize) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1FF) as usize
}

/// Una dirección de 48 bits llevada a la forma canónica (los bits 48..64 copian el 47).
pub const fn canonical(virt: u64) -> u64 {
    (((virt << 16) as i64) >> 16) as u64
}

/// Una jerarquía de tablas de páginas; `root` es la dirección física de la PML4 (lo que va en
/// CR3).
#[derive(Clone, Copy, Debug)]
pub struct PageTable {
    pub root: u64,
}

/// Una hoja: lo que mapea una página.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub virt: u64,
    pub phys: u64,
    pub size: Size,
    /// Los bits de la entrada, sin la dirección ni `HUGE`.
    pub flags: u64,
}

fn zero(mem: &mut dyn PhysMem, table: u64) {
    for i in 0..512 {
        mem.write(table + i * 8, 0);
    }
}

impl PageTable {
    /// Una PML4 nueva, vacía.
    pub fn new(mem: &mut dyn PhysMem, frames: &mut FrameAllocator) -> Option<PageTable> {
        let root = frames.alloc()?;
        zero(mem, root);
        Some(PageTable { root })
    }

    /// Mapea una página de `size` en `virt` → `phys` con `flags` (sin `PRESENT` ni `HUGE`: los
    /// pone esta función). Crea las tablas intermedias que falten.
    pub fn map(
        &self,
        mem: &mut dyn PhysMem,
        frames: &mut FrameAllocator,
        virt: u64,
        phys: u64,
        size: Size,
        flags: u64,
    ) -> Result<(), MapError> {
        if !virt.is_multiple_of(size.bytes()) || !phys.is_multiple_of(size.bytes()) {
            return Err(MapError::Unaligned);
        }
        let mut table = self.root;
        for level in (size.level() + 1..=4).rev() {
            let slot = table + index(virt, level) as u64 * 8;
            let e = mem.read(slot);
            table = if e & flags::PRESENT == 0 {
                let t = frames.alloc().ok_or(MapError::NoFrames)?;
                zero(mem, t);
                // Las intermedias permiten todo: los permisos los decide la hoja (el procesador
                // combina los de todos los niveles, y gana el más restrictivo).
                let user = flags & flags::USER;
                mem.write(slot, t | flags::PRESENT | flags::WRITABLE | user);
                t
            } else if e & flags::HUGE != 0 {
                return Err(MapError::AlreadyMapped);
            } else {
                e & ADDR_MASK
            };
        }
        let slot = table + index(virt, size.level()) as u64 * 8;
        if mem.read(slot) & flags::PRESENT != 0 {
            return Err(MapError::AlreadyMapped);
        }
        let huge = if size == Size::Small { 0 } else { flags::HUGE };
        mem.write(slot, phys | (flags & !ADDR_MASK) | flags::PRESENT | huge);
        Ok(())
    }

    /// La hoja que mapea `virt`, si hay.
    pub fn leaf(&self, mem: &dyn PhysMem, virt: u64) -> Option<Leaf> {
        let mut table = self.root;
        for level in (1..=4).rev() {
            let e = mem.read(table + index(virt, level) as u64 * 8);
            if e & flags::PRESENT == 0 {
                return None;
            }
            let size = match level {
                3 if e & flags::HUGE != 0 => Some(Size::Huge),
                2 if e & flags::HUGE != 0 => Some(Size::Large),
                1 => Some(Size::Small),
                _ => None,
            };
            if let Some(size) = size {
                let base = virt & !(size.bytes() - 1);
                return Some(Leaf {
                    virt: base,
                    phys: e & ADDR_MASK & !(size.bytes() - 1),
                    size,
                    flags: e & !ADDR_MASK & !flags::HUGE,
                });
            }
            table = e & ADDR_MASK;
        }
        None
    }

    /// La dirección física que corresponde a `virt`.
    pub fn translate(&self, mem: &dyn PhysMem, virt: u64) -> Option<u64> {
        self.leaf(mem, virt)
            .map(|l| l.phys + (virt & (l.size.bytes() - 1)))
    }

    /// Saca el mapeo de la página que contiene `virt` (las tablas intermedias quedan). Devuelve
    /// la hoja que había. Después hay que invalidar la TLB (`invlpg`).
    pub fn unmap(&self, mem: &mut dyn PhysMem, virt: u64) -> Option<Leaf> {
        let leaf = self.leaf(mem, virt)?;
        let mut table = self.root;
        for level in (leaf.size.level() + 1..=4).rev() {
            table = mem.read(table + index(virt, level) as u64 * 8) & ADDR_MASK;
        }
        mem.write(table + index(virt, leaf.size.level()) as u64 * 8, 0);
        Some(leaf)
    }

    /// Todas las hojas, en orden de dirección virtual.
    pub fn walk(&self, mem: &dyn PhysMem, f: &mut dyn FnMut(Leaf)) {
        walk_level(mem, self.root, 4, 0, f);
    }

    /// Cuántas tablas usa la jerarquía (la PML4 incluida).
    pub fn tables(&self, mem: &dyn PhysMem) -> usize {
        count_tables(mem, self.root, 4)
    }
}

fn walk_level(mem: &dyn PhysMem, table: u64, level: usize, base: u64, f: &mut dyn FnMut(Leaf)) {
    for i in 0..512u64 {
        let e = mem.read(table + i * 8);
        if e & flags::PRESENT == 0 {
            continue;
        }
        let virt = canonical(base | (i << (12 + 9 * (level - 1))));
        let size = match level {
            1 => Some(Size::Small),
            2 if e & flags::HUGE != 0 => Some(Size::Large),
            3 if e & flags::HUGE != 0 => Some(Size::Huge),
            _ => None,
        };
        match size {
            Some(size) => f(Leaf {
                virt,
                phys: e & ADDR_MASK & !(size.bytes() - 1),
                size,
                flags: e & !ADDR_MASK & !flags::HUGE,
            }),
            None => walk_level(mem, e & ADDR_MASK, level - 1, virt, f),
        }
    }
}

fn count_tables(mem: &dyn PhysMem, table: u64, level: usize) -> usize {
    let mut n = 1;
    if level > 1 {
        for i in 0..512u64 {
            let e = mem.read(table + i * 8);
            if e & flags::PRESENT != 0 && e & flags::HUGE == 0 {
                n += count_tables(mem, e & ADDR_MASK, level - 1);
            }
        }
    }
    n
}
