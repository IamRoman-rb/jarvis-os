//! Lo que Windows hace distinto de Linux, en funciones puras: texto en UTF-16, rutas con `C:\` y
//! barras invertidas, la hora en "FILETIME" (centenas de nanosegundos desde 1601) y los códigos de
//! error.

use std::io;

// --- UTF-16 -----------------------------------------------------------------------------------

/// UTF-8 → UTF-16, sin el cero final.
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// UTF-8 → UTF-16 con el cero final.
pub fn to_wide_nul(s: &str) -> Vec<u16> {
    let mut w = to_wide(s);
    w.push(0);
    w
}

/// UTF-16 → UTF-8 (lo inválido sale como U+FFFD).
pub fn from_wide(w: &[u16]) -> String {
    String::from_utf16_lossy(w)
}

/// Un texto UTF-16 terminado en cero, desde un puntero del programa.
///
/// # Safety
/// `p` es nulo o apunta a un texto UTF-16 terminado en cero.
pub unsafe fn wstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: lo garantiza quien llama: hasta el cero, todo es del texto.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: los `n` valores de arriba existen.
    from_wide(unsafe { std::slice::from_raw_parts(p, n) })
}

/// Un texto de bytes terminado en cero, desde un puntero del programa.
///
/// # Safety
/// `p` es nulo o apunta a un texto terminado en cero.
pub unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: lo garantiza quien llama.
    unsafe { std::ffi::CStr::from_ptr(p.cast()).to_bytes() }
}

/// Copia `s` a un buffer de `cap` valores UTF-16 como las funciones `…W` de Windows que
/// devuelven un largo: si entra (con el cero), devuelve cuántos copió **sin** el cero; si no,
/// cuántos hacen falta **con** el cero (y no copia nada).
///
/// # Safety
/// `dst` apunta a `cap` valores escribibles (o `cap` es 0).
pub unsafe fn put_wide(dst: *mut u16, cap: u32, s: &str) -> u32 {
    let w = to_wide_nul(s);
    if dst.is_null() || (cap as usize) < w.len() {
        return w.len() as u32;
    }
    // SAFETY: entran `w.len()` valores (lo de arriba).
    unsafe { std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len()) };
    (w.len() - 1) as u32
}

/// Lo mismo con bytes (las funciones `…A`; el texto va en UTF-8, la "página de códigos" de
/// `vino`: ver `GetACP`).
///
/// # Safety
/// `dst` apunta a `cap` bytes escribibles (o `cap` es 0).
pub unsafe fn put_narrow(dst: *mut u8, cap: u32, s: &str) -> u32 {
    let n = s.len() + 1;
    if dst.is_null() || (cap as usize) < n {
        return n as u32;
    }
    // SAFETY: entran `n` bytes (lo de arriba).
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), dst, s.len());
        *dst.add(s.len()) = 0;
    }
    s.len() as u32
}

// --- Rutas ------------------------------------------------------------------------------------

/// Una ruta de Windows → la de JARVIS-OS. Todas las unidades (`C:`, `D:`…) son la raíz: el disco
/// de JARVIS-OS es uno solo. `\\?\` (rutas "literales") y `\??\` (las de NT) se sacan.
pub fn to_unix(p: &str) -> String {
    let p = p
        .strip_prefix(r"\\?\")
        .or_else(|| p.strip_prefix(r"\??\"))
        .unwrap_or(p);
    let lower = p.to_ascii_lowercase();
    match lower.as_str() {
        "nul" => return "/dev/null".into(),
        "con" | "conout$" | "conin$" => return "/dev/tty".into(),
        _ => {}
    }
    let b = p.as_bytes();
    let rest = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let r = &p[2..];
        // `C:foo` es relativa (a la carpeta actual de esa unidad, que es la única).
        if r.starts_with(['\\', '/']) || r.is_empty() {
            if r.is_empty() { "/" } else { r }
        } else {
            r
        }
    } else {
        p
    };
    let out: String = rest
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    if out.is_empty() { ".".into() } else { out }
}

/// Una ruta de JARVIS-OS → como la ve un programa de Windows: `/Descargas/x.exe` →
/// `C:\Descargas\x.exe`. Las relativas quedan relativas.
pub fn to_windows(p: &str) -> String {
    let w: String = p.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    if p.starts_with('/') {
        format!("C:{w}")
    } else {
        w
    }
}

// --- Hora -------------------------------------------------------------------------------------

/// Segundos entre 1601-01-01 (el comienzo de FILETIME) y 1970-01-01.
pub const EPOCH_DIFF: u64 = 11_644_473_600;

/// Nanosegundos desde 1970 → FILETIME.
pub fn filetime(unix_ns: u128) -> u64 {
    (unix_ns / 100) as u64 + EPOCH_DIFF * 10_000_000
}

/// La hora actual en FILETIME.
pub fn filetime_now() -> u64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    filetime(d.as_nanos())
}

/// FILETIME → SYSTEMTIME: año, mes, día de la semana (0 = domingo), día, hora, minuto, segundo
/// y milisegundo, como los ocho `WORD` de la estructura de Windows.
pub fn system_time(ft: u64) -> [u16; 8] {
    let ms_total = ft / 10_000;
    let ms = (ms_total % 1000) as u16;
    let secs = (ms_total / 1000).saturating_sub(EPOCH_DIFF);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil(days);
    // 1970-01-01 fue jueves (4).
    let wday = ((days % 7 + 7 + 4) % 7) as u16;
    [
        y as u16,
        m as u16,
        wday,
        d as u16,
        (rem / 3600) as u16,
        (rem / 60 % 60) as u16,
        (rem % 60) as u16,
        ms,
    ]
}

/// Días desde 1970 → (año, mes, día). El algoritmo de Howard Hinnant ("chrono-compatible
/// low-level date algorithms"): cuenta en eras de 400 años que empiezan el 1 de marzo, así el
/// 29 de febrero queda al final del año.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

// --- Errores ----------------------------------------------------------------------------------

pub mod error {
    pub const SUCCESS: u32 = 0;
    pub const FILE_NOT_FOUND: u32 = 2;
    pub const PATH_NOT_FOUND: u32 = 3;
    pub const ACCESS_DENIED: u32 = 5;
    pub const INVALID_HANDLE: u32 = 6;
    pub const NOT_ENOUGH_MEMORY: u32 = 8;
    pub const NO_MORE_FILES: u32 = 18;
    pub const GEN_FAILURE: u32 = 31;
    pub const HANDLE_EOF: u32 = 38;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const FILE_EXISTS: u32 = 80;
    pub const INVALID_PARAMETER: u32 = 87;
    pub const CALL_NOT_IMPLEMENTED: u32 = 120;
    pub const INSUFFICIENT_BUFFER: u32 = 122;
    pub const MOD_NOT_FOUND: u32 = 126;
    pub const PROC_NOT_FOUND: u32 = 127;
    pub const DIR_NOT_EMPTY: u32 = 145;
    pub const ALREADY_EXISTS: u32 = 183;
    pub const ENVVAR_NOT_FOUND: u32 = 203;
    pub const BROKEN_PIPE: u32 = 109;
}

/// Un error de E/S de Rust (que vino de un errno de Linux) → el código de Windows.
pub fn error_code(e: &io::Error) -> u32 {
    use io::ErrorKind::*;
    match e.kind() {
        NotFound => error::FILE_NOT_FOUND,
        PermissionDenied => error::ACCESS_DENIED,
        AlreadyExists => error::ALREADY_EXISTS,
        DirectoryNotEmpty => error::DIR_NOT_EMPTY,
        NotADirectory => error::PATH_NOT_FOUND,
        InvalidInput => error::INVALID_PARAMETER,
        OutOfMemory => error::NOT_ENOUGH_MEMORY,
        Unsupported => error::NOT_SUPPORTED,
        BrokenPipe => error::BROKEN_PIPE,
        _ => error::GEN_FAILURE,
    }
}

/// La línea de comandos de Windows a partir de los argumentos: Windows no le pasa al programa
/// una lista sino **un texto**, y cada programa lo parte (el CRT o la biblioteca de Rust) con
/// las reglas de `CommandLineToArgvW`. Comillas para lo que tiene espacios; las barras
/// invertidas solo se duplican antes de una comilla.
pub fn command_line(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            out.push_str(a);
            continue;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in a.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_de_windows_a_jarvis_y_vuelta() {
        assert_eq!(to_unix(r"C:\Descargas\x.exe"), "/Descargas/x.exe");
        assert_eq!(to_unix(r"d:\a\b"), "/a/b");
        assert_eq!(to_unix(r"\\?\C:\Documentos"), "/Documentos");
        assert_eq!(to_unix(r"carpeta\datos.txt"), "carpeta/datos.txt");
        assert_eq!(to_unix("C:"), "/");
        assert_eq!(to_unix("C:notas.txt"), "notas.txt");
        assert_eq!(to_unix("NUL"), "/dev/null");
        assert_eq!(to_unix(""), ".");
        assert_eq!(to_windows("/Descargas/x.exe"), r"C:\Descargas\x.exe");
        assert_eq!(to_windows("a/b"), r"a\b");
    }

    #[test]
    fn utf16_ida_y_vuelta() {
        let w = to_wide_nul("año ✓");
        assert_eq!(*w.last().unwrap(), 0);
        // SAFETY: `w` termina en cero.
        assert_eq!(unsafe { wstr(w.as_ptr()) }, "año ✓");
        let mut buf = [0u16; 4];
        // SAFETY: `buf` tiene 4 lugares.
        assert_eq!(
            unsafe { put_wide(buf.as_mut_ptr(), 4, "hola") },
            5,
            "no entra"
        );
        // SAFETY: ídem.
        assert_eq!(unsafe { put_wide(buf.as_mut_ptr(), 4, "abc") }, 3);
        assert_eq!(buf, [97, 98, 99, 0]);
    }

    #[test]
    fn hora_de_windows() {
        // 2026-10-20 12:34:56.789 UTC, un martes.
        let ns = (1_792_499_696_789u128) * 1_000_000;
        let ft = filetime(ns);
        assert_eq!(ft, 1_792_499_696_789 * 10_000 + EPOCH_DIFF * 10_000_000);
        assert_eq!(system_time(ft), [2026, 10, 2, 20, 12, 34, 56, 789]);
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(19_782), (2024, 2, 29));
    }

    #[test]
    fn linea_de_comandos() {
        let a = |v: &[&str]| command_line(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["x.exe", "uno", "dos"]), "x.exe uno dos");
        assert_eq!(
            a(&["x.exe", "con espacio", ""]),
            r#"x.exe "con espacio" """#
        );
        assert_eq!(a(&["x", r#"di "hola""#]), r#"x "di \"hola\"""#);
        assert_eq!(a(&["x", r"C:\a b\"]), r#"x "C:\a b\\""#);
    }
}
