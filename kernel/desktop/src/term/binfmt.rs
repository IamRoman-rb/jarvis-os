//! ¿Qué es este archivo? Reconoce programas de Windows (`.exe`, formato PE), de Linux (ELF),
//! paquetes `.deb`, imágenes, comprimidos y texto, mirando sus primeros bytes (como `file`).
//!
//! Los programas se pueden **descargar e inspeccionar**, pero todavía no **ejecutar**: para eso
//! el kernel necesita espacio de usuario, un cargador de programas y las llamadas al sistema que
//! esos programas esperan (Win32 o Linux). Ver ADR 0005.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Programa o biblioteca de Windows.
    Pe(PeInfo),
    /// Programa o biblioteca de Linux.
    Elf(ElfInfo),
    /// Script con `#!` o texto con comandos.
    Script,
    Text,
    Deb,
    /// Sistema de archivos comprimido de Linux: un paquete `.snap` (o una imagen de AppImage).
    Squashfs,
    Other(&'static str),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: &'static str,
    pub bits: u8,
    pub gui: bool,
    pub dll: bool,
    pub dotnet: bool,
    pub sections: u16,
    pub imports: Vec<String>,
    /// Instalador conocido (Inno Setup, NSIS…).
    pub installer: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElfInfo {
    pub machine: &'static str,
    pub bits: u8,
    pub shared: bool,
    pub interpreter: Option<String>,
}

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn cstr(b: &[u8], at: usize) -> Option<String> {
    let rest = b.get(at..)?;
    let end = rest.iter().position(|&c| c == 0)?.min(128);
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

pub fn detect(b: &[u8]) -> Kind {
    if b.starts_with(b"MZ")
        && let Some(pe) = parse_pe(b)
    {
        return Kind::Pe(pe);
    }
    if b.starts_with(b"\x7fELF")
        && let Some(elf) = parse_elf(b)
    {
        return Kind::Elf(elf);
    }
    if b.starts_with(b"!<arch>\ndebian-binary") {
        return Kind::Deb;
    }
    if b.starts_with(b"hsqs") {
        return Kind::Squashfs;
    }
    if b.starts_with(b"#!") {
        return Kind::Script;
    }
    let other = if b.starts_with(b"PK\x03\x04") {
        Some("archivo comprimido ZIP")
    } else if b.starts_with(b"%PDF") {
        Some("documento PDF")
    } else if b.starts_with(b"\x89PNG") {
        Some("imagen PNG")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("imagen JPEG")
    } else if b.starts_with(b"GIF8") {
        Some("imagen GIF")
    } else if b.starts_with(b"BM") && b.len() > 54 {
        Some("imagen BMP")
    } else if b.starts_with(&[0x1f, 0x8b]) {
        Some("comprimido gzip")
    } else if b.starts_with(b"7z\xbc\xaf") {
        Some("comprimido 7-Zip")
    } else if b.starts_with(b"Rar!") {
        Some("comprimido RAR")
    } else if b.starts_with(b"MZ") {
        Some("programa de MS-DOS")
    } else {
        None
    };
    if let Some(o) = other {
        return Kind::Other(o);
    }
    let sample = &b[..b.len().min(4096)];
    let text = core::str::from_utf8(sample).is_ok()
        || sample
            .iter()
            .all(|&c| c >= 0x20 || matches!(c, b'\n' | b'\r' | b'\t'));
    if b.is_empty() {
        Kind::Other("vacío")
    } else if text {
        Kind::Text
    } else {
        Kind::Other("datos binarios")
    }
}

fn parse_pe(b: &[u8]) -> Option<PeInfo> {
    let pe = u32le(b, 0x3c)? as usize;
    if b.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let machine = match u16le(b, coff)? {
        0x8664 => "x86-64",
        0x14c => "x86 (32 bits)",
        0xaa64 => "ARM64",
        0x1c0 | 0x1c4 => "ARM",
        _ => "desconocida",
    };
    let sections = u16le(b, coff + 2)?;
    let opt_size = u16le(b, coff + 16)? as usize;
    let characteristics = u16le(b, coff + 18)?;
    let opt = coff + 20;
    let magic = u16le(b, opt)?;
    let bits = if magic == 0x20b { 64 } else { 32 };
    let subsystem = u16le(b, opt + 68).unwrap_or(0);
    let dirs = opt + if bits == 64 { 112 } else { 96 };
    let dir = |i: usize| -> Option<(u32, u32)> {
        Some((u32le(b, dirs + i * 8)?, u32le(b, dirs + i * 8 + 4)?))
    };
    // Secciones, para pasar de direcciones virtuales (RVA) a posiciones en el archivo.
    let sec_table = opt + opt_size;
    let rva_to_off = |rva: u32| -> Option<usize> {
        (0..sections as usize).find_map(|i| {
            let s = sec_table + i * 40;
            let vsize = u32le(b, s + 8)?;
            let vaddr = u32le(b, s + 12)?;
            let raw_size = u32le(b, s + 16)?;
            let raw = u32le(b, s + 20)?;
            (rva >= vaddr && rva < vaddr + vsize.max(raw_size))
                .then(|| (rva - vaddr + raw) as usize)
        })
    };
    let mut imports = Vec::new();
    if let Some((rva, _)) = dir(1).filter(|d| d.0 != 0)
        && let Some(mut at) = rva_to_off(rva)
    {
        while imports.len() < 24 {
            let name_rva = u32le(b, at + 12).unwrap_or(0);
            if name_rva == 0 {
                break;
            }
            if let Some(name) = rva_to_off(name_rva).and_then(|o| cstr(b, o)) {
                imports.push(name);
            }
            at += 20;
        }
    }
    let dotnet = dir(14).is_some_and(|d| d.0 != 0);
    let head = &b[..b.len().min(4 * 1024 * 1024)];
    let installer = if contains(head, b"Inno Setup") {
        Some("Inno Setup")
    } else if contains(head, b"Nullsoft") {
        Some("NSIS (Nullsoft)")
    } else if contains(head, b"InstallShield") {
        Some("InstallShield")
    } else {
        None
    };
    Some(PeInfo {
        machine,
        bits,
        gui: subsystem == 2,
        dll: characteristics & 0x2000 != 0,
        dotnet,
        sections,
        imports,
        installer,
    })
}

fn parse_elf(b: &[u8]) -> Option<ElfInfo> {
    let bits = match b.get(4)? {
        1 => 32,
        2 => 64,
        _ => return None,
    };
    if *b.get(5)? != 1 {
        return None; // big endian: no lo miramos
    }
    let e_type = u16le(b, 16)?;
    let machine = match u16le(b, 18)? {
        0x3e => "x86-64",
        3 => "x86 (32 bits)",
        0xb7 => "ARM64 (AArch64)",
        0x28 => "ARM",
        0xf3 => "RISC-V",
        _ => "desconocida",
    };
    let (phoff, phentsize, phnum) = if bits == 64 {
        (
            u64le(b, 32)? as usize,
            u16le(b, 54)? as usize,
            u16le(b, 56)? as usize,
        )
    } else {
        (
            u32le(b, 28)? as usize,
            u16le(b, 42)? as usize,
            u16le(b, 44)? as usize,
        )
    };
    let mut interpreter = None;
    for i in 0..phnum.min(64) {
        let ph = phoff + i * phentsize;
        if u32le(b, ph)? == 3 {
            let (off, size) = if bits == 64 {
                (u64le(b, ph + 8)? as usize, u64le(b, ph + 32)? as usize)
            } else {
                (u32le(b, ph + 4)? as usize, u32le(b, ph + 16)? as usize)
            };
            interpreter = b.get(off..off + size.min(256)).map(|s| {
                String::from_utf8_lossy(s)
                    .trim_end_matches('\0')
                    .to_string()
            });
        }
    }
    Some(ElfInfo {
        machine,
        bits,
        shared: e_type == 3,
        interpreter,
    })
}

/// Una línea, como `file`.
pub fn describe(b: &[u8]) -> String {
    match detect(b) {
        Kind::Pe(p) => {
            let what = if p.dll {
                "biblioteca (DLL)"
            } else if p.installer.is_some() {
                "instalador"
            } else if p.gui {
                "programa con ventanas"
            } else {
                "programa de consola"
            };
            let mut s = format!(
                "PE{} de Windows: {what}, {}",
                if p.bits == 64 { "32+" } else { "32" },
                p.machine
            );
            if p.dotnet {
                s.push_str(", .NET");
            }
            if let Some(i) = p.installer {
                s.push_str(&format!(" ({i})"));
            }
            s
        }
        Kind::Elf(e) => format!(
            "ELF de {} bits (Linux), {}, {}{}",
            e.bits,
            e.machine,
            if e.shared {
                "ejecutable PIE o biblioteca"
            } else {
                "ejecutable"
            },
            match &e.interpreter {
                Some(i) => format!(", enlazado dinámicamente ({i})"),
                None => ", enlazado estáticamente".into(),
            }
        ),
        Kind::Script => "script de shell (texto con #!)".into(),
        Kind::Text => "texto".into(),
        Kind::Deb => "paquete de Debian/Ubuntu (.deb)".into(),
        Kind::Squashfs => "paquete snap de Linux (sistema de archivos squashfs)".into(),
        Kind::Other(o) => o.into(),
    }
}

/// Lo que se muestra al intentar ejecutar un programa que no es de JARVIS-OS.
pub fn why_not(b: &[u8]) -> Option<String> {
    let k = detect(b);
    let (what, needs): (String, &str) = match &k {
        Kind::Pe(p) => {
            let mut w = describe(b);
            if !p.imports.is_empty() {
                let list: Vec<&str> = p.imports.iter().take(8).map(|s| s.as_str()).collect();
                w.push_str(&format!(
                    "\n  usa: {}{}",
                    list.join(", "),
                    if p.imports.len() > 8 { ", ..." } else { "" }
                ));
            }
            (
                w,
                "la API de Windows (Win32: esas DLL). Es lo que hace Wine en Linux",
            )
        }
        Kind::Elf(_) => (
            describe(b),
            "las llamadas al sistema de Linux y su biblioteca de C (glibc)",
        ),
        Kind::Deb => (
            describe(b),
            "dpkg y programas ELF de Linux adentro del paquete",
        ),
        Kind::Squashfs => (
            describe(b),
            "leer squashfs y los programas ELF de Linux que trae adentro",
        ),
        _ => return None,
    };
    Some(format!(
        "{what}\nJARVIS-OS todavía no puede ejecutarlo: le falta espacio de usuario (ring 3), un\n\
         cargador de programas y {needs}. Está en el roadmap (K11). Mientras tanto: `file`,\n\
         `xxd` y `strings` para inspeccionarlo, y `apt install` para programas de JARVIS-OS."
    ))
}

/// Cadenas de texto de al menos `min` caracteres (como `strings`).
pub fn strings(b: &[u8], min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for &c in b {
        if (0x20..0x7f).contains(&c) {
            cur.push(c as char);
        } else {
            if cur.len() >= min {
                out.push(core::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= min {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un PE64 mínimo: cabecera MZ, "PE", COFF, cabecera opcional y una sección con una tabla
    /// de importación que pide KERNEL32.dll.
    pub fn tiny_exe() -> Vec<u8> {
        let mut b = alloc::vec![0u8; 0x400];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        b[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        b[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        let opt = coff + 20;
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        b[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes()); // consola
        // Directorio 1 (importaciones) → RVA 0x1000.
        b[opt + 112 + 8..opt + 112 + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        let sec = opt + 240;
        b[sec..sec + 5].copy_from_slice(b".idat");
        b[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // tamaño virtual
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // dirección virtual
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // en el archivo
        // Descriptor de importación en 0x200: nombre en RVA 0x1100 (archivo 0x300).
        b[0x200 + 12..0x200 + 16].copy_from_slice(&0x1100u32.to_le_bytes());
        b[0x300..0x30c].copy_from_slice(b"KERNEL32.dll");
        b
    }

    #[test]
    fn reconoce_un_exe_de_windows() {
        let exe = tiny_exe();
        let Kind::Pe(p) = detect(&exe) else {
            panic!("no lo reconoció")
        };
        assert_eq!(
            (p.machine, p.bits, p.gui, p.dll),
            ("x86-64", 64, false, false)
        );
        assert_eq!(p.imports, ["KERNEL32.dll"]);
        assert!(describe(&exe).starts_with("PE32+ de Windows: programa de consola"));
        assert!(why_not(&exe).unwrap().contains("Win32"));
    }

    #[test]
    fn reconoce_elf_y_otros() {
        let mut elf = alloc::vec![0u8; 128];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[16] = 2;
        elf[18] = 0x3e;
        assert!(describe(&elf).starts_with("ELF de 64 bits (Linux), x86-64, ejecutable"));
        assert_eq!(detect(b"#!/bin/jsh\necho hola"), Kind::Script);
        assert_eq!(detect("hola, qué tal".as_bytes()), Kind::Text);
        assert_eq!(describe(b"PK\x03\x04resto"), "archivo comprimido ZIP");
        assert!(why_not(b"texto").is_none());
        assert_eq!(
            strings(b"\0\0hola mundo\x01ab\x02texto", 4),
            ["hola mundo", "texto"]
        );
    }
}
