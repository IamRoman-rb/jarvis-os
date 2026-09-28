//! Los segmentos cargables (`PT_LOAD`) de un ELF64: dónde quedó cada parte del kernel y con qué
//! permisos la pidió el compilador (código R-X, constantes R--, datos RW-).
//!
//! El bootloader deja el archivo ELF del kernel en memoria (`kernel_addr`); de ahí se leen los
//! encabezados de programa. Referencia: especificación ELF64, "Program Header".

use alloc::vec::Vec;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Dirección virtual donde quedó (ya sumado el desplazamiento de carga).
    pub start: u64,
    pub end: u64,
    pub writable: bool,
    pub executable: bool,
}

impl Segment {
    pub fn contains(&self, virt: u64) -> bool {
        (self.start..self.end).contains(&virt)
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Los segmentos `PT_LOAD` de `elf`, corridos `load_offset` (el kernel es PIE: el bootloader
/// elige dónde ponerlo). `None` si no es un ELF64 little-endian.
pub fn load_segments(elf: &[u8], load_offset: u64) -> Option<Vec<Segment>> {
    if elf.get(..4)? != b"\x7fELF" || *elf.get(4)? != 2 || *elf.get(5)? != 1 {
        return None;
    }
    let phoff = u64_at(elf, 0x20)? as usize;
    let phentsize = u16_at(elf, 0x36)? as usize;
    let phnum = u16_at(elf, 0x38)? as usize;
    if phentsize < 56 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if u32_at(elf, ph)? != PT_LOAD {
            continue;
        }
        let flags = u32_at(elf, ph + 4)?;
        let vaddr = u64_at(elf, ph + 0x10)?;
        let memsz = u64_at(elf, ph + 0x28)?;
        if memsz == 0 {
            continue;
        }
        let start = load_offset.wrapping_add(vaddr);
        out.push(Segment {
            start,
            end: start.wrapping_add(memsz),
            writable: flags & PF_W != 0,
            executable: flags & PF_X != 0,
        });
    }
    Some(out)
}

/// Los permisos que le corresponden a una página del kernel que empieza en `virt` (con
/// `old` los que le dio el bootloader): lo que no es código no se ejecuta y lo que no es datos
/// no se escribe (W^X: nunca las dos cosas a la vez). Una página que comparte dos segmentos
/// suma los permisos de ambos. Fuera de los segmentos, sin cambios.
pub fn tighten(old: u64, virt: u64, size: u64, segments: &[Segment]) -> u64 {
    use crate::table::flags::{NO_EXECUTE, WRITABLE};
    let touching: Vec<&Segment> = segments
        .iter()
        .filter(|s| s.start < virt + size && virt < s.end)
        .collect();
    if touching.is_empty() {
        return old;
    }
    let writable = touching.iter().any(|s| s.writable);
    let executable = touching.iter().any(|s| s.executable);
    let mut f = old & !WRITABLE & !NO_EXECUTE;
    if writable {
        f |= WRITABLE;
    }
    if !executable {
        f |= NO_EXECUTE;
    }
    f
}
