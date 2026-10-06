//! fw_cfg de QEMU (y de VirtualBox 7): archivos que el anfitrión le pasa a la máquina
//! (`-fw_cfg name=...,string=...`) y la línea de comandos.
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
const CMDLINE_SIZE: u16 = 0x0014;
const CMDLINE_DATA: u16 = 0x0015;

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

/// La línea de comandos que el anfitrión le pasa al "kernel" (elementos fijos 0x14 = largo y
/// 0x15 = texto). VirtualBox 7 la ofrece en su dispositivo `qemu-fw-cfg`
/// (`VBoxInternal/Devices/qemu-fw-cfg/0/Config/CmdLine`), aunque no tenga archivos con nombre.
pub fn cmdline() -> Option<String> {
    if !present() {
        return None;
    }
    select(CMDLINE_SIZE);
    let mut n = [0u8; 4];
    read(&mut n);
    let size = u32::from_le_bytes(n) as usize;
    if size == 0 || size > 4096 {
        return None;
    }
    let mut data = alloc::vec![0u8; size];
    select(CMDLINE_DATA);
    read(&mut data);
    let end = data.iter().position(|&b| b == 0).unwrap_or(size);
    String::from_utf8(data[..end].to_vec()).ok()
}

/// ¿Hay fw_cfg? (La firma es "QEMU".)
fn present() -> bool {
    select(SIGNATURE);
    let mut sig = [0u8; 4];
    read(&mut sig);
    &sig == b"QEMU"
}

/// El contenido de un archivo de fw_cfg, si existe.
pub fn file(name: &str) -> Option<Vec<u8>> {
    if !present() {
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
/// En VirtualBox (que no tiene archivos con nombre) viene en la línea de comandos de fw_cfg:
/// `jarvis.cerebro=puerto,token` (lo pone `cargo xtask vbox`).
pub fn brain() -> Option<(u16, String)> {
    let s = match file("opt/jarvis/cerebro") {
        Some(f) => String::from_utf8(f).ok()?,
        None => cmdline()?
            .split_whitespace()
            .find_map(|w| w.strip_prefix("jarvis.cerebro="))?
            .replacen(',', " ", 1),
    };
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
