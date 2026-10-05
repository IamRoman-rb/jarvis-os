//! El cargador de ELF: qué partes del archivo van a qué direcciones y con qué permisos.
//!
//! Un ejecutable ELF64 trae una tabla de *program headers*; los `PT_LOAD` dicen "copiá estos
//! `filesz` bytes del archivo a esta dirección, reservá `memsz` (lo que sobra, en cero: el
//! `.bss`) y dale estos permisos". Dos tipos nos sirven:
//!
//! - `ET_EXEC`: direcciones fijas (típicamente desde 0x400000).
//! - `ET_DYN`: un *PIE*. Se puede cargar en cualquier lado; lo ponemos en [`PIE_BASE`].
//!
//! Si el ELF pide un intérprete (`PT_INTERP`, normalmente `/lib64/ld-linux-x86-64.so.2`), usa
//! bibliotecas dinámicas (glibc): el kernel carga también el intérprete ([`parse_interpreter`],
//! en [`INTERP_BASE`]) y arranca por él; el intérprete carga las bibliotecas (`libc.so.6`…) con
//! `open`/`mmap` y salta al programa (`AT_ENTRY`). Así lo hace Linux.
//! Referencia: la especificación "System V ABI, AMD64 Architecture Processor Supplement".

use alloc::vec::Vec;

use crate::abi::prot;

/// Dónde buscar el intérprete `path` de un programa dinámico. FAT32 no tiene enlaces simbólicos:
/// `/lib64/ld-linux-x86-64.so.2` es un enlace en Debian, así que también se busca con el mismo
/// nombre en las carpetas de bibliotecas.
pub fn interpreter_paths(path: &str) -> [alloc::string::String; 4] {
    let name = path.rsplit('/').next().unwrap_or(path);
    [
        path.into(),
        alloc::format!("/lib/x86_64-linux-gnu/{name}"),
        alloc::format!("/usr/lib/x86_64-linux-gnu/{name}"),
        alloc::format!("/usr/lib64/{name}"),
    ]
}

/// Dónde se carga un static-pie (como `ELF_ET_DYN_BASE` en Linux, pero más abajo).
pub const PIE_BASE: u64 = 0x40_0000;
/// Dónde se carga el intérprete de un programa dinámico (`ld-linux-x86-64.so.2`): bien arriba,
/// lejos del heap del programa y debajo de los `mmap`.
pub const INTERP_BASE: u64 = 0x7e_0000_0000;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Dirección (ya con la base sumada) donde empieza.
    pub vaddr: u64,
    /// Bytes que se copian del archivo (desde `offset`).
    pub filesz: u64,
    /// Bytes que ocupa en memoria (lo que pasa de `filesz` va en cero).
    pub memsz: u64,
    pub offset: u64,
    /// `prot::READ | WRITE | EXEC`.
    pub prot: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub entry: u64,
    pub segments: Vec<Segment>,
    /// Dónde queda la tabla de program headers en la memoria del programa (para `AT_PHDR`).
    pub phdr: u64,
    pub phnum: u64,
    pub phent: u64,
    /// La base sumada a todas las direcciones (0 para `ET_EXEC`).
    pub base: u64,
    /// La dirección más alta que ocupa (el `brk` empieza en la página siguiente).
    pub end: u64,
    /// El intérprete que pide (`PT_INTERP`), si usa bibliotecas dinámicas.
    pub interp: Option<alloc::string::String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    NotElf,
    /// ELF de 32 bits, big endian, de otra arquitectura o que no es ejecutable.
    WrongKind(&'static str),
    /// Pide bibliotecas dinámicas y su intérprete no está en el disco (el que nombra).
    Dynamic(alloc::string::String),
    Broken(&'static str),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::NotElf => f.write_str("no es un programa ELF"),
            LoadError::WrongKind(s) => write!(f, "no es un programa para este sistema: {s}"),
            LoadError::Dynamic(i) => write!(
                f,
                "usa bibliotecas dinamicas y falta su interprete ({i}): instala libc6 (apt install libc6)"
            ),
            LoadError::Broken(s) => write!(f, "el programa esta danado: {s}"),
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, LoadError> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(LoadError::Broken("encabezado cortado"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, LoadError> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(LoadError::Broken("encabezado cortado"))
}

fn u64_at(b: &[u8], o: usize) -> Result<u64, LoadError> {
    b.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap_or([0; 8])))
        .ok_or(LoadError::Broken("encabezado cortado"))
}

/// Lee el ELF y decide dónde va cada cosa. No toca memoria: eso lo hace quien carga.
pub fn parse(file: &[u8], user_end: u64) -> Result<Program, LoadError> {
    parse_at(file, user_end, None)
}

/// El intérprete de un programa dinámico (`ld.so`): un `ET_DYN` sin intérprete propio, cargado
/// en `base`.
pub fn parse_interpreter(file: &[u8], user_end: u64, base: u64) -> Result<Program, LoadError> {
    let p = parse_at(file, user_end, Some(base))?;
    if p.interp.is_some() {
        return Err(LoadError::WrongKind("el intérprete pide otro intérprete"));
    }
    Ok(p)
}

fn parse_at(file: &[u8], user_end: u64, at: Option<u64>) -> Result<Program, LoadError> {
    if !file.starts_with(b"\x7fELF") {
        return Err(LoadError::NotElf);
    }
    if file.get(4) != Some(&2) {
        return Err(LoadError::WrongKind("de 32 bits"));
    }
    if file.get(5) != Some(&1) {
        return Err(LoadError::WrongKind("big endian"));
    }
    let kind = u16_at(file, 16)?;
    if u16_at(file, 18)? != 62 {
        return Err(LoadError::WrongKind("de otra arquitectura (no x86_64)"));
    }
    let base = match (kind, at) {
        (3, Some(b)) => b,
        (_, Some(_)) => return Err(LoadError::WrongKind("el intérprete no es reubicable")),
        (2, None) => 0,
        (3, None) => PIE_BASE,
        _ => {
            return Err(LoadError::WrongKind(
                "no es un ejecutable (¿una biblioteca u objeto?)",
            ));
        }
    };
    let entry = u64_at(file, 24)?;
    let phoff = u64_at(file, 32)? as usize;
    let phent = u16_at(file, 54)? as usize;
    let phnum = u16_at(file, 56)? as usize;
    if phent < 56 || phnum == 0 || phnum > 128 {
        return Err(LoadError::Broken("tabla de program headers inválida"));
    }
    let mut segments = Vec::new();
    let mut phdr = None;
    let mut interp = None;
    for i in 0..phnum {
        let h = phoff
            .checked_add(i * phent)
            .ok_or(LoadError::Broken("tabla fuera del archivo"))?;
        let kind = u32_at(file, h)?;
        let flags = u32_at(file, h + 4)?;
        let offset = u64_at(file, h + 8)?;
        let vaddr = u64_at(file, h + 16)?;
        let filesz = u64_at(file, h + 32)?;
        let memsz = u64_at(file, h + 40)?;
        match kind {
            PT_INTERP => {
                let end = offset.saturating_add(filesz) as usize;
                let name = file
                    .get(offset as usize..end)
                    .map(|s| s.split(|&b| b == 0).next().unwrap_or(s))
                    .map(|s| alloc::string::String::from_utf8_lossy(s).into_owned())
                    .unwrap_or_default();
                if name.is_empty() {
                    return Err(LoadError::Broken("intérprete sin nombre"));
                }
                interp = Some(name);
            }
            PT_PHDR => phdr = Some(base + vaddr),
            PT_LOAD if memsz > 0 => {
                if filesz > memsz || offset.saturating_add(filesz) > file.len() as u64 {
                    return Err(LoadError::Broken("segmento fuera del archivo"));
                }
                let start = base
                    .checked_add(vaddr)
                    .ok_or(LoadError::Broken("dirección inválida"))?;
                let end = start
                    .checked_add(memsz)
                    .ok_or(LoadError::Broken("dirección inválida"))?;
                if start < 0x1000 || end > user_end {
                    return Err(LoadError::Broken(
                        "segmento fuera de la memoria del programa",
                    ));
                }
                // El archivo y la memoria tienen que coincidir módulo la página: se mapean
                // páginas enteras.
                if vaddr % 4096 != offset % 4096 {
                    return Err(LoadError::Broken("segmento desalineado"));
                }
                // Los bits de p_flags: X = 1, W = 2, R = 4 (al revés que PROT_*).
                let mut p = 0;
                if flags & 4 != 0 {
                    p |= prot::READ;
                }
                if flags & 2 != 0 {
                    p |= prot::WRITE;
                }
                if flags & 1 != 0 {
                    p |= prot::EXEC;
                }
                segments.push(Segment {
                    vaddr: start,
                    filesz,
                    memsz,
                    offset,
                    prot: p,
                });
            }
            _ => {}
        }
    }
    if segments.is_empty() {
        return Err(LoadError::Broken("no tiene nada que cargar"));
    }
    segments.sort_by_key(|s| s.vaddr);
    for w in segments.windows(2) {
        // Dos segmentos pueden compartir una página (el final de uno y el principio del otro),
        // pero no pisarse.
        if w[0].vaddr + w[0].memsz > w[1].vaddr {
            return Err(LoadError::Broken("segmentos superpuestos"));
        }
    }
    // Sin PT_PHDR, la tabla está donde cae su desplazamiento dentro del primer segmento que la
    // contiene (así lo calcula Linux).
    let phdr = match phdr {
        Some(p) => p,
        None => segments
            .iter()
            .find(|s| s.offset <= phoff as u64 && (phoff as u64) < s.offset + s.filesz)
            .map(|s| s.vaddr + (phoff as u64 - s.offset))
            .unwrap_or(0),
    };
    let end = segments
        .iter()
        .map(|s| s.vaddr + s.memsz)
        .max()
        .unwrap_or(0);
    let entry = base + entry;
    if !segments
        .iter()
        .any(|s| s.prot & prot::EXEC != 0 && (s.vaddr..s.vaddr + s.memsz).contains(&entry))
    {
        return Err(LoadError::Broken("el punto de entrada no es código"));
    }
    Ok(Program {
        entry,
        segments,
        phdr,
        phnum: phnum as u64,
        phent: phent as u64,
        base,
        end,
        interp,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::vec;

    /// Un ELF mínimo a mano: un segmento de código (R-X) y uno de datos (RW-) con `.bss`.
    pub(crate) fn tiny(kind: u16, interp: bool) -> Vec<u8> {
        let mut f = vec![0u8; 0x2000];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2; // 64 bits
        f[5] = 1; // little endian
        f[6] = 1;
        f[16..18].copy_from_slice(&kind.to_le_bytes());
        f[18..20].copy_from_slice(&62u16.to_le_bytes());
        let base = if kind == 2 { 0x40_0000u64 } else { 0 };
        f[24..32].copy_from_slice(&(base + 0x1000).to_le_bytes()); // entrada
        f[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        let n = if interp { 3u16 } else { 2 };
        f[56..58].copy_from_slice(&n.to_le_bytes());
        let ph = |f: &mut Vec<u8>,
                  i: usize,
                  kind: u32,
                  flags: u32,
                  off: u64,
                  va: u64,
                  fs: u64,
                  ms: u64| {
            let h = 64 + i * 56;
            f[h..h + 4].copy_from_slice(&kind.to_le_bytes());
            f[h + 4..h + 8].copy_from_slice(&flags.to_le_bytes());
            f[h + 8..h + 16].copy_from_slice(&off.to_le_bytes());
            f[h + 16..h + 24].copy_from_slice(&va.to_le_bytes());
            f[h + 32..h + 40].copy_from_slice(&fs.to_le_bytes());
            f[h + 40..h + 48].copy_from_slice(&ms.to_le_bytes());
        };
        // Código: la página 0x1000 del archivo, con un `syscall` cualquiera.
        ph(&mut f, 0, PT_LOAD, 5, 0, base, 0x1010, 0x1010);
        // Datos: 16 bytes del archivo y 0x3000 de memoria (el resto, .bss).
        ph(&mut f, 1, PT_LOAD, 6, 0x1800, base + 0x2800, 16, 0x3000);
        if interp {
            f[0x1900..0x1900 + 28].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
            ph(&mut f, 2, PT_INTERP, 4, 0x1900, 0, 28, 28);
        }
        f[0x1000..0x1004].copy_from_slice(&[0x0F, 0x05, 0xEB, 0xFE]);
        f[0x1800..0x1810].copy_from_slice(b"datos iniciales!");
        f
    }

    #[test]
    fn ejecutable_fijo_y_static_pie() {
        let p = parse(&tiny(2, false), 1 << 39).unwrap();
        assert_eq!(p.base, 0);
        assert_eq!(p.entry, 0x40_1000);
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].prot, prot::READ | prot::EXEC);
        assert_eq!(p.segments[1].prot, prot::READ | prot::WRITE);
        assert_eq!(p.phdr, 0x40_0040, "la tabla está en el primer segmento");
        assert_eq!(p.end, 0x40_5800);

        let p = parse(&tiny(3, false), 1 << 39).unwrap();
        assert_eq!(p.base, PIE_BASE);
        assert_eq!(p.entry, PIE_BASE + 0x1000);
        assert_eq!(p.segments[1].vaddr, PIE_BASE + 0x2800);
    }

    #[test]
    fn dinamico_con_su_interprete() {
        let p = parse(&tiny(3, true), 1 << 39).unwrap();
        assert_eq!(p.interp.as_deref(), Some("/lib64/ld-linux-x86-64.so.2"));
        assert_eq!(p.base, PIE_BASE);
        // El intérprete (otro ET_DYN) va en su propia base.
        let i = parse_interpreter(&tiny(3, false), 1 << 39, INTERP_BASE).unwrap();
        assert_eq!(i.base, INTERP_BASE);
        assert_eq!(i.entry, INTERP_BASE + 0x1000);
        assert!(i.interp.is_none());
        assert!(parse(&tiny(2, false), 1 << 39).unwrap().interp.is_none());
    }

    #[test]
    fn rechaza_lo_que_no_puede_correr() {
        assert_eq!(parse(b"MZ\x90\x00", 1 << 39), Err(LoadError::NotElf));
        // El intérprete no puede pedir otro, y tiene que ser reubicable.
        assert!(matches!(
            parse_interpreter(&tiny(3, true), 1 << 39, INTERP_BASE),
            Err(LoadError::WrongKind(_))
        ));
        assert!(matches!(
            parse_interpreter(&tiny(2, false), 1 << 39, INTERP_BASE),
            Err(LoadError::WrongKind(_))
        ));
        let mut f = tiny(2, false);
        f[18] = 40; // ARM
        assert!(matches!(parse(&f, 1 << 39), Err(LoadError::WrongKind(_))));
        let mut f = tiny(2, false);
        f[4] = 1;
        assert!(matches!(parse(&f, 1 << 39), Err(LoadError::WrongKind(_))));
        // Un segmento que se sale de la memoria del programa.
        assert!(matches!(
            parse(&tiny(2, false), 0x40_4000),
            Err(LoadError::Broken(_))
        ));
        // Cortado.
        let f = tiny(2, false);
        for cut in [10, 60, 100] {
            assert!(parse(&f[..cut], 1 << 39).is_err());
        }
    }
}
