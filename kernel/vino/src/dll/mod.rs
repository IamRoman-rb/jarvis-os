//! Las DLL de Windows de `vino`, escritas en Rust ("builtin", como las llama Wine).
//!
//! No son archivos `.dll`: son funciones de este programa con la convención de llamada de
//! Windows (`extern "win64"`: argumentos en rcx, rdx, r8 y r9, 32 bytes de "sombra" en la pila,
//! xmm6–xmm15 se conservan). El cargador escribe su dirección en la IAT del `.exe` y el programa
//! las llama como si fueran de Windows.
//!
//! - [`kernel32`]: consola, archivos, memoria, hora, entorno, módulos, TLS y sincronización.
//! - [`ntdll`]: lo poco de NT que piden los programas (`NtWriteFile`, `Rtl*`).
//! - [`crt`]: la biblioteca de C de Microsoft (`ucrtbase`, `msvcrt`, `vcruntime140`).
//! - [`extra`]: azar (`bcryptprimitives`, `advapi32`) y `MessageBox` (`user32`) en la Terminal.
//!
//! Los nombres de DLL se normalizan: los "api sets" (`api-ms-win-crt-stdio-l1-1-0.dll`) son
//! nombres virtuales que Windows resuelve a la DLL de verdad (`ucrtbase.dll`), y `kernel32` toma
//! de `ntdll` lo que Windows le reenvía (`RtlCaptureContext`…).

#![allow(clippy::missing_safety_doc, clippy::too_many_arguments)]

pub mod crt;
pub mod crt2;
pub mod extra;
pub mod kernel32;
pub mod lowio;
pub mod ntdll;
pub mod sistema;

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::pe::Symbol;

/// Las DLL que tiene `vino`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dll {
    Kernel32,
    Ntdll,
    Crt,
    Bcrypt,
    Advapi32,
    User32,
    Version,
}

const ALL: [Dll; 7] = [
    Dll::Kernel32,
    Dll::Ntdll,
    Dll::Crt,
    Dll::Bcrypt,
    Dll::Advapi32,
    Dll::User32,
    Dll::Version,
];

/// `KERNEL32.dll`, `api-ms-win-crt-runtime-l1-1-0.dll`… → la DLL de `vino` que la reemplaza.
pub fn dll_of(name: &str) -> Option<Dll> {
    let n = name.to_ascii_lowercase();
    let n = n.strip_suffix(".dll").unwrap_or(&n);
    if n.starts_with("api-ms-win-crt-") {
        return Some(Dll::Crt);
    }
    if n.starts_with("api-ms-win-core-") || n.starts_with("ext-ms-win-") {
        return Some(Dll::Kernel32);
    }
    Some(match n {
        "kernel32" | "kernelbase" => Dll::Kernel32,
        "ntdll" => Dll::Ntdll,
        "ucrtbase" | "ucrtbased" | "msvcrt" | "vcruntime140" | "vcruntime140_1"
        | "vcruntime140d" => Dll::Crt,
        "bcryptprimitives" | "bcrypt" => Dll::Bcrypt,
        "advapi32" | "cryptbase" => Dll::Advapi32,
        "user32" => Dll::User32,
        "version" => Dll::Version,
        _ => return None,
    })
}

/// El nombre "oficial" (para `GetModuleFileName` y los mensajes).
pub fn file_name(d: Dll) -> &'static str {
    match d {
        Dll::Kernel32 => "kernel32.dll",
        Dll::Ntdll => "ntdll.dll",
        Dll::Crt => "ucrtbase.dll",
        Dll::Bcrypt => "bcryptprimitives.dll",
        Dll::Advapi32 => "advapi32.dll",
        Dll::User32 => "user32.dll",
        Dll::Version => "version.dll",
    }
}

fn table() -> &'static HashMap<(Dll, &'static str), usize> {
    static T: OnceLock<HashMap<(Dll, &'static str), usize>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = HashMap::new();
        for (d, list) in [
            (Dll::Kernel32, kernel32::exports()),
            (Dll::Kernel32, sistema::exports()),
            (Dll::Ntdll, ntdll::exports()),
            (Dll::Crt, crt::exports()),
            (Dll::Crt, lowio::exports()),
            (Dll::Crt, crt2::exports()),
            (Dll::Version, extra::version_exports()),
            (Dll::Bcrypt, extra::bcrypt_exports()),
            (Dll::Advapi32, extra::advapi32_exports()),
            (Dll::User32, extra::user32_exports()),
        ] {
            for (name, f) in list {
                t.insert((d, name), f);
            }
        }
        t
    })
}

/// La función `name` de la DLL `d` (o, si es `kernel32`, la que Windows le reenvía a `ntdll`).
pub fn find(d: Dll, name: &str) -> Option<usize> {
    let t = table();
    t.get(&(d, name))
        .or_else(|| match d {
            Dll::Kernel32 => t.get(&(Dll::Ntdll, name)),
            _ => None,
        })
        .copied()
}

/// La dirección para un import del programa: la función nuestra o, si no existe, una que avisa
/// y termina.
pub fn resolve(dll: &str, sym: &Symbol) -> u64 {
    let found = match (dll_of(dll), sym) {
        (Some(d), Symbol::Name(n)) => find(d, n),
        _ => None,
    };
    found
        .map(|a| a as u64)
        .unwrap_or_else(|| missing::stub_for(missing::label(dll, sym, None)))
}

// --- Los HMODULE de las DLL builtin ---------------------------------------------------------

/// Un HMODULE inventado para cada DLL nuestra (para `GetModuleHandle` y `GetProcAddress`). No
/// apunta a nada: los programas solo lo usan para pedir funciones.
const MODULE_BASE: u64 = 0x7ff0_0000_0000;

pub fn module_handle(d: Dll) -> u64 {
    MODULE_BASE + ALL.iter().position(|&x| x == d).unwrap_or(0) as u64 * 0x1_0000
}

pub fn dll_of_handle(h: u64) -> Option<Dll> {
    let i = h.checked_sub(MODULE_BASE)?;
    (i % 0x1_0000 == 0)
        .then(|| ALL.get((i / 0x1_0000) as usize).copied())
        .flatten()
}

/// Llama a una función del programa sin argumentos (`atexit`, `_initterm`).
///
/// # Safety
/// `f` es la dirección de una función del programa de ese tipo.
pub unsafe fn call0(f: u64) {
    // SAFETY: lo garantiza quien llama.
    let f: unsafe extern "win64" fn() = unsafe { std::mem::transmute(f as usize) };
    // SAFETY: ídem.
    unsafe { f() }
}

/// Termina como `exit()` de C: las funciones de `atexit` (de la última a la primera), vaciar
/// los `FILE` y salir.
pub fn exit(code: i32) -> ! {
    run_atexit();
    exit_now(code)
}

pub fn run_atexit() {
    loop {
        // El lock se suelta antes de llamar al programa (puede registrar más funciones).
        let f = crate::state::state().atexit.pop();
        let Some(f) = f else { break };
        // SAFETY: el programa la registró con atexit/_crt_atexit/_register_onexit_function.
        unsafe { call0(f) };
    }
}

/// Termina ya (`_exit`, `ExitProcess`): vacía los `FILE` y sale con `code`.
pub fn exit_now(code: i32) -> ! {
    crt::flush_all();
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

/// Algo que `vino` no sabe hacer y el programa necesita: lo dice y termina.
pub fn fatal(what: &str) -> ! {
    crt::flush_all();
    let _ = std::io::stdout().flush();
    // De una sola vez (`eprintln!` escribe de a pedazos y la Terminal los recibe por separado).
    let _ = std::io::stderr().write_all(format!("vino: {what}\n").as_bytes());
    std::process::exit(3)
}

/// Las funciones que faltan.
///
/// Cada import que no tenemos apunta a una función distinta (`stub::<N>`), así el aviso puede
/// decir **cuál** llamó el programa. Son funciones normales generadas por el compilador (una por
/// N con genéricos), no código armado en tiempo de ejecución.
pub mod missing {
    use super::*;

    const N: usize = 512;
    static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn missing(n: usize) -> ! {
        let name = NAMES
            .lock()
            .ok()
            .and_then(|v| v.get(n).cloned())
            .unwrap_or_else(|| "una función".into());
        fatal(&format!(
            "falta {name}: el programa la usa y vino todavía no la tiene"
        ))
    }

    unsafe extern "win64" fn stub<const I: usize>() -> u64 {
        missing(I)
    }

    /// Una función que no se sabe cuál es (pasaron las 512 distintas).
    unsafe extern "win64" fn any() -> u64 {
        missing(usize::MAX)
    }

    macro_rules! row {
        ($v:ident, $hi:literal) => {
            $v.extend_from_slice(&[
                stub::<{ $hi * 16 }> as *const () as usize,
                stub::<{ $hi * 16 + 1 }> as *const () as usize,
                stub::<{ $hi * 16 + 2 }> as *const () as usize,
                stub::<{ $hi * 16 + 3 }> as *const () as usize,
                stub::<{ $hi * 16 + 4 }> as *const () as usize,
                stub::<{ $hi * 16 + 5 }> as *const () as usize,
                stub::<{ $hi * 16 + 6 }> as *const () as usize,
                stub::<{ $hi * 16 + 7 }> as *const () as usize,
                stub::<{ $hi * 16 + 8 }> as *const () as usize,
                stub::<{ $hi * 16 + 9 }> as *const () as usize,
                stub::<{ $hi * 16 + 10 }> as *const () as usize,
                stub::<{ $hi * 16 + 11 }> as *const () as usize,
                stub::<{ $hi * 16 + 12 }> as *const () as usize,
                stub::<{ $hi * 16 + 13 }> as *const () as usize,
                stub::<{ $hi * 16 + 14 }> as *const () as usize,
                stub::<{ $hi * 16 + 15 }> as *const () as usize,
            ])
        };
    }

    fn stubs() -> &'static [usize] {
        static S: OnceLock<Vec<usize>> = OnceLock::new();
        S.get_or_init(|| {
            let mut v = Vec::with_capacity(N);
            row!(v, 0);
            row!(v, 1);
            row!(v, 2);
            row!(v, 3);
            row!(v, 4);
            row!(v, 5);
            row!(v, 6);
            row!(v, 7);
            row!(v, 8);
            row!(v, 9);
            row!(v, 10);
            row!(v, 11);
            row!(v, 12);
            row!(v, 13);
            row!(v, 14);
            row!(v, 15);
            row!(v, 16);
            row!(v, 17);
            row!(v, 18);
            row!(v, 19);
            row!(v, 20);
            row!(v, 21);
            row!(v, 22);
            row!(v, 23);
            row!(v, 24);
            row!(v, 25);
            row!(v, 26);
            row!(v, 27);
            row!(v, 28);
            row!(v, 29);
            row!(v, 30);
            row!(v, 31);
            v
        })
    }

    /// `KERNEL32!Foo`, `WS2_32!#23`; con `why` si falta la DLL entera.
    pub fn label(dll: &str, sym: &Symbol, why: Option<&str>) -> String {
        let file = dll.rsplit(['\\', '/']).next().unwrap_or(dll);
        let short = file.split('.').next().unwrap_or(file).to_ascii_uppercase();
        let what = match sym {
            Symbol::Name(n) => format!("{short}!{n}"),
            Symbol::Ordinal(o) => format!("{short}!#{o}"),
        };
        match why {
            Some(w) => format!("{what} ({w})"),
            None => what,
        }
    }

    /// La función que avisa que falta `name` (la misma si se pide dos veces).
    pub fn stub_for(name: String) -> u64 {
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        let i = match names.iter().position(|n| *n == name) {
            Some(i) => i,
            None => {
                names.push(name);
                names.len() - 1
            }
        };
        stubs().get(i).copied().unwrap_or(any as *const () as usize) as u64
    }

    /// Los nombres que faltaron al cargar (para `vino --imports`).
    pub fn names() -> Vec<String> {
        NAMES.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres_de_dll_y_api_sets() {
        assert_eq!(dll_of("KERNEL32.dll"), Some(Dll::Kernel32));
        assert_eq!(dll_of("api-ms-win-crt-stdio-l1-1-0.dll"), Some(Dll::Crt));
        assert_eq!(
            dll_of("api-ms-win-core-synch-l1-2-0.dll"),
            Some(Dll::Kernel32)
        );
        assert_eq!(dll_of("VCRUNTIME140.dll"), Some(Dll::Crt));
        assert_eq!(dll_of("msvcrt.dll"), Some(Dll::Crt));
        assert_eq!(dll_of("d3d11.dll"), None);
        assert_eq!(dll_of_handle(module_handle(Dll::User32)), Some(Dll::User32));
        assert_eq!(dll_of_handle(0x1_4000_0000), None);
    }

    #[test]
    fn resuelve_lo_que_hay_y_avisa_lo_que_falta() {
        let k = resolve("KERNEL32.dll", &Symbol::Name("GetStdHandle".into()));
        assert_eq!(
            k as usize,
            kernel32::exports()
                .iter()
                .find(|e| e.0 == "GetStdHandle")
                .unwrap()
                .1
        );
        // kernel32 reenvía a ntdll.
        assert!(find(Dll::Kernel32, "RtlCaptureContext").is_some());
        // Los imports que usan los programas de prueba existen todos.
        for name in [
            "WriteConsoleW",
            "CreateFileW",
            "FindFirstFileExW",
            "FlsAlloc",
            "GetEnvironmentStringsW",
            "WaitOnAddress",
            "SetFileInformationByHandle",
            "GetFileInformationByHandleEx",
        ] {
            assert!(find(Dll::Kernel32, name).is_some(), "{name}");
        }
        for name in [
            "__stdio_common_vfprintf",
            "_initterm_e",
            "__acrt_iob_func",
            "memcpy",
            "printf",
        ] {
            assert!(find(Dll::Crt, name).is_some(), "{name}");
        }
        let a = resolve("KERNEL32.dll", &Symbol::Name("NoExiste".into()));
        let b = resolve("KERNEL32.dll", &Symbol::Name("TampocoExiste".into()));
        let a2 = resolve("KERNEL32.dll", &Symbol::Name("NoExiste".into()));
        assert_ne!(a, b);
        assert_eq!(a, a2);
        assert!(missing::names().contains(&"KERNEL32!NoExiste".to_string()));
        resolve("ws2_32.dll", &Symbol::Ordinal(23));
        assert!(missing::names().contains(&"WS2_32!#23".to_string()));
    }
}
