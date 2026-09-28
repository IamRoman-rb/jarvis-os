//! fw_cfg de QEMU: archivos que el anfitrión le pasa a la máquina (`-fw_cfg name=...,string=...`).
//! Se leen por dos puertos de E/S: 0x510 elige el elemento y 0x511 entrega sus bytes.
//!
//! Lo usa `xtask` para decirle al kernel la resolución del monitor del anfitrión: la que informa
//! la placa de video es la de la ventana de QEMU al arrancar (640×480).

use alloc::string::String;
use alloc::vec::Vec;
use x86_64::instructions::port::Port;

const SELECTOR: u16 = 0x510;
const DATA: u16 = 0x511;
const SIGNATURE: u16 = 0x0000;
const FILE_DIR: u16 = 0x0019;

fn select(key: u16) {
    // SAFETY: 0x510 es el selector de fw_cfg en QEMU (x86); en una PC sin fw_cfg el puerto no
    // hace nada y la firma no coincide.
    unsafe { Port::<u16>::new(SELECTOR).write(key) }
}

fn read(buf: &mut [u8]) {
    let mut port = Port::<u8>::new(DATA);
    for b in buf {
        // SAFETY: 0x511 es el puerto de datos de fw_cfg; leer avanza dentro del elemento elegido.
        *b = unsafe { port.read() };
    }
}

/// El contenido de un archivo de fw_cfg, si existe.
pub fn file(name: &str) -> Option<Vec<u8>> {
    select(SIGNATURE);
    let mut sig = [0u8; 4];
    read(&mut sig);
    if &sig != b"QEMU" {
        return None;
    }
    select(FILE_DIR);
    let mut n = [0u8; 4];
    read(&mut n);
    for _ in 0..u32::from_be_bytes(n).min(4096) {
        let mut e = [0u8; 64];
        read(&mut e);
        let size = u32::from_be_bytes([e[0], e[1], e[2], e[3]]) as usize;
        let key = u16::from_be_bytes([e[4], e[5]]);
        let end = e[8..].iter().position(|&b| b == 0).unwrap_or(56);
        if &e[8..8 + end] == name.as_bytes() {
            let mut data = alloc::vec![0u8; size.min(4096)];
            select(key);
            read(&mut data);
            return Some(data);
        }
    }
    None
}

/// `opt/jarvis/cerebro` = "puerto token": dónde está el cerebro en el anfitrión y el token de
/// esta sesión (lo genera `cargo xtask run`).
pub fn brain() -> Option<(u16, String)> {
    let s = String::from_utf8(file("opt/jarvis/cerebro")?).ok()?;
    let (port, token) = s.trim().split_once(' ')?;
    let token = token.trim();
    (token.len() >= 16 && token.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| Some((port.parse().ok()?, token.into())))?
}

/// `opt/jarvis/resolucion` = "ANCHOxALTO".
pub fn resolution() -> Option<(u32, u32)> {
    let s = String::from_utf8(file("opt/jarvis/resolucion")?).ok()?;
    let (w, h) = s.trim().split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    ((640..=7680).contains(&w) && (480..=4320).contains(&h)).then_some((w, h))
}
