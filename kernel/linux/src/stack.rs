//! La pila con la que arranca un programa de Linux.
//!
//! Un programa no recibe `argv` como parámetro de una función: al arrancar, `rsp` apunta a
//!
//! ```text
//! rsp →  argc
//!        argv[0] … argv[argc-1], NULL
//!        envp[0] … , NULL
//!        vector auxiliar: pares (tipo, valor) … , (AT_NULL, 0)
//!        … relleno …
//!        los textos de argv y envp, "x86_64", 16 bytes al azar      ← arriba de todo
//! ```
//!
//! El vector auxiliar le cuenta al programa cosas del sistema: el tamaño de página, dónde quedó
//! su tabla de program headers (musl la usa para reubicarse y para el TLS), 16 bytes al azar
//! (para el "canario" de la pila y las tablas hash) y quién es el usuario. `rsp` tiene que quedar
//! alineado a 16 bytes al arrancar (la ABI lo exige; `movaps` falla si no).

use alloc::vec::Vec;

use crate::abi::at;

/// Arma la pila. `top` es la dirección más alta (exclusiva). Devuelve el `rsp` inicial y los
/// bytes que van en `[rsp, top)`.
pub fn build(
    top: u64,
    argv: &[&[u8]],
    envp: &[&[u8]],
    aux: &[(u64, u64)],
    random: [u8; 16],
) -> (u64, Vec<u8>) {
    // Primero los textos, desde arriba: se escriben en un buffer que después va al final.
    let mut strings: Vec<u8> = Vec::new();
    let mut place = |s: &[u8]| -> u64 {
        strings.extend_from_slice(s);
        strings.push(0);
        strings.len() as u64 // desplazamiento del final; se convierte a dirección más abajo
    };
    let argv_ends: Vec<u64> = argv.iter().map(|a| place(a)).collect();
    let envp_ends: Vec<u64> = envp.iter().map(|e| place(e)).collect();
    let platform_end = place(b"x86_64");
    strings.extend_from_slice(&random);
    let total = strings.len() as u64;
    // Una dirección a partir del desplazamiento del final de cada texto y su largo.
    let strings_base = top - total;
    let addr = |end: u64, len: usize| strings_base + end - len as u64 - 1;
    let argv_ptrs: Vec<u64> = argv_ends
        .iter()
        .zip(argv)
        .map(|(&e, a)| addr(e, a.len()))
        .collect();
    let envp_ptrs: Vec<u64> = envp_ends
        .iter()
        .zip(envp)
        .map(|(&e, s)| addr(e, s.len()))
        .collect();
    let platform = addr(platform_end, 6);
    let random_addr = top - 16;
    let execfn = argv_ptrs.first().copied().unwrap_or(platform);

    let mut words: Vec<u64> = Vec::new();
    words.push(argv.len() as u64);
    words.extend(&argv_ptrs);
    words.push(0);
    words.extend(&envp_ptrs);
    words.push(0);
    for &(k, v) in aux {
        words.push(k);
        words.push(v);
    }
    for (k, v) in [
        (at::RANDOM, random_addr),
        (at::PLATFORM, platform),
        (at::EXECFN, execfn),
        (at::NULL, 0),
    ] {
        words.push(k);
        words.push(v);
    }
    // rsp: debajo de los textos, alineado a 16, con lugar para todas las palabras.
    let below = (strings_base & !15) - words.len() as u64 * 8;
    let rsp = below & !15;
    let mut image = alloc::vec![0u8; (top - rsp) as usize];
    for (i, w) in words.iter().enumerate() {
        image[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    let s = (strings_base - rsp) as usize;
    image[s..].copy_from_slice(&strings);
    (rsp, image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(img: &[u8], rsp: u64, addr: u64) -> u64 {
        let o = (addr - rsp) as usize;
        u64::from_le_bytes(img[o..o + 8].try_into().unwrap())
    }

    fn cstr(img: &[u8], rsp: u64, addr: u64) -> &[u8] {
        let o = (addr - rsp) as usize;
        let end = img[o..].iter().position(|&b| b == 0).unwrap();
        &img[o..o + end]
    }

    #[test]
    fn argc_argv_envp_y_auxiliar() {
        let top = 0x7F_FFFF_F000;
        let (rsp, img) = build(
            top,
            &[b"/bin/hola", b"-v"],
            &[b"HOME=/", b"USER=roman"],
            &[(at::PAGESZ, 4096)],
            [7; 16],
        );
        assert_eq!(rsp % 16, 0, "la ABI pide rsp alineado a 16");
        assert_eq!(rsp + img.len() as u64, top);
        assert_eq!(word(&img, rsp, rsp), 2, "argc");
        assert_eq!(cstr(&img, rsp, word(&img, rsp, rsp + 8)), b"/bin/hola");
        assert_eq!(cstr(&img, rsp, word(&img, rsp, rsp + 16)), b"-v");
        assert_eq!(word(&img, rsp, rsp + 24), 0);
        assert_eq!(cstr(&img, rsp, word(&img, rsp, rsp + 32)), b"HOME=/");
        assert_eq!(cstr(&img, rsp, word(&img, rsp, rsp + 40)), b"USER=roman");
        assert_eq!(word(&img, rsp, rsp + 48), 0);
        // El vector auxiliar: el que pasamos y los que agrega (AT_RANDOM apunta a los 16 bytes).
        let mut a = rsp + 56;
        let mut aux = Vec::new();
        loop {
            let (k, v) = (word(&img, rsp, a), word(&img, rsp, a + 8));
            aux.push((k, v));
            a += 16;
            if k == at::NULL {
                break;
            }
        }
        assert_eq!(aux[0], (at::PAGESZ, 4096));
        let random = aux.iter().find(|p| p.0 == at::RANDOM).unwrap().1;
        assert_eq!(&img[(random - rsp) as usize..], &[7; 16]);
        let platform = aux.iter().find(|p| p.0 == at::PLATFORM).unwrap().1;
        assert_eq!(cstr(&img, rsp, platform), b"x86_64");
        assert!(a <= top - 16);
    }

    #[test]
    fn sin_argumentos() {
        let (rsp, img) = build(0x10_0000, &[], &[], &[], [0; 16]);
        assert_eq!(rsp % 16, 0);
        assert_eq!(word(&img, rsp, rsp), 0);
    }
}
