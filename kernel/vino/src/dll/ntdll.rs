//! `ntdll.dll`: la capa de abajo de Windows. Los programas casi no la llaman directo; la
//! biblioteca de Rust sí (`NtWriteFile`/`NtReadFile` para la consola y los archivos) y el CRT
//! usa las funciones de desenrollar la pila (`Rtl*Unwind*`) cuando hay una excepción.
//!
//! Las excepciones (SEH) no están: desenrollar la pila exige leer las tablas `.pdata`/`.xdata`
//! del programa e interpretarlas. Las funciones que se usan **solo** al fallar dicen que no hay
//! información (y el programa termina con su mensaje); las que se usan al lanzar una, terminan.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use super::kernel32::{read_handle, write_handle};
use crate::state;

const STATUS_SUCCESS: u32 = 0;
const STATUS_END_OF_FILE: u32 = 0xC000_0011;
const STATUS_INVALID_HANDLE: u32 = 0xC000_0008;
const STATUS_UNSUCCESSFUL: u32 = 0xC000_0001;
const STATUS_PIPE_BROKEN: u32 = 0xC000_014B;

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        NtWriteFile,
        NtReadFile,
        RtlNtStatusToDosError,
        RtlCaptureContext,
        RtlLookupFunctionEntry,
        RtlVirtualUnwind,
        RtlUnwindEx,
        RtlUnwind,
        RtlPcToFileHeader,
        RtlGetVersion,
        RtlAllocateHeap,
        RtlFreeHeap,
        RtlGetLastWin32Error,
        RtlSetLastWin32Error,
    ]
}

/// IO_STATUS_BLOCK: el estado y cuántos bytes.
unsafe fn io_status(iosb: *mut u64, status: u32, n: usize) {
    if !iosb.is_null() {
        iosb.write_unaligned(status as u64);
        iosb.add(1).write_unaligned(n as u64);
    }
}

fn status_of(e: u32) -> u32 {
    match e {
        6 => STATUS_INVALID_HANDLE,
        109 => STATUS_PIPE_BROKEN,
        _ => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "win64" fn NtWriteFile(
    h: u64,
    _event: u64,
    _apc: u64,
    _ctx: u64,
    iosb: *mut u64,
    buf: *const u8,
    len: u32,
    _off: u64,
    _key: u64,
) -> u32 {
    match write_handle(h, std::slice::from_raw_parts(buf, len as usize)) {
        Ok(n) => {
            io_status(iosb, STATUS_SUCCESS, n);
            STATUS_SUCCESS
        }
        Err(e) => status_of(e),
    }
}

unsafe extern "win64" fn NtReadFile(
    h: u64,
    _event: u64,
    _apc: u64,
    _ctx: u64,
    iosb: *mut u64,
    buf: *mut u8,
    len: u32,
    _off: u64,
    _key: u64,
) -> u32 {
    match read_handle(h, std::slice::from_raw_parts_mut(buf, len as usize)) {
        // De un archivo, 0 bytes es "fin del archivo"; de la Terminal, es Ctrl+D (y Rust lo
        // toma igual).
        Ok(0) => {
            io_status(iosb, STATUS_END_OF_FILE, 0);
            STATUS_END_OF_FILE
        }
        Ok(n) => {
            io_status(iosb, STATUS_SUCCESS, n);
            STATUS_SUCCESS
        }
        Err(e) => status_of(e),
    }
}

unsafe extern "win64" fn RtlNtStatusToDosError(status: u32) -> u32 {
    match status {
        STATUS_SUCCESS => 0,
        STATUS_END_OF_FILE => 38, // ERROR_HANDLE_EOF
        STATUS_INVALID_HANDLE => 6,
        STATUS_PIPE_BROKEN => 109, // ERROR_BROKEN_PIPE
        _ => 31,                   // ERROR_GEN_FAILURE
    }
}

unsafe extern "win64" fn RtlCaptureContext(ctx: *mut u8) {
    // CONTEXT: 1232 bytes. Vacío: alcanza para que el CRT arme su informe de error y termine.
    std::ptr::write_bytes(ctx, 0, 1232);
}

unsafe extern "win64" fn RtlLookupFunctionEntry(_pc: u64, base: *mut u64, _h: u64) -> u64 {
    if !base.is_null() {
        *base = 0;
    }
    0 // "es una función hoja": no hay nada que desenrollar
}

unsafe extern "win64" fn RtlVirtualUnwind(
    _t: u32,
    _base: u64,
    _pc: u64,
    _f: u64,
    _ctx: u64,
    data: *mut u64,
    frame: *mut u64,
    _ptrs: u64,
) -> u64 {
    if !data.is_null() {
        *data = 0;
    }
    if !frame.is_null() {
        *frame = 0;
    }
    0
}

unsafe extern "win64" fn RtlUnwindEx(
    _f: u64,
    _ip: u64,
    _rec: u64,
    _rv: u64,
    _ctx: u64,
    _h: u64,
) -> ! {
    super::fatal("el programa desenrolla la pila por una excepción y vino todavía no las maneja")
}

unsafe extern "win64" fn RtlUnwind(_f: u64, _ip: u64, _rec: u64, _rv: u64) -> ! {
    super::fatal("el programa desenrolla la pila por una excepción y vino todavía no las maneja")
}

unsafe extern "win64" fn RtlPcToFileHeader(pc: u64, base: *mut u64) -> u64 {
    let r = crate::modules::module_at(pc).map_or(0, |(b, _)| b);
    *base = r;
    r
}

unsafe extern "win64" fn RtlGetVersion(v: *mut u8) -> u32 {
    (v.add(4) as *mut u32).write_unaligned(10);
    (v.add(8) as *mut u32).write_unaligned(0);
    (v.add(12) as *mut u32).write_unaligned(19045);
    (v.add(16) as *mut u32).write_unaligned(2);
    STATUS_SUCCESS
}

unsafe extern "win64" fn RtlAllocateHeap(_h: u64, flags: u32, size: usize) -> *mut u8 {
    state::heap_alloc(size, flags & 0x8 != 0)
}

unsafe extern "win64" fn RtlFreeHeap(_h: u64, _flags: u32, p: *mut u8) -> u8 {
    state::heap_free(p);
    1
}

unsafe extern "win64" fn RtlGetLastWin32Error() -> u32 {
    state::last_error()
}

unsafe extern "win64" fn RtlSetLastWin32Error(e: u32) {
    state::set_last_error(e)
}
