//! Lo que un programa de Windows espera encontrar en memoria: el TEB, el PEB y los parámetros
//! del proceso.
//!
//! - El **TEB** (Thread Environment Block) es del hilo. En x86-64 Windows deja su dirección en
//!   la base del registro GS, y el código compilado lo lee directo: `gs:[0x30]` (el TEB mismo),
//!   `gs:[0x58]` (el TLS), `gs:[0x60]` (el PEB), `gs:[0x10]` (el límite de la pila, que mira
//!   `__chkstk`). Linux no usa GS en el espacio de usuario, así que `vino` lo pide con
//!   `arch_prctl(ARCH_SET_GS)` y queda para el programa.
//! - El **PEB** (Process Environment Block) es del proceso: la base de la imagen, el heap y los
//!   parámetros (línea de comandos, ruta del programa, entorno).
//!
//! Offsets de Windows 10/11 x64 (los mismos que usa Wine: include/winternl.h).

use crate::win::to_wide_nul;

pub mod off {
    // TEB
    pub const TEB_EXCEPTION_LIST: usize = 0x00;
    pub const TEB_STACK_BASE: usize = 0x08;
    pub const TEB_STACK_LIMIT: usize = 0x10;
    pub const TEB_SELF: usize = 0x30;
    pub const TEB_PROCESS_ID: usize = 0x40;
    pub const TEB_THREAD_ID: usize = 0x48;
    pub const TEB_TLS_POINTER: usize = 0x58;
    pub const TEB_PEB: usize = 0x60;
    pub const TEB_LAST_ERROR: usize = 0x68;
    /// `TlsSlots[64]`: lo que guardan `TlsSetValue`/`TlsGetValue`.
    pub const TEB_TLS_SLOTS: usize = 0x1480;
    pub const TEB_SIZE: usize = 0x1838;
    // PEB
    pub const PEB_BEING_DEBUGGED: usize = 0x02;
    pub const PEB_IMAGE_BASE: usize = 0x10;
    pub const PEB_PARAMETERS: usize = 0x20;
    pub const PEB_PROCESS_HEAP: usize = 0x30;
    pub const PEB_OS_MAJOR: usize = 0x118;
    pub const PEB_OS_MINOR: usize = 0x11c;
    pub const PEB_OS_BUILD: usize = 0x120;
    pub const PEB_OS_PLATFORM: usize = 0x124;
    pub const PEB_SIZE: usize = 0x7d0;
    // RTL_USER_PROCESS_PARAMETERS
    pub const PP_STD_INPUT: usize = 0x20;
    pub const PP_STD_OUTPUT: usize = 0x28;
    pub const PP_STD_ERROR: usize = 0x30;
    pub const PP_CURRENT_DIR: usize = 0x38;
    pub const PP_IMAGE_PATH: usize = 0x60;
    pub const PP_COMMAND_LINE: usize = 0x70;
    pub const PP_ENVIRONMENT: usize = 0x80;
    pub const PP_SIZE: usize = 0x440;
}

/// Lo que va en los bloques.
pub struct Info<'a> {
    pub image_base: u64,
    pub stack_base: u64,
    pub stack_limit: u64,
    pub pid: u64,
    pub tid: u64,
    /// La dirección del arreglo de bloques de TLS (uno por módulo con `__declspec(thread)`).
    pub tls_array: u64,
    pub heap: u64,
    pub std_handles: [u64; 3],
    pub image_path: &'a str,
    pub command_line: &'a str,
    pub current_dir: &'a str,
    /// El entorno en UTF-16: `NOMBRE=valor\0…\0\0`.
    pub environment: u64,
}

/// Dónde quedó cada cosa adentro del bloque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    pub teb: u64,
    pub peb: u64,
    pub params: u64,
}

const TEB_AT: usize = 0;
const PEB_AT: usize = 0x2000;
const PARAMS_AT: usize = 0x3000;
const STRINGS_AT: usize = 0x3800;
/// El tamaño del bloque: TEB, PEB, parámetros y sus textos.
pub const BLOCK_SIZE: usize = 0x10000;

fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Llena `block` (de [`BLOCK_SIZE`] bytes, en cero, que está en la dirección `at`).
pub fn fill(block: &mut [u8], at: u64, info: &Info<'_>) -> Addrs {
    use off::*;
    let a = Addrs {
        teb: at + TEB_AT as u64,
        peb: at + PEB_AT as u64,
        params: at + PARAMS_AT as u64,
    };
    // Los textos UTF-16 (`UNICODE_STRING`: largo en bytes sin el cero, máximo y puntero).
    let mut next = STRINGS_AT;
    let mut unicode = |block: &mut [u8], field: usize, s: &str| {
        let w = to_wide_nul(s);
        let bytes = w.len() * 2;
        if next + bytes > block.len() {
            return;
        }
        for (i, c) in w.iter().enumerate() {
            block[next + i * 2..next + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        let len = ((bytes - 2) as u32).min(0xfffe);
        put32(block, field, len | (len + 2).min(0xffff) << 16);
        put64(block, field + 8, at + next as u64);
        next = (next + bytes + 7) & !7;
    };
    let p = PARAMS_AT;
    put32(block, p, PP_SIZE as u32);
    put32(block, p + 4, PP_SIZE as u32);
    put32(block, p + 8, 1); // PROCESS_PARAMS_FLAG_NORMALIZED: los punteros son direcciones
    put64(block, p + PP_STD_INPUT, info.std_handles[0]);
    put64(block, p + PP_STD_OUTPUT, info.std_handles[1]);
    put64(block, p + PP_STD_ERROR, info.std_handles[2]);
    unicode(block, p + PP_CURRENT_DIR, info.current_dir);
    unicode(block, p + PP_IMAGE_PATH, info.image_path);
    unicode(block, p + PP_COMMAND_LINE, info.command_line);
    put64(block, p + PP_ENVIRONMENT, info.environment);

    let e = PEB_AT;
    block[e + PEB_BEING_DEBUGGED] = 0;
    put64(block, e + PEB_IMAGE_BASE, info.image_base);
    put64(block, e + PEB_PARAMETERS, a.params);
    put64(block, e + PEB_PROCESS_HEAP, info.heap);
    // Windows 10 (10.0.19045): lo que miran los programas que preguntan la versión.
    put32(block, e + PEB_OS_MAJOR, 10);
    put32(block, e + PEB_OS_MINOR, 0);
    block[e + PEB_OS_BUILD..e + PEB_OS_BUILD + 2].copy_from_slice(&19045u16.to_le_bytes());
    put32(block, e + PEB_OS_PLATFORM, 2); // VER_PLATFORM_WIN32_NT

    let t = TEB_AT;
    // Sin manejadores de excepciones de 32 bits: el final de la lista es -1.
    put64(block, t + TEB_EXCEPTION_LIST, u64::MAX);
    put64(block, t + TEB_STACK_BASE, info.stack_base);
    put64(block, t + TEB_STACK_LIMIT, info.stack_limit);
    put64(block, t + TEB_SELF, a.teb);
    put64(block, t + TEB_PROCESS_ID, info.pid);
    put64(block, t + TEB_THREAD_ID, info.tid);
    put64(block, t + TEB_TLS_POINTER, info.tls_array);
    put64(block, t + TEB_PEB, a.peb);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get64(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }

    #[test]
    fn teb_peb_y_parametros_con_los_offsets_de_windows() {
        let mut b = vec![0u8; BLOCK_SIZE];
        let at = 0x7000_0000;
        let a = fill(
            &mut b,
            at,
            &Info {
                image_base: 0x1_4000_0000,
                stack_base: 0x9000_0000,
                stack_limit: 0x8000_0000,
                pid: 7,
                tid: 7,
                tls_array: 0x5000,
                heap: 0x4000,
                std_handles: [0x10, 0x14, 0x18],
                image_path: r"C:\x.exe",
                command_line: "x.exe uno",
                current_dir: r"C:\",
                environment: 0x6000,
            },
        );
        let t = (a.teb - at) as usize;
        assert_eq!(get64(&b, t + off::TEB_SELF), a.teb);
        assert_eq!(get64(&b, t + off::TEB_PEB), a.peb);
        assert_eq!(get64(&b, t + off::TEB_STACK_LIMIT), 0x8000_0000);
        assert_eq!(get64(&b, t + off::TEB_TLS_POINTER), 0x5000);
        let e = (a.peb - at) as usize;
        assert_eq!(get64(&b, e + off::PEB_IMAGE_BASE), 0x1_4000_0000);
        assert_eq!(get64(&b, e + off::PEB_PARAMETERS), a.params);
        let p = (a.params - at) as usize;
        // CommandLine: "x.exe uno" son 9 caracteres = 18 bytes.
        assert_eq!(u16::from_le_bytes([b[p + 0x70], b[p + 0x71]]), 18);
        let s = (get64(&b, p + off::PP_COMMAND_LINE + 8) - at) as usize;
        let w: Vec<u16> = (0..9)
            .map(|i| u16::from_le_bytes([b[s + i * 2], b[s + i * 2 + 1]]))
            .collect();
        assert_eq!(String::from_utf16(&w).unwrap(), "x.exe uno");
        assert_eq!(get64(&b, p + off::PP_STD_OUTPUT), 0x14);
    }
}
