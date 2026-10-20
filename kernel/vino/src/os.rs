//! Las pocas llamadas de Linux que la biblioteca estándar de Rust no ofrece: `mmap`, `mprotect`,
//! `munmap`, `arch_prctl` y `getrandom`. Las da musl (la libc con la que se enlaza `vino`).
//!
//! En el anfitrión (los tests) no existen: devuelven error, y lo que depende de ellas no se
//! prueba ahí sino en `cargo xtask test`.

pub const PROT_NONE: u32 = 0;

#[cfg(target_os = "linux")]
mod imp {
    use core::ffi::{c_int, c_long, c_void};

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
        fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
        fn syscall(n: c_long, ...) -> c_long;
    }

    const MAP_PRIVATE: c_int = 0x02;
    const MAP_ANONYMOUS: c_int = 0x20;
    const MAP_FIXED_NOREPLACE: c_int = 0x10_0000;
    const MAP_FAILED: usize = usize::MAX;
    const SYS_ARCH_PRCTL: c_long = 158;
    const SYS_GETRANDOM: c_long = 318;
    const ARCH_SET_GS: c_long = 0x1001;

    pub fn map(addr: u64, len: usize, prot: u32, fixed: bool) -> Option<u64> {
        let flags = MAP_PRIVATE | MAP_ANONYMOUS | if fixed { MAP_FIXED_NOREPLACE } else { 0 };
        // SAFETY: memoria anónima nueva; con MAP_FIXED_NOREPLACE el kernel no pisa nada mapeado.
        let p = unsafe { mmap(addr as *mut c_void, len, prot as c_int, flags, -1, 0) };
        (p as usize != MAP_FAILED && (!fixed || p as u64 == addr)).then_some(p as u64)
    }

    pub fn protect(addr: u64, len: usize, prot: u32) -> bool {
        // SAFETY: cambia permisos de memoria que mapeó `vino` (o que el programa pidió con
        // VirtualProtect: si no es suya, el kernel devuelve un error y no pasa nada).
        unsafe { mprotect(addr as *mut c_void, len, prot as c_int) == 0 }
    }

    pub fn unmap(addr: u64, len: usize) -> bool {
        // SAFETY: lo pidió el programa (VirtualFree) sobre memoria que le dio VirtualAlloc.
        unsafe { munmap(addr as *mut c_void, len) == 0 }
    }

    pub fn set_gs(base: u64) -> bool {
        // SAFETY: arch_prctl(ARCH_SET_GS) solo cambia la base de GS de este proceso; ni Rust ni
        // musl usan GS (el TLS de musl va en FS).
        unsafe { syscall(SYS_ARCH_PRCTL, ARCH_SET_GS, base as c_long) == 0 }
    }

    /// El número de este hilo para el kernel (`gettid`).
    pub fn gettid() -> u32 {
        // SAFETY: gettid no tiene argumentos ni efectos.
        unsafe { syscall(186) as u32 }
    }

    /// Termina el hilo actual (no el proceso) sin volver: `pthread_exit` de musl.
    pub fn thread_exit() -> ! {
        unsafe extern "C" {
            fn pthread_exit(value: *mut c_void) -> !;
        }
        // SAFETY: lo llama un hilo creado con `std::thread` (un pthread de musl), que no
        // vuelve a usar nada de su pila.
        unsafe { pthread_exit(core::ptr::null_mut()) }
    }

    pub fn random(buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            // Por `syscall` y no por la función `getrandom` de musl: en el musl que trae Rust esa
            // función es un símbolo débil que puede quedar sin resolver (en 0).
            let rest = &mut buf[done..];
            // SAFETY: getrandom escribe a lo sumo `rest.len()` bytes adentro de `rest`.
            let n = unsafe {
                syscall(
                    SYS_GETRANDOM,
                    rest.as_mut_ptr() as c_long,
                    rest.len() as c_long,
                    0 as c_long,
                )
            };
            if n <= 0 {
                return false;
            }
            done += n as usize;
        }
        true
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub fn map(_addr: u64, _len: usize, _prot: u32, _fixed: bool) -> Option<u64> {
        None
    }
    pub fn protect(_addr: u64, _len: usize, _prot: u32) -> bool {
        false
    }
    pub fn unmap(_addr: u64, _len: usize) -> bool {
        false
    }
    pub fn set_gs(_base: u64) -> bool {
        false
    }
    pub fn gettid() -> u32 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(1000);
        thread_local! {
            static ID: u32 = NEXT.fetch_add(1, Ordering::Relaxed);
        }
        ID.with(|i| *i)
    }
    pub fn thread_exit() -> ! {
        loop {
            std::thread::park();
        }
    }
    pub fn random(buf: &mut [u8]) -> bool {
        // Solo para los tests del anfitrión: no es azar de verdad.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for b in buf {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        true
    }
}

pub use imp::*;
