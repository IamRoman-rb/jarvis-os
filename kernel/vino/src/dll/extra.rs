//! DLL chicas: el azar (`bcryptprimitives`, `bcrypt`, `advapi32`) y `MessageBox` (`user32`).
//!
//! El azar sale de `getrandom` (una llamada de Linux que JARVIS-OS saca de su generador
//! ChaCha20, K10). `MessageBox` no tiene ventanas todavía: el mensaje va a la Terminal y se
//! responde "Aceptar".

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use super::kernel32::write_console;
use crate::os;
use crate::win::{cstr, wstr};

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn bcrypt_exports() -> Vec<(&'static str, usize)> {
    exports![ProcessPrng, BCryptGenRandom]
}

pub fn advapi32_exports() -> Vec<(&'static str, usize)> {
    exports![
        SystemFunction036,
        GetUserNameW,
        GetUserNameA,
        RegOpenKeyExW,
        RegCloseKey
    ]
}

pub fn user32_exports() -> Vec<(&'static str, usize)> {
    exports![MessageBoxA, MessageBoxW]
}

unsafe fn fill_random(buf: *mut u8, len: usize) -> bool {
    os::random(std::slice::from_raw_parts_mut(buf, len))
}

/// Lo que usa la biblioteca de Rust (y `rand`) en Windows 10+.
unsafe extern "win64" fn ProcessPrng(buf: *mut u8, len: usize) -> i32 {
    i32::from(fill_random(buf, len))
}

unsafe extern "win64" fn BCryptGenRandom(_alg: u64, buf: *mut u8, len: u32, _flags: u32) -> u32 {
    if fill_random(buf, len as usize) {
        0
    } else {
        0xC000_0001
    }
}

/// `RtlGenRandom`, el nombre de siempre.
unsafe extern "win64" fn SystemFunction036(buf: *mut u8, len: u32) -> u8 {
    u8::from(fill_random(buf, len as usize))
}

fn user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "usuario".into())
}

unsafe extern "win64" fn GetUserNameW(buf: *mut u16, len: *mut u32) -> i32 {
    let r = crate::win::put_wide(buf, *len, &user());
    let ok = r < *len;
    *len = if ok { r + 1 } else { r };
    i32::from(ok)
}

unsafe extern "win64" fn GetUserNameA(buf: *mut u8, len: *mut u32) -> i32 {
    let r = crate::win::put_narrow(buf, *len, &user());
    let ok = r < *len;
    *len = if ok { r + 1 } else { r };
    i32::from(ok)
}

/// No hay registro de Windows: toda clave "no existe" (ERROR_FILE_NOT_FOUND).
unsafe extern "win64" fn RegOpenKeyExW(_k: u64, _s: u64, _o: u32, _sam: u32, out: *mut u64) -> u32 {
    if !out.is_null() {
        *out = 0;
    }
    2
}

unsafe extern "win64" fn RegCloseKey(_k: u64) -> u32 {
    0
}

fn message_box(caption: &str, text: &str) -> i32 {
    let title = if caption.is_empty() { "Error" } else { caption };
    let _ = write_console(1, format!("[{title}] {text}\n").as_bytes());
    1 // IDOK
}

unsafe extern "win64" fn MessageBoxA(_w: u64, text: *const u8, caption: *const u8, _t: u32) -> i32 {
    message_box(
        &String::from_utf8_lossy(cstr(caption)),
        &String::from_utf8_lossy(cstr(text)),
    )
}

unsafe extern "win64" fn MessageBoxW(
    _w: u64,
    text: *const u16,
    caption: *const u16,
    _t: u32,
) -> i32 {
    message_box(&wstr(caption), &wstr(text))
}
