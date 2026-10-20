//! Más del CRT de Microsoft: cadenas UTF-16 (`wcs*`), conversión entre UTF-8 y UTF-16
//! (`mbstowcs`), matemática que no está en Rust (`erf`), el entorno en UTF-16, `wcsftime` y lo
//! que no se puede hacer (lanzar otros programas: `_wspawnv`, `_wsystem`), que falla con `errno`.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use super::crt::{self, errno, set_errno};
use crate::state;
use crate::win::{self, wstr};

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        wcsncpy_s,
        wcsxfrm,
        wcstok_s,
        wcstok,
        wcscoll,
        _wcsnicmp,
        wcspbrk,
        wcsspn,
        wcscspn,
        _wcslwr,
        _wcsupr,
        strcoll,
        _strnicoll,
        _stricoll,
        _strcoll_l,
        _memicmp,
        strxfrm,
        mbstowcs,
        wcstombs,
        mbstowcs_s,
        wcstombs_s,
        mbtowc,
        wctomb,
        mblen,
        btowc,
        wcrtomb,
        mbrtowc,
        wcstol,
        wcstoul,
        wcstoll,
        wcstoull,
        _wcstoi64,
        _wcstoui64,
        wcstod,
        _wtoi,
        _wtol,
        _wtoi64,
        _wtof,
        nextafter,
        nextafterf,
        acosh,
        asinh,
        atanh,
        erf,
        erfc,
        fma,
        tgamma,
        lgamma,
        remainder,
        _wputenv,
        _wputenv_s,
        _putenv_s,
        _wgetcwd,
        _wchdir,
        _wrmdir,
        _wunlink,
        _wfullpath,
        _set_thread_local_invalid_parameter_handler,
        _get_invalid_parameter_handler,
        __sys_errlist,
        __sys_nerr,
        __fpe_flt_rounds,
        _heapmin,
        wcsftime,
        _wsystem,
        _wexecv,
        _wexecve,
        _wspawnv,
        _wspawnve,
        _execv,
        _spawnv,
        _cwait,
        _wcserror,
        _wcserror_s,
        strerror_s,
        _strerror,
        _wperror,
        _wtempnam,
        _wtmpnam,
        _get_pgmptr,
        _get_wpgmptr,
        _getdrive,
        _wsetlocale,
        ___lc_locale_name_func,
        __lc_codepage,
        _beginthread,
    ]
}

unsafe fn wlen(p: *const u16) -> usize {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    n
}

unsafe fn wsl<'a>(p: *const u16) -> &'a [u16] {
    std::slice::from_raw_parts(p, wlen(p))
}

// --- Cadenas UTF-16 -----------------------------------------------------------------------------

unsafe extern "win64" fn wcsncpy_s(d: *mut u16, n: usize, s: *const u16, max: usize) -> i32 {
    if d.is_null() || n == 0 {
        return errno::EINVAL;
    }
    let mut len = 0;
    while len < max && *s.add(len) != 0 {
        len += 1;
    }
    if len >= n {
        if max != usize::MAX {
            *d = 0;
            return errno::ERANGE;
        }
        len = n - 1; // _TRUNCATE
    }
    std::ptr::copy(s, d, len);
    *d.add(len) = 0;
    0
}

unsafe extern "win64" fn wcsxfrm(d: *mut u16, s: *const u16, n: usize) -> usize {
    let len = wlen(s);
    if len < n && !d.is_null() {
        std::ptr::copy(s, d, len + 1);
    }
    len
}

unsafe extern "win64" fn wcstok_s(s: *mut u16, delim: *const u16, ctx: *mut *mut u16) -> *mut u16 {
    let set = wsl(delim).to_vec();
    let mut p = if s.is_null() { *ctx } else { s };
    if p.is_null() {
        return std::ptr::null_mut();
    }
    while *p != 0 && set.contains(&*p) {
        p = p.add(1);
    }
    if *p == 0 {
        *ctx = p;
        return std::ptr::null_mut();
    }
    let mut end = p;
    while *end != 0 && !set.contains(&*end) {
        end = end.add(1);
    }
    if *end == 0 {
        *ctx = end;
    } else {
        *end = 0;
        *ctx = end.add(1);
    }
    p
}

unsafe extern "win64" fn wcstok(s: *mut u16, delim: *const u16, ctx: *mut *mut u16) -> *mut u16 {
    // La UCRT tiene la versión de tres argumentos (como C11).
    static CTX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ctx = if ctx.is_null() {
        CTX.as_ptr() as *mut *mut u16
    } else {
        ctx
    };
    wcstok_s(s, delim, ctx)
}

fn cmp(a: &str, b: &str) -> i32 {
    a.cmp(b) as i32
}

unsafe extern "win64" fn wcscoll(a: *const u16, b: *const u16) -> i32 {
    cmp(&wstr(a), &wstr(b))
}

unsafe extern "win64" fn _wcsnicmp(a: *const u16, b: *const u16, n: usize) -> i32 {
    let x: String = wstr(a).chars().take(n).collect::<String>().to_lowercase();
    let y: String = wstr(b).chars().take(n).collect::<String>().to_lowercase();
    cmp(&x, &y)
}

unsafe extern "win64" fn wcspbrk(s: *const u16, set: *const u16) -> *const u16 {
    let set = wsl(set);
    wsl(s)
        .iter()
        .position(|c| set.contains(c))
        .map_or(std::ptr::null(), |i| s.add(i))
}

unsafe extern "win64" fn wcsspn(s: *const u16, set: *const u16) -> usize {
    let set = wsl(set);
    wsl(s).iter().take_while(|c| set.contains(c)).count()
}

unsafe extern "win64" fn wcscspn(s: *const u16, set: *const u16) -> usize {
    let set = wsl(set);
    wsl(s).iter().take_while(|c| !set.contains(c)).count()
}

unsafe fn map_wide(s: *mut u16, upper: bool) -> *mut u16 {
    let n = wlen(s);
    for i in 0..n {
        let c = char::from_u32(*s.add(i) as u32).unwrap_or('\0');
        let m: Vec<u16> = if upper {
            c.to_uppercase()
                .collect::<String>()
                .encode_utf16()
                .collect()
        } else {
            c.to_lowercase()
                .collect::<String>()
                .encode_utf16()
                .collect()
        };
        if m.len() == 1 {
            *s.add(i) = m[0];
        }
    }
    s
}

unsafe extern "win64" fn _wcslwr(s: *mut u16) -> *mut u16 {
    map_wide(s, false)
}
unsafe extern "win64" fn _wcsupr(s: *mut u16) -> *mut u16 {
    map_wide(s, true)
}

fn bytes_str(p: *const u8) -> String {
    // SAFETY: las funciones de C reciben textos terminados en cero.
    String::from_utf8_lossy(unsafe { win::cstr(p) }).into_owned()
}

unsafe extern "win64" fn strcoll(a: *const u8, b: *const u8) -> i32 {
    cmp(&bytes_str(a), &bytes_str(b))
}
unsafe extern "win64" fn _strcoll_l(a: *const u8, b: *const u8, _l: u64) -> i32 {
    strcoll(a, b)
}
unsafe extern "win64" fn _stricoll(a: *const u8, b: *const u8) -> i32 {
    cmp(&bytes_str(a).to_lowercase(), &bytes_str(b).to_lowercase())
}
unsafe extern "win64" fn _strnicoll(a: *const u8, b: *const u8, n: usize) -> i32 {
    let x: String = bytes_str(a)
        .chars()
        .take(n)
        .collect::<String>()
        .to_lowercase();
    let y: String = bytes_str(b)
        .chars()
        .take(n)
        .collect::<String>()
        .to_lowercase();
    cmp(&x, &y)
}
unsafe extern "win64" fn _memicmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let (x, y) = (
        std::slice::from_raw_parts(a, n),
        std::slice::from_raw_parts(b, n),
    );
    for (p, q) in x.iter().zip(y) {
        let (p, q) = (p.to_ascii_lowercase(), q.to_ascii_lowercase());
        if p != q {
            return p as i32 - q as i32;
        }
    }
    0
}
unsafe extern "win64" fn strxfrm(d: *mut u8, s: *const u8, n: usize) -> usize {
    let b = win::cstr(s);
    if b.len() < n && !d.is_null() {
        std::ptr::copy(s, d, b.len() + 1);
    }
    b.len()
}

// --- UTF-8 ↔ UTF-16 (la "página de códigos" de vino es UTF-8) ------------------------------------

/// `mbstowcs`: cuántos valores UTF-16 (sin el cero); copia hasta `n` si `d` no es nulo.
unsafe extern "win64" fn mbstowcs(d: *mut u16, s: *const u8, n: usize) -> usize {
    let w = win::to_wide(&bytes_str(s));
    if d.is_null() {
        return w.len();
    }
    let k = w.len().min(n);
    std::ptr::copy_nonoverlapping(w.as_ptr(), d, k);
    if k < n {
        *d.add(k) = 0;
    }
    k
}

unsafe extern "win64" fn wcstombs(d: *mut u8, s: *const u16, n: usize) -> usize {
    let b = wstr(s).into_bytes();
    if d.is_null() {
        return b.len();
    }
    let k = b.len().min(n);
    std::ptr::copy_nonoverlapping(b.as_ptr(), d, k);
    if k < n {
        *d.add(k) = 0;
    }
    k
}

unsafe extern "win64" fn mbstowcs_s(
    out: *mut usize,
    d: *mut u16,
    size: usize,
    s: *const u8,
    count: usize,
) -> i32 {
    let w = win::to_wide(&bytes_str(s));
    let k = w.len().min(count);
    if !d.is_null() {
        if k >= size {
            if size > 0 {
                *d = 0;
            }
            return errno::ERANGE;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), d, k);
        *d.add(k) = 0;
    }
    if !out.is_null() {
        *out = k + 1;
    }
    0
}

unsafe extern "win64" fn wcstombs_s(
    out: *mut usize,
    d: *mut u8,
    size: usize,
    s: *const u16,
    count: usize,
) -> i32 {
    let b = wstr(s).into_bytes();
    let k = b.len().min(count);
    if !d.is_null() {
        if k >= size {
            if size > 0 {
                *d = 0;
            }
            return errno::ERANGE;
        }
        std::ptr::copy_nonoverlapping(b.as_ptr(), d, k);
        *d.add(k) = 0;
    }
    if !out.is_null() {
        *out = k + 1;
    }
    0
}

/// Cuántos bytes ocupa el carácter UTF-8 que empieza con `b` (o 0 si no empieza uno).
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

unsafe extern "win64" fn mbtowc(d: *mut u16, s: *const u8, n: usize) -> i32 {
    if s.is_null() {
        return 0;
    }
    if *s == 0 {
        if !d.is_null() {
            *d = 0;
        }
        return 0;
    }
    let len = utf8_len(*s);
    if len == 0 || len > n {
        set_errno(42); // EILSEQ
        return -1;
    }
    let text = String::from_utf8_lossy(std::slice::from_raw_parts(s, len)).into_owned();
    if !d.is_null() {
        *d = text.encode_utf16().next().unwrap_or(0xfffd);
    }
    len as i32
}
unsafe extern "win64" fn mblen(s: *const u8, n: usize) -> i32 {
    mbtowc(std::ptr::null_mut(), s, n)
}
unsafe extern "win64" fn mbrtowc(d: *mut u16, s: *const u8, n: usize, _st: u64) -> usize {
    match mbtowc(d, s, n) {
        r if r < 0 => usize::MAX,
        r => r as usize,
    }
}
unsafe extern "win64" fn wctomb(d: *mut u8, c: u16) -> i32 {
    if d.is_null() {
        return 0;
    }
    let s = char::from_u32(c as u32).unwrap_or('?').to_string();
    std::ptr::copy_nonoverlapping(s.as_ptr(), d, s.len());
    s.len() as i32
}
unsafe extern "win64" fn wcrtomb(d: *mut u8, c: u16, _st: u64) -> usize {
    wctomb(d, c) as usize
}
unsafe extern "win64" fn btowc(c: i32) -> u16 {
    if (0..0x80).contains(&c) {
        c as u16
    } else {
        0xffff
    }
}

// --- Números en UTF-16 --------------------------------------------------------------------------

/// Los dígitos de un texto UTF-16 pasados a bytes (solo importan los ASCII) y cuántos valores
/// del texto corresponden a cada byte (1 a 1: los no ASCII cortan el número igual).
unsafe fn ascii(p: *const u16) -> Vec<u8> {
    wsl(p)
        .iter()
        .map(|&c| if c < 0x80 { c as u8 } else { 0xff })
        .collect()
}

unsafe fn wcsto(s: *const u16, end: *mut *const u16, base: i32, min: i128, max: i128) -> i128 {
    let (v, used, over) = crt::parse_int(&ascii(s), base as u32);
    if !end.is_null() {
        *end = s.add(used);
    }
    if over || v < min || v > max {
        set_errno(errno::ERANGE);
        return if v < 0 { min } else { max };
    }
    v
}

unsafe extern "win64" fn wcstol(s: *const u16, e: *mut *const u16, b: i32) -> i32 {
    wcsto(s, e, b, i32::MIN as i128, i32::MAX as i128) as i32
}
unsafe extern "win64" fn wcstoul(s: *const u16, e: *mut *const u16, b: i32) -> u32 {
    wcsto(s, e, b, -(u32::MAX as i128), u32::MAX as i128) as i64 as u32
}
unsafe extern "win64" fn wcstoll(s: *const u16, e: *mut *const u16, b: i32) -> i64 {
    wcsto(s, e, b, i64::MIN as i128, i64::MAX as i128) as i64
}
unsafe extern "win64" fn wcstoull(s: *const u16, e: *mut *const u16, b: i32) -> u64 {
    wcsto(s, e, b, -(u64::MAX as i128), u64::MAX as i128) as u64
}
unsafe extern "win64" fn _wcstoi64(s: *const u16, e: *mut *const u16, b: i32) -> i64 {
    wcstoll(s, e, b)
}
unsafe extern "win64" fn _wcstoui64(s: *const u16, e: *mut *const u16, b: i32) -> u64 {
    wcstoull(s, e, b)
}
unsafe extern "win64" fn wcstod(s: *const u16, e: *mut *const u16) -> f64 {
    let (v, used) = crt::parse_float(&ascii(s));
    if !e.is_null() {
        *e = s.add(used);
    }
    v
}
unsafe extern "win64" fn _wtoi(s: *const u16) -> i32 {
    crt::parse_int(&ascii(s), 10).0 as i32
}
unsafe extern "win64" fn _wtol(s: *const u16) -> i32 {
    _wtoi(s)
}
unsafe extern "win64" fn _wtoi64(s: *const u16) -> i64 {
    crt::parse_int(&ascii(s), 10).0 as i64
}
unsafe extern "win64" fn _wtof(s: *const u16) -> f64 {
    crt::parse_float(&ascii(s)).0
}

// --- Matemática ---------------------------------------------------------------------------------

/// El siguiente `f64` después de `x` en dirección a `y`.
pub fn next_after(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        return y;
    }
    if x == 0.0 {
        let tiny = f64::from_bits(1);
        return if y > 0.0 { tiny } else { -tiny };
    }
    let b = x.to_bits();
    let up = (y > x) == (x > 0.0);
    f64::from_bits(if up { b + 1 } else { b - 1 })
}

/// `erf` (función de error) con la aproximación de W. J. Cody, como la de musl: por tramos con
/// polinomios racionales. Acá, la serie de Taylor cerca de 0 y la fracción continua de `erfc` lejos.
pub fn erf_f(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let a = x.abs();
    let r = if a < 2.5 {
        // erf(x) = 2/√π · Σ (-1)^n x^(2n+1) / (n! (2n+1))
        let mut sum = 0.0;
        let mut term = a;
        let x2 = a * a;
        let mut n = 0.0;
        loop {
            let t = term / (2.0 * n + 1.0);
            sum += t;
            if t.abs() < 1e-17 * sum.abs() || n > 200.0 {
                break;
            }
            n += 1.0;
            term *= -x2 / n;
        }
        sum * 2.0 / std::f64::consts::PI.sqrt()
    } else {
        1.0 - erfc_f(a)
    };
    if x < 0.0 { -r } else { r }
}

/// `erfc(x) = 1 - erf(x)`, con la fracción continua de Laplace para x grande (sin perder
/// precisión por restar números parecidos).
pub fn erfc_f(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 2.5 {
        return 1.0 - erf_f(x);
    }
    // erfc(x) = e^(-x²)/√π · 1/(x + 1/2/(x + 1/(x + 3/2/(x + …))))
    let mut f = x;
    for k in (1..=60).rev() {
        f = x + (k as f64 / 2.0) / f;
    }
    (-x * x).exp() / (std::f64::consts::PI.sqrt() * f)
}

/// `tgamma` con la aproximación de Lanczos (g = 7, 9 coeficientes).
pub fn gamma_f(x: f64) -> f64 {
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        return std::f64::consts::PI / ((std::f64::consts::PI * x).sin() * gamma_f(1.0 - x));
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    (2.0 * std::f64::consts::PI).sqrt() * t.powf(x + 0.5) * (-t).exp() * a
}

unsafe extern "win64" fn nextafter(x: f64, y: f64) -> f64 {
    next_after(x, y)
}
unsafe extern "win64" fn nextafterf(x: f32, y: f32) -> f32 {
    if x == y || x.is_nan() || y.is_nan() {
        return if x.is_nan() { x } else { y };
    }
    if x == 0.0 {
        return if y > 0.0 {
            f32::from_bits(1)
        } else {
            -f32::from_bits(1)
        };
    }
    let b = x.to_bits();
    f32::from_bits(if (y > x) == (x > 0.0) { b + 1 } else { b - 1 })
}
unsafe extern "win64" fn acosh(x: f64) -> f64 {
    x.acosh()
}
unsafe extern "win64" fn asinh(x: f64) -> f64 {
    x.asinh()
}
unsafe extern "win64" fn atanh(x: f64) -> f64 {
    x.atanh()
}
unsafe extern "win64" fn erf(x: f64) -> f64 {
    erf_f(x)
}
unsafe extern "win64" fn erfc(x: f64) -> f64 {
    erfc_f(x)
}
unsafe extern "win64" fn fma(x: f64, y: f64, z: f64) -> f64 {
    x.mul_add(y, z)
}
unsafe extern "win64" fn tgamma(x: f64) -> f64 {
    gamma_f(x)
}
unsafe extern "win64" fn lgamma(x: f64) -> f64 {
    gamma_f(x).abs().ln()
}
unsafe extern "win64" fn remainder(x: f64, y: f64) -> f64 {
    x - (x / y).round_ties_even() * y
}

// --- Entorno y carpetas en UTF-16 -------------------------------------------------------------

unsafe extern "win64" fn _wputenv(s: *const u16) -> i32 {
    match wstr(s).split_once('=') {
        Some((k, v)) => {
            super::kernel32::set_env(k, (!v.is_empty()).then_some(v));
            0
        }
        None => -1,
    }
}
unsafe extern "win64" fn _wputenv_s(k: *const u16, v: *const u16) -> i32 {
    let v = wstr(v);
    super::kernel32::set_env(&wstr(k), (!v.is_empty()).then_some(v.as_str()));
    0
}
unsafe extern "win64" fn _putenv_s(k: *const u8, v: *const u8) -> i32 {
    let v = bytes_str(v);
    super::kernel32::set_env(&bytes_str(k), (!v.is_empty()).then_some(v.as_str()));
    0
}
unsafe extern "win64" fn _wgetcwd(buf: *mut u16, n: i32) -> *mut u16 {
    let d = std::env::current_dir()
        .map(|p| win::to_windows(&p.to_string_lossy()))
        .unwrap_or_else(|_| r"C:\".into());
    let w = win::to_wide_nul(&d);
    if buf.is_null() {
        let p = state::heap_alloc(w.len() * 2, false) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        }
        return p;
    }
    if w.len() > n.max(0) as usize {
        set_errno(errno::ERANGE);
        return std::ptr::null_mut();
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    buf
}
fn io(r: std::io::Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(_) => {
            set_errno(errno::ENOENT);
            -1
        }
    }
}
unsafe extern "win64" fn _wchdir(p: *const u16) -> i32 {
    io(std::env::set_current_dir(win::to_unix(&wstr(p))))
}
unsafe extern "win64" fn _wrmdir(p: *const u16) -> i32 {
    io(std::fs::remove_dir(win::to_unix(&wstr(p))))
}
unsafe extern "win64" fn _wunlink(p: *const u16) -> i32 {
    io(std::fs::remove_file(win::to_unix(&wstr(p))))
}
unsafe extern "win64" fn _wfullpath(buf: *mut u16, p: *const u16, n: usize) -> *mut u16 {
    let full = super::kernel32::full_path(&wstr(p));
    let w = win::to_wide_nul(&full);
    if buf.is_null() {
        let q = state::heap_alloc(w.len() * 2, false) as *mut u16;
        if !q.is_null() {
            std::ptr::copy_nonoverlapping(w.as_ptr(), q, w.len());
        }
        return q;
    }
    if w.len() > n {
        return std::ptr::null_mut();
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    buf
}

// --- Runtime ------------------------------------------------------------------------------------

unsafe extern "win64" fn _set_thread_local_invalid_parameter_handler(_h: u64) -> u64 {
    0
}
unsafe extern "win64" fn _get_invalid_parameter_handler() -> u64 {
    0
}

/// `__sys_errlist()`/`__sys_nerr()`: la tabla de mensajes de `strerror`.
static ERRLIST: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
const NERR: i32 = 43;
unsafe extern "win64" fn __sys_errlist() -> *const u64 {
    ERRLIST
        .get_or_init(|| {
            (0..NERR)
                .map(|e| crt::strerror_text(e).as_ptr() as u64)
                .collect()
        })
        .as_ptr()
}
static NERR_V: i32 = NERR;
unsafe extern "win64" fn __sys_nerr() -> *const i32 {
    &NERR_V
}
unsafe extern "win64" fn __fpe_flt_rounds() -> i32 {
    1 // al más cercano
}
unsafe extern "win64" fn _heapmin() -> i32 {
    0
}

unsafe extern "win64" fn wcsftime(
    buf: *mut u16,
    max: usize,
    fmt: *const u16,
    tm: *const i32,
) -> usize {
    let mut v = [0i32; 9];
    std::ptr::copy_nonoverlapping(tm, v.as_mut_ptr(), 9);
    let t = crt::strftime_text(wstr(fmt).as_bytes(), &v);
    let w = win::to_wide(&String::from_utf8_lossy(&t));
    if w.len() + 1 > max {
        return 0;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    *buf.add(w.len()) = 0;
    w.len()
}

/// Lanzar otro programa: JARVIS-OS todavía no deja que un programa lance otro (falta
/// `fork`/`exec` en el kernel). `errno = ENOENT`, como si no hubiera intérprete.
unsafe extern "win64" fn no_spawn() -> isize {
    set_errno(errno::ENOENT);
    -1
}
unsafe extern "win64" fn _wsystem(cmd: *const u16) -> i32 {
    if cmd.is_null() {
        return 0;
    }
    no_spawn() as i32
}
unsafe extern "win64" fn _wexecv() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _wexecve() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _wspawnv() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _wspawnve() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _execv() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _spawnv() -> isize {
    no_spawn()
}
unsafe extern "win64" fn _cwait() -> isize {
    set_errno(10); // ECHILD
    -1
}
/// `_beginthread` sin hilos en el kernel: falla (−1) con `EAGAIN` ("no hay recursos").
unsafe extern "win64" fn _beginthread(_f: u64, _stack: u32, _arg: u64) -> isize {
    set_errno(11);
    -1
}

unsafe extern "win64" fn _wcserror(e: i32) -> *const u16 {
    let s = String::from_utf8_lossy(crt::strerror_text(e))
        .trim_end_matches('\0')
        .to_string();
    let mut st = state::state();
    st.keep(
        win::to_wide_nul(&s)
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect(),
    ) as *const u16
}
unsafe extern "win64" fn _wcserror_s(buf: *mut u16, n: usize, e: i32) -> i32 {
    let s = String::from_utf8_lossy(crt::strerror_text(e))
        .trim_end_matches('\0')
        .to_string();
    win::put_wide(buf, n as u32, &s);
    0
}
unsafe extern "win64" fn strerror_s(buf: *mut u8, n: usize, e: i32) -> i32 {
    let s = String::from_utf8_lossy(crt::strerror_text(e))
        .trim_end_matches('\0')
        .to_string();
    win::put_narrow(buf, n as u32, &s);
    0
}
unsafe extern "win64" fn _strerror(_s: *const u8) -> *const u8 {
    crt::strerror_text(crt::get_errno()).as_ptr()
}
unsafe extern "win64" fn _wperror(s: *const u16) {
    let msg = String::from_utf8_lossy(crt::strerror_text(crt::get_errno()))
        .trim_end_matches('\0')
        .to_string();
    let p = if s.is_null() { String::new() } else { wstr(s) };
    let line = if p.is_empty() {
        format!("{msg}\n")
    } else {
        format!("{p}: {msg}\n")
    };
    let _ = super::kernel32::write_console(2, line.as_bytes());
}
unsafe extern "win64" fn _wtempnam(_d: *const u16, _p: *const u16) -> *mut u16 {
    set_errno(errno::EINVAL);
    std::ptr::null_mut()
}
unsafe extern "win64" fn _wtmpnam(_b: *mut u16) -> *mut u16 {
    std::ptr::null_mut()
}
unsafe extern "win64" fn _get_pgmptr(out: *mut u64) -> i32 {
    let p = state::state().image_path.clone();
    *out = state::state().keep(format!("{p}\0").into_bytes());
    0
}
unsafe extern "win64" fn _get_wpgmptr(out: *mut u64) -> i32 {
    let p = state::state().image_path.clone();
    *out = state::state().keep(
        win::to_wide_nul(&p)
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect(),
    );
    0
}
unsafe extern "win64" fn _getdrive() -> i32 {
    3 // C:
}
static C_W: [u16; 2] = [b'C' as u16, 0];
unsafe extern "win64" fn _wsetlocale(_cat: i32, _l: *const u16) -> *const u16 {
    C_W.as_ptr()
}
unsafe extern "win64" fn ___lc_locale_name_func() -> *const u64 {
    static NAMES: [u64; 6] = [0; 6];
    NAMES.as_ptr()
}
unsafe extern "win64" fn __lc_codepage() -> u32 {
    65001
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matematica_que_no_trae_rust() {
        assert_eq!(next_after(1.0, 2.0), 1.0 + f64::EPSILON);
        assert_eq!(next_after(1.0, 0.0), 1.0 - f64::EPSILON / 2.0);
        assert_eq!(next_after(0.0, -1.0), -f64::from_bits(1));
        // Valores de referencia (Abramowitz y Stegun, tabla 7.1).
        for (x, want) in [
            (0.5, 0.520_499_877_813_046_5),
            (1.0, 0.842_700_792_949_714_9),
            (3.0, 0.999_977_909_503_001_4),
        ] {
            assert!((erf_f(x) - want).abs() < 1e-14, "erf({x}) = {}", erf_f(x));
            assert!((erf_f(-x) + want).abs() < 1e-14);
        }
        assert!((erfc_f(5.0) - 1.537_459_794_428_034_8e-12).abs() < 1e-24);
        assert!((gamma_f(5.0) - 24.0).abs() < 1e-10);
        assert!((gamma_f(0.5) - std::f64::consts::PI.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn cadenas_utf16() {
        let w = win::to_wide_nul("  -42 resto");
        let mut end = std::ptr::null();
        // SAFETY: texto terminado en cero.
        unsafe {
            assert_eq!(wcstol(w.as_ptr(), &mut end, 10), -42);
            assert_eq!(end, w.as_ptr().add(5));
            let mut buf = [0u16; 4];
            let src = win::to_wide_nul("hola");
            assert_eq!(
                wcsncpy_s(buf.as_mut_ptr(), 4, src.as_ptr(), usize::MAX),
                0,
                "_TRUNCATE"
            );
            assert_eq!(win::from_wide(&buf[..3]), "hol");
            let mut t = win::to_wide_nul("a;b;;c");
            let d = win::to_wide_nul(";");
            let mut ctx = std::ptr::null_mut();
            let mut parts = Vec::new();
            let mut p = wcstok_s(t.as_mut_ptr(), d.as_ptr(), &mut ctx);
            while !p.is_null() {
                parts.push(wstr(p));
                p = wcstok_s(std::ptr::null_mut(), d.as_ptr(), &mut ctx);
            }
            assert_eq!(parts, ["a", "b", "c"]);
            let mut out = [0u16; 8];
            assert_eq!(mbstowcs(out.as_mut_ptr(), c"año".as_ptr().cast(), 8), 3);
            assert_eq!(win::from_wide(&out[..3]), "año");
        }
    }
}
