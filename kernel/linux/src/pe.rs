//! El formato de los programas de Windows: PE32+ ("Portable Executable", x86_64).
//!
//! Un `.exe` empieza con el encabezado de MS-DOS (`MZ`), que en `0x3C` dice dónde está el de PE
//! (`PE\0\0`): la máquina, cuántas secciones hay y el "encabezado opcional" (que no tiene nada de
//! opcional) con la dirección preferida (`ImageBase`), el punto de entrada y los directorios de
//! datos: imports, relocalizaciones y TLS, entre otros. Las direcciones son **RVA**: relativas al
//! comienzo de la imagen cargada.
//!
//! Las DLL tienen el mismo formato, más la tabla de **exports**: qué funciones ofrecen, por
//! nombre o por número (ordinal). Algunas no están en la DLL sino que la "reenvían" a otra
//! (`KERNEL32.HeapAlloc` es en realidad `NTDLL.RtlAllocateHeap`).
//!
//! Lo que hace el cargador con esto está en `process::win`. Referencia: "PE Format" de Microsoft
//! (learn.microsoft.com/windows/win32/debug/pe-format).

use alloc::string::String;
use alloc::vec::Vec;

use crate::abi::prot;
use crate::elf::LoadError;

const MACHINE_AMD64: u16 = 0x8664;
const MACHINE_I386: u16 = 0x14c;
const MACHINE_ARM64: u16 = 0xaa64;
const PE32_PLUS: u16 = 0x20b;
const PE32: u16 = 0x10b;
const FILE_DLL: u16 = 0x2000;

const DIR_EXPORT: usize = 0;
const DIR_IMPORT: usize = 1;
const DIR_BASERELOC: usize = 5;
const DIR_TLS: usize = 9;
const DIR_CLR: usize = 14;

const SCN_EXECUTE: u32 = 0x2000_0000;
const SCN_READ: u32 = 0x4000_0000;
const SCN_WRITE: u32 = 0x8000_0000;

/// Una sección: dónde va en la imagen, qué se copia del archivo y con qué permisos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub rva: u32,
    pub size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub prot: u32,
}

/// Una función (o dato) que el programa toma de una DLL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Symbol {
    Name(String),
    Ordinal(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// La DLL, en minúsculas (`kernel32.dll`).
    pub dll: String,
    /// Cada entrada de su tabla de direcciones (IAT): la RVA donde va la dirección y qué es.
    pub entries: Vec<(u32, Symbol)>,
}

/// Dónde está una función que ofrece una DLL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// En la DLL misma, en esta RVA.
    Rva(u32),
    /// En otra DLL: `"NTDLL.RtlAllocateHeap"` (o `"NTDLL.#12"`, por ordinal).
    Forward(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// Sin nombre: solo se la puede pedir por ordinal.
    pub name: Option<String>,
    pub ordinal: u16,
    pub target: Target,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tls {
    /// Direcciones absolutas (con la `ImageBase` preferida): el molde de los datos, su final, la
    /// variable donde va el índice y la lista de funciones a llamar al arrancar.
    pub start: u64,
    pub end: u64,
    pub index_addr: u64,
    pub callbacks: u64,
    pub zero_fill: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub image_base: u64,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub entry_rva: u32,
    /// 2: con ventanas (GUI); 3: de consola.
    pub subsystem: u16,
    pub stack_reserve: u64,
    pub sections: Vec<Section>,
    pub imports: Vec<Import>,
    /// Las relocalizaciones (`IMAGE_REL_BASED_DIR64`): RVAs de valores de 64 bits a corregir si
    /// la imagen no va en su dirección preferida.
    pub relocs: Vec<u32>,
    pub tls: Option<Tls>,
    /// Es una biblioteca (DLL), no un programa.
    pub dll: bool,
    /// Lo que ofrece (vacío en casi todos los programas).
    pub exports: Vec<Export>,
}

impl Image {
    pub fn gui(&self) -> bool {
        self.subsystem == 2
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// ¿Empieza como un programa de Windows?
pub fn is_pe(file: &[u8]) -> bool {
    file.starts_with(b"MZ")
        && u32_at(file, 0x3c)
            .and_then(|o| file.get(o as usize..o as usize + 4))
            .is_some_and(|s| s == b"PE\0\0")
}

/// Un programa: el encabezado, las secciones, los imports, las relocalizaciones y el TLS. Una DLL
/// se rechaza (no se puede "ejecutar").
pub fn parse(file: &[u8]) -> Result<Image, LoadError> {
    parse_image(file, false)
}

/// Una DLL, con su tabla de exports.
pub fn parse_dll(file: &[u8]) -> Result<Image, LoadError> {
    parse_image(file, true)
}

fn parse_image(file: &[u8], want_dll: bool) -> Result<Image, LoadError> {
    let broken = LoadError::Broken;
    if !is_pe(file) {
        return Err(LoadError::NotElf);
    }
    let pe = u32_at(file, 0x3c).ok_or(broken("encabezado"))? as usize;
    let coff = pe + 4;
    let machine = u16_at(file, coff).ok_or(broken("encabezado"))?;
    let nsections = u16_at(file, coff + 2).ok_or(broken("encabezado"))? as usize;
    let opt_size = u16_at(file, coff + 16).ok_or(broken("encabezado"))? as usize;
    let characteristics = u16_at(file, coff + 18).ok_or(broken("encabezado"))?;
    let opt = coff + 20;
    let magic = u16_at(file, opt).ok_or(broken("encabezado opcional"))?;
    match (machine, magic) {
        (MACHINE_AMD64, PE32_PLUS) => {}
        (MACHINE_I386, _) | (_, PE32) => {
            return Err(LoadError::WrongKind(
                "es un programa de Windows de 32 bits (x86); JARVIS-OS corre los de 64 bits",
            ));
        }
        (MACHINE_ARM64, _) => {
            return Err(LoadError::WrongKind("es un programa de Windows para ARM64"));
        }
        _ => return Err(LoadError::WrongKind("es para otra arquitectura")),
    }
    let dll = characteristics & FILE_DLL != 0;
    if dll && !want_dll {
        return Err(LoadError::WrongKind(
            "es una biblioteca de Windows (DLL), no un programa",
        ));
    }
    if want_dll && !dll {
        return Err(LoadError::WrongKind(
            "no es una biblioteca de Windows (DLL)",
        ));
    }
    let entry_rva = u32_at(file, opt + 16).ok_or(broken("encabezado opcional"))?;
    let image_base = u64_at(file, opt + 24).ok_or(broken("encabezado opcional"))?;
    let size_of_image = u32_at(file, opt + 56).ok_or(broken("encabezado opcional"))?;
    let size_of_headers = u32_at(file, opt + 60).ok_or(broken("encabezado opcional"))?;
    let subsystem = u16_at(file, opt + 68).ok_or(broken("encabezado opcional"))?;
    let stack_reserve = u64_at(file, opt + 72).ok_or(broken("encabezado opcional"))?;
    let ndirs = u32_at(file, opt + 108).ok_or(broken("encabezado opcional"))? as usize;
    let dir = |i: usize| -> (u32, u32) {
        if i >= ndirs.min(16) || 112 + i * 8 + 8 > opt_size {
            return (0, 0);
        }
        let o = opt + 112 + i * 8;
        (
            u32_at(file, o).unwrap_or(0),
            u32_at(file, o + 4).unwrap_or(0),
        )
    };
    if dir(DIR_CLR).0 != 0 {
        return Err(LoadError::WrongKind(
            "es un programa de .NET: necesita el entorno de .NET, que JARVIS-OS no tiene",
        ));
    }
    if size_of_image == 0 || size_of_image > 1 << 30 {
        return Err(broken("tamaño de imagen inválido"));
    }

    let mut sections = Vec::new();
    let table = opt + opt_size;
    for i in 0..nsections {
        let s = table + i * 40;
        let raw_name = file.get(s..s + 8).ok_or(broken("tabla de secciones"))?;
        let name_len = raw_name.iter().position(|&c| c == 0).unwrap_or(8);
        let ch = u32_at(file, s + 36).ok_or(broken("tabla de secciones"))?;
        let mut p = 0;
        if ch & SCN_READ != 0 {
            p |= prot::READ;
        }
        if ch & SCN_WRITE != 0 {
            p |= prot::WRITE;
        }
        if ch & SCN_EXECUTE != 0 {
            p |= prot::EXEC | prot::READ;
        }
        let virtual_size = u32_at(file, s + 8).ok_or(broken("tabla de secciones"))?;
        let raw_size = u32_at(file, s + 16).ok_or(broken("tabla de secciones"))?;
        let section = Section {
            name: String::from_utf8_lossy(&raw_name[..name_len]).into_owned(),
            rva: u32_at(file, s + 12).ok_or(broken("tabla de secciones"))?,
            size: if virtual_size == 0 {
                raw_size
            } else {
                virtual_size
            },
            raw_offset: u32_at(file, s + 20).ok_or(broken("tabla de secciones"))?,
            raw_size,
            prot: p,
        };
        if section.rva as u64 + section.size as u64 > size_of_image as u64
            || (section.raw_size > 0
                && section.raw_offset as usize + (section.raw_size.min(section.size) as usize)
                    > file.len())
        {
            return Err(broken("una sección se sale del archivo"));
        }
        sections.push(section);
    }

    let mut img = Image {
        image_base,
        size_of_image,
        size_of_headers,
        entry_rva,
        subsystem,
        stack_reserve,
        sections,
        imports: Vec::new(),
        relocs: Vec::new(),
        tls: None,
        dll,
        exports: Vec::new(),
    };
    img.exports = exports(file, &img, dir(DIR_EXPORT))?;
    img.imports = imports(file, &img, dir(DIR_IMPORT).0)?;
    img.relocs = relocs(file, &img, dir(DIR_BASERELOC))?;
    let (tls_rva, _) = dir(DIR_TLS);
    if tls_rva != 0 {
        let o = img.offset(tls_rva).ok_or(broken("TLS"))?;
        img.tls = Some(Tls {
            start: u64_at(file, o).ok_or(broken("TLS"))?,
            end: u64_at(file, o + 8).ok_or(broken("TLS"))?,
            index_addr: u64_at(file, o + 16).ok_or(broken("TLS"))?,
            callbacks: u64_at(file, o + 24).ok_or(broken("TLS"))?,
            zero_fill: u32_at(file, o + 32).ok_or(broken("TLS"))?,
        });
    }
    Ok(img)
}

impl Image {
    /// Dónde está en el archivo lo que va en `rva` (si está en el archivo).
    pub fn offset(&self, rva: u32) -> Option<usize> {
        if rva < self.size_of_headers {
            return Some(rva as usize);
        }
        self.sections.iter().find_map(|s| {
            (s.rva <= rva && rva < s.rva + s.raw_size.min(s.size).max(1))
                .then(|| (s.raw_offset + (rva - s.rva)) as usize)
        })
    }
}

impl Image {
    /// La función `name` que ofrece la DLL.
    pub fn export(&self, name: &str) -> Option<&Export> {
        self.exports
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
    }

    pub fn export_ordinal(&self, ordinal: u16) -> Option<&Export> {
        self.exports.iter().find(|e| e.ordinal == ordinal)
    }
}

fn exports(file: &[u8], img: &Image, (rva, size): (u32, u32)) -> Result<Vec<Export>, LoadError> {
    let broken = LoadError::Broken("tabla de exports");
    let mut out = Vec::new();
    if rva == 0 {
        return Ok(out);
    }
    let d = img.offset(rva).ok_or(broken.clone())?;
    let base = u32_at(file, d + 16).ok_or(broken.clone())?;
    let nfuncs = u32_at(file, d + 20).ok_or(broken.clone())? as usize;
    let nnames = u32_at(file, d + 24).ok_or(broken.clone())? as usize;
    if nfuncs > 65536 || nnames > nfuncs {
        return Err(broken);
    }
    let funcs = img
        .offset(u32_at(file, d + 28).ok_or(broken.clone())?)
        .ok_or(broken.clone())?;
    // Los nombres: cada uno apunta (por su índice en la tabla de ordinales) a una función.
    let mut names: Vec<Option<String>> = alloc::vec![None; nfuncs];
    if nnames > 0 {
        let name_ptrs = img
            .offset(u32_at(file, d + 32).ok_or(broken.clone())?)
            .ok_or(broken.clone())?;
        let ords = img
            .offset(u32_at(file, d + 36).ok_or(broken.clone())?)
            .ok_or(broken.clone())?;
        for i in 0..nnames {
            let name_rva = u32_at(file, name_ptrs + i * 4).ok_or(broken.clone())?;
            let idx = u16_at(file, ords + i * 2).ok_or(broken.clone())? as usize;
            let name = img
                .offset(name_rva)
                .and_then(|o| cstr(file, o))
                .ok_or(broken.clone())?;
            if let Some(slot) = names.get_mut(idx) {
                *slot = Some(name);
            }
        }
    }
    for (i, name) in names.into_iter().enumerate() {
        let f = u32_at(file, funcs + i * 4).ok_or(broken.clone())?;
        if f == 0 {
            continue; // un hueco en la numeración
        }
        // Si la "dirección" cae adentro de la tabla de exports, es el nombre de otra DLL.
        let target = if f >= rva && f < rva + size {
            Target::Forward(
                img.offset(f)
                    .and_then(|o| cstr(file, o))
                    .ok_or(broken.clone())?,
            )
        } else {
            Target::Rva(f)
        };
        out.push(Export {
            name,
            ordinal: (base as usize + i) as u16,
            target,
        });
    }
    Ok(out)
}

fn cstr(file: &[u8], o: usize) -> Option<String> {
    let rest = file.get(o..)?;
    let n = rest.iter().take(512).position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&rest[..n]).into_owned())
}

fn imports(file: &[u8], img: &Image, rva: u32) -> Result<Vec<Import>, LoadError> {
    let broken = LoadError::Broken;
    let mut out = Vec::new();
    if rva == 0 {
        return Ok(out);
    }
    let mut d = img.offset(rva).ok_or(broken("tabla de imports"))?;
    loop {
        let lookup = u32_at(file, d).ok_or(broken("tabla de imports"))?;
        let name_rva = u32_at(file, d + 12).ok_or(broken("tabla de imports"))?;
        let iat = u32_at(file, d + 16).ok_or(broken("tabla de imports"))?;
        if name_rva == 0 && iat == 0 {
            break;
        }
        let dll = img
            .offset(name_rva)
            .and_then(|o| cstr(file, o))
            .ok_or(broken("nombre de una DLL"))?
            .to_ascii_lowercase();
        // La tabla de nombres (si no está, la IAT misma tiene los nombres antes de cargar).
        let names = if lookup != 0 { lookup } else { iat };
        let mut o = img.offset(names).ok_or(broken("tabla de imports"))?;
        let mut entries = Vec::new();
        let mut slot = iat;
        loop {
            let v = u64_at(file, o).ok_or(broken("tabla de imports"))?;
            if v == 0 {
                break;
            }
            let sym = if v & (1 << 63) != 0 {
                Symbol::Ordinal(v as u16)
            } else {
                let at = img
                    .offset(v as u32)
                    .ok_or(broken("nombre de una función importada"))?;
                Symbol::Name(cstr(file, at + 2).ok_or(broken("nombre de una función"))?)
            };
            entries.push((slot, sym));
            slot += 8;
            o += 8;
            if entries.len() > 65536 {
                return Err(broken("demasiados imports"));
            }
        }
        out.push(Import { dll, entries });
        d += 20;
        if out.len() > 4096 {
            return Err(broken("demasiadas DLL"));
        }
    }
    Ok(out)
}

fn relocs(file: &[u8], img: &Image, (rva, size): (u32, u32)) -> Result<Vec<u32>, LoadError> {
    let mut out = Vec::new();
    if rva == 0 {
        return Ok(out);
    }
    let Some(mut o) = img.offset(rva) else {
        return Ok(out);
    };
    let end = o + size as usize;
    while o + 8 <= end {
        let page = u32_at(file, o).ok_or(LoadError::Broken("relocalizaciones"))?;
        let block = u32_at(file, o + 4).ok_or(LoadError::Broken("relocalizaciones"))? as usize;
        if block < 8 {
            break;
        }
        for i in (o + 8..o + block).step_by(2) {
            let e = u16_at(file, i).ok_or(LoadError::Broken("relocalizaciones"))?;
            match e >> 12 {
                0 => {} // relleno
                10 => out.push(page + (e & 0xfff) as u32),
                _ => return Err(LoadError::Broken("una relocalización desconocida")),
            }
        }
        o += block;
    }
    Ok(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::vec;

    /// Un `.exe` mínimo armado a mano: una sección de código (`.text`, con `ret`) y una de
    /// datos con los imports de `KERNEL32.dll` (`ExitProcess` y el ordinal 7) y `msvcrt.dll`.
    pub fn tiny() -> Vec<u8> {
        let mut f = vec![0u8; 0x600];
        f[0..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        f[0x80..0x84].copy_from_slice(b"PE\0\0");
        let w16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let w64 = |f: &mut Vec<u8>, o: usize, v: u64| f[o..o + 8].copy_from_slice(&v.to_le_bytes());
        let coff = 0x84;
        w16(&mut f, coff, MACHINE_AMD64);
        w16(&mut f, coff + 2, 2);
        w16(&mut f, coff + 16, 240);
        w16(&mut f, coff + 18, 0x22);
        let opt = coff + 20;
        w16(&mut f, opt, PE32_PLUS);
        w32(&mut f, opt + 16, 0x1000); // entrada
        w64(&mut f, opt + 24, 0x1_4000_0000);
        w32(&mut f, opt + 56, 0x3000);
        w32(&mut f, opt + 60, 0x200);
        w16(&mut f, opt + 68, 3);
        w64(&mut f, opt + 72, 0x10_0000);
        w32(&mut f, opt + 108, 16);
        w32(&mut f, opt + 112 + DIR_IMPORT * 8, 0x2000);
        let t = opt + 240;
        f[t..t + 5].copy_from_slice(b".text");
        w32(&mut f, t + 8, 0x10);
        w32(&mut f, t + 12, 0x1000);
        w32(&mut f, t + 16, 0x200);
        w32(&mut f, t + 20, 0x200);
        w32(&mut f, t + 36, SCN_EXECUTE | SCN_READ);
        let t = t + 40;
        f[t..t + 6].copy_from_slice(b".idata");
        w32(&mut f, t + 8, 0x200);
        w32(&mut f, t + 12, 0x2000);
        w32(&mut f, t + 16, 0x200);
        w32(&mut f, t + 20, 0x400);
        w32(&mut f, t + 36, SCN_READ | SCN_WRITE);
        f[0x200] = 0xc3;
        // .idata en 0x400 (RVA 0x2000): dos descriptores y el final.
        let d = 0x400;
        w32(&mut f, d, 0x2080); // nombres
        w32(&mut f, d + 12, 0x2100); // "KERNEL32.dll"
        w32(&mut f, d + 16, 0x20c0); // IAT
        w32(&mut f, d + 20, 0); // sin tabla de nombres: la IAT los tiene
        w32(&mut f, d + 20 + 12, 0x2110);
        w32(&mut f, d + 20 + 16, 0x20e0);
        w64(&mut f, 0x480, 0x2120); // ExitProcess
        w64(&mut f, 0x488, (1 << 63) | 7);
        w64(&mut f, 0x4c0, 0x2120);
        w64(&mut f, 0x4c8, (1 << 63) | 7);
        w64(&mut f, 0x4e0, 0x2130); // puts
        f[0x500..0x50c].copy_from_slice(b"KERNEL32.dll");
        f[0x510..0x51a].copy_from_slice(b"msvcrt.dll");
        f[0x522..0x52d].copy_from_slice(b"ExitProcess");
        f[0x532..0x536].copy_from_slice(b"puts");
        f
    }

    #[test]
    fn lee_un_exe_minimo() {
        let img = parse(&tiny()).unwrap();
        assert_eq!(img.image_base, 0x1_4000_0000);
        assert_eq!(img.entry_rva, 0x1000);
        assert!(!img.gui());
        assert_eq!(img.sections.len(), 2);
        assert_eq!(img.sections[0].prot, prot::READ | prot::EXEC);
        assert_eq!(img.imports.len(), 2);
        assert_eq!(img.imports[0].dll, "kernel32.dll");
        assert_eq!(
            img.imports[0].entries,
            vec![
                (0x20c0, Symbol::Name("ExitProcess".into())),
                (0x20c8, Symbol::Ordinal(7))
            ]
        );
        assert_eq!(
            img.imports[1].entries,
            vec![(0x20e0, Symbol::Name("puts".into()))]
        );
    }

    /// La misma imagen como DLL, con dos exports: `Hola` (en 0x1000) y el ordinal 2, sin
    /// nombre, que reenvía a `NTDLL.RtlFoo`.
    pub fn tiny_dll() -> Vec<u8> {
        let mut f = tiny();
        f.resize(0x800, 0);
        let w16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
        f[0x84 + 19] |= (FILE_DLL >> 8) as u8;
        let opt = 0x84 + 20;
        // .idata ahora mide 0x400 (RVA 0x2000..0x2400): ahí también va la tabla de exports.
        let t = opt + 240 + 40;
        w32(&mut f, t + 8, 0x400);
        w32(&mut f, t + 16, 0x400);
        w32(&mut f, opt + 112 + DIR_EXPORT * 8, 0x2200);
        w32(&mut f, opt + 112 + DIR_EXPORT * 8 + 4, 0xa0);
        let d = 0x600; // RVA 0x2200
        w32(&mut f, d + 12, 0x2280); // nombre de la DLL
        w32(&mut f, d + 16, 1); // primer ordinal
        w32(&mut f, d + 20, 2); // funciones
        w32(&mut f, d + 24, 1); // nombres
        w32(&mut f, d + 28, 0x2240);
        w32(&mut f, d + 32, 0x2250);
        w32(&mut f, d + 36, 0x2260);
        w32(&mut f, 0x640, 0x1000);
        w32(&mut f, 0x644, 0x2290); // adentro de la tabla: reenvío
        w32(&mut f, 0x650, 0x2270);
        w16(&mut f, 0x660, 0);
        f[0x670..0x674].copy_from_slice(b"Hola");
        f[0x680..0x68a].copy_from_slice(b"jarvis.dll");
        f[0x690..0x69c].copy_from_slice(b"NTDLL.RtlFoo");
        f
    }

    #[test]
    fn lee_los_exports_de_una_dll() {
        let f = tiny_dll();
        assert!(matches!(parse(&f), Err(LoadError::WrongKind(m)) if m.contains("DLL")));
        let img = parse_dll(&f).unwrap();
        assert!(img.dll);
        assert_eq!(
            img.export("Hola"),
            Some(&Export {
                name: Some("Hola".into()),
                ordinal: 1,
                target: Target::Rva(0x1000),
            })
        );
        assert_eq!(
            img.export_ordinal(2).map(|e| &e.target),
            Some(&Target::Forward("NTDLL.RtlFoo".into()))
        );
        assert!(img.export("Chau").is_none());
        // Un programa no es una DLL.
        assert!(matches!(parse_dll(&tiny()), Err(LoadError::WrongKind(_))));
        assert!(parse(&tiny()).unwrap().exports.is_empty());
    }

    #[test]
    fn rechaza_lo_que_no_puede_correr() {
        let mut f = tiny();
        f[0x84..0x86].copy_from_slice(&MACHINE_I386.to_le_bytes());
        assert!(matches!(parse(&f), Err(LoadError::WrongKind(m)) if m.contains("32 bits")));
        let mut f = tiny();
        f[0x84 + 19] |= (FILE_DLL >> 8) as u8;
        assert!(matches!(parse(&f), Err(LoadError::WrongKind(m)) if m.contains("DLL")));
        let mut f = tiny();
        let clr = 0x84 + 20 + 112 + DIR_CLR * 8;
        f[clr] = 1;
        assert!(matches!(parse(&f), Err(LoadError::WrongKind(m)) if m.contains(".NET")));
        assert!(matches!(parse(b"MZ"), Err(LoadError::NotElf)));
        assert!(!is_pe(b"\x7fELF"));
    }
}
