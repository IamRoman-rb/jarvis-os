//! Poner un `.exe` en memoria, sobre un buffer: lo que hace el cargador de Windows antes de
//! ejecutar la primera instrucción del programa.
//!
//! 1. **Secciones**: el archivo y la memoria no tienen la misma forma. Cada sección dice dónde
//!    va (`rva`, relativa a la base) y qué parte del archivo se copia; lo que sobra queda en cero
//!    (las variables globales sin valor inicial, `.bss`).
//! 2. **Relocalizaciones**: el compilador armó el programa para su `ImageBase` (0x140000000 en
//!    los de 64 bits). Si va en otra dirección, cada dirección absoluta guardada en el programa
//!    (punteros a funciones, a textos) se corre la misma distancia. La tabla `.reloc` dice dónde
//!    están.
//! 3. **Imports**: la IAT (tabla de direcciones de imports) tiene un lugar por cada función que el
//!    programa usa de una DLL. El código llama "a través" de ese lugar (`call [rip+…]`), así que
//!    alcanza con escribir ahí la dirección de la función nuestra.
//!
//! Todo trabaja sobre `&mut [u8]` (la imagen entera, de `size_of_image` bytes): en `vino` es la
//! memoria que dio `mmap`; en los tests, un `Vec`.

use crate::pe::{Image, LoadError, Symbol};

/// Copia los encabezados y las secciones del archivo a `mem` (que empieza en cero).
pub fn copy_sections(file: &[u8], img: &Image, mem: &mut [u8]) -> Result<(), LoadError> {
    if mem.len() < img.size_of_image as usize {
        return Err(LoadError::Broken("memoria chica para la imagen"));
    }
    let h = (img.size_of_headers as usize)
        .min(file.len())
        .min(mem.len());
    mem[..h].copy_from_slice(&file[..h]);
    for s in &img.sections {
        let n = s.raw_size.min(s.size) as usize;
        if n == 0 {
            continue;
        }
        let from = s.raw_offset as usize;
        let to = s.rva as usize;
        let src = file
            .get(from..from + n)
            .ok_or(LoadError::Broken("una sección se sale del archivo"))?;
        mem.get_mut(to..to + n)
            .ok_or(LoadError::Broken("una sección se sale de la imagen"))?
            .copy_from_slice(src);
    }
    Ok(())
}

/// Corrige las direcciones absolutas si la imagen quedó en `base` y no en su `ImageBase`.
pub fn relocate(img: &Image, mem: &mut [u8], base: u64) -> Result<(), LoadError> {
    let delta = base.wrapping_sub(img.image_base);
    if delta == 0 {
        return Ok(());
    }
    for &rva in &img.relocs {
        let at = rva as usize;
        let slot = mem
            .get_mut(at..at + 8)
            .ok_or(LoadError::Broken("una relocalización se sale de la imagen"))?;
        let v = u64::from_le_bytes(slot.try_into().unwrap_or_default());
        slot.copy_from_slice(&v.wrapping_add(delta).to_le_bytes());
    }
    Ok(())
}

/// Llena la IAT: para cada import, la dirección que devuelve `resolve(dll, símbolo)`.
pub fn bind(
    img: &Image,
    mem: &mut [u8],
    resolve: &mut dyn FnMut(&str, &Symbol) -> u64,
) -> Result<(), LoadError> {
    for imp in &img.imports {
        for (rva, sym) in &imp.entries {
            let at = *rva as usize;
            let addr = resolve(&imp.dll, sym);
            mem.get_mut(at..at + 8)
                .ok_or(LoadError::Broken(
                    "la tabla de imports se sale de la imagen",
                ))?
                .copy_from_slice(&addr.to_le_bytes());
        }
    }
    Ok(())
}

/// Las variables `__declspec(thread)` (`thread_local!` en Rust) de un programa.
#[derive(Debug, PartialEq, Eq)]
pub struct TlsInfo {
    /// El molde con el que arranca el bloque de cada hilo (los datos más los ceros del final).
    pub template: Vec<u8>,
    /// RVA de la variable `_tls_index` (ahí va el índice del bloque: 0, el único).
    pub index_rva: u32,
    /// Las funciones a llamar al arrancar (y al terminar), ya relocalizadas.
    pub callbacks: Vec<u64>,
}

/// Lee el TLS de la imagen **ya relocalizada** en `mem`.
pub fn tls(img: &Image, mem: &[u8]) -> Result<Option<TlsInfo>, LoadError> {
    let Some(t) = &img.tls else {
        return Ok(None);
    };
    let broken = LoadError::Broken("TLS");
    // Las direcciones del directorio son absolutas y con la base preferida (el directorio en sí
    // está en `.rdata`, que también se relocalizó, pero el parser lo leyó del archivo).
    let rva = |va: u64| -> Option<usize> {
        let r = va.checked_sub(img.image_base)?;
        (r < img.size_of_image as u64).then_some(r as usize)
    };
    let start = rva(t.start).ok_or(broken.clone())?;
    let end = rva(t.end).ok_or(broken.clone())?;
    if end < start {
        return Err(broken);
    }
    let mut template = mem.get(start..end).ok_or(broken.clone())?.to_vec();
    template.resize(template.len() + t.zero_fill as usize, 0);
    let index_rva = rva(t.index_addr).ok_or(broken.clone())? as u32;
    let mut callbacks = Vec::new();
    if t.callbacks != 0 {
        let mut at = rva(t.callbacks).ok_or(broken.clone())?;
        loop {
            let v = mem
                .get(at..at + 8)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()))
                .ok_or(broken.clone())?;
            if v == 0 || callbacks.len() > 64 {
                break;
            }
            // La lista está en la imagen: sus valores ya tienen la base nueva.
            callbacks.push(v);
            at += 8;
        }
    }
    Ok(Some(TlsInfo {
        template,
        index_rva,
        callbacks,
    }))
}

/// Los permisos de cada parte de la imagen, por páginas: `(rva, tamaño, prot)`. Los
/// encabezados, de solo lectura.
pub fn protections(img: &Image) -> Vec<(u32, u32, u32)> {
    let mut out = vec![(0, img.size_of_headers.max(1), crate::pe::prot::READ)];
    for s in &img.sections {
        if s.size > 0 {
            out.push((s.rva, s.size, s.prot));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe;

    #[test]
    fn carga_relocaliza_y_resuelve_imports() {
        let file = pe::tests::tiny();
        let mut img = pe::parse(&file).unwrap();
        // Una relocalización inventada: el valor de 64 bits en la RVA 0x2100 (adentro de .idata).
        img.relocs.push(0x2100);
        let mut mem = vec![0u8; img.size_of_image as usize];
        copy_sections(&file, &img, &mut mem).unwrap();
        assert_eq!(&mem[..2], b"MZ");
        assert_eq!(mem[0x1000], 0xc3, "el código en su RVA");
        mem[0x2100..0x2108].copy_from_slice(&0x1_4000_1000u64.to_le_bytes());
        relocate(&img, &mut mem, 0x7000_0000).unwrap();
        assert_eq!(
            u64::from_le_bytes(mem[0x2100..0x2108].try_into().unwrap()),
            0x7000_1000
        );
        let mut asked = Vec::new();
        bind(&img, &mut mem, &mut |dll, sym| {
            asked.push(format!("{dll}!{sym:?}"));
            0xAA00 + asked.len() as u64
        })
        .unwrap();
        assert_eq!(asked.len(), 3);
        assert!(asked[0].contains("kernel32.dll") && asked[0].contains("ExitProcess"));
        assert_eq!(
            u64::from_le_bytes(mem[0x20c0..0x20c8].try_into().unwrap()),
            0xAA01
        );
        assert_eq!(
            u64::from_le_bytes(mem[0x20e0..0x20e8].try_into().unwrap()),
            0xAA03
        );
        let p = protections(&img);
        assert_eq!(p[1], (0x1000, 0x10, pe::prot::READ | pe::prot::EXEC));
    }

    #[test]
    fn el_tls_del_programa() {
        let file = pe::tests::tiny();
        let mut img = pe::parse(&file).unwrap();
        let base = img.image_base;
        img.tls = Some(pe::Tls {
            start: base + 0x2180,
            end: base + 0x2184,
            index_addr: base + 0x2190,
            callbacks: base + 0x21a0,
            zero_fill: 4,
        });
        let mut mem = vec![0u8; img.size_of_image as usize];
        mem[0x2180..0x2184].copy_from_slice(b"jarv");
        mem[0x21a0..0x21a8].copy_from_slice(&(base + 0x1000).to_le_bytes());
        let t = tls(&img, &mem).unwrap().unwrap();
        assert_eq!(t.template, b"jarv\0\0\0\0");
        assert_eq!(t.index_rva, 0x2190);
        assert_eq!(t.callbacks, vec![base + 0x1000]);
    }
}
