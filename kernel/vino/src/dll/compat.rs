//! Lo que piden programas reales instalados con `winget` (K16, ADR 0021): ripgrep (`rg.exe`,
//! Rust con el CRT de Microsoft) y jq (`jq.exe`, C con el UCRT). Salió de `vino --imports`.
//!
//! - **E/S asíncrona y tuberías con nombre** (`ReadFileEx`, `WriteFileEx`,
//!   `NtCreateNamedPipeFile`) y **contextos de hilos** (`Get/SetThreadContext`): `vino` no las
//!   tiene; contestan "no soportado", como un Windows sin esa función, y el programa sigue por
//!   otro camino (la biblioteca de Rust solo las usa para lanzar otros procesos).
//! - **`setjmp`/`longjmp`** en ensamblador: guardan y restauran los registros que la convención
//!   de Windows x64 obliga a conservar (rbx, rbp, rsi, rdi, r12–r15, rsp, xmm6–xmm15, MXCSR) y la
//!   dirección de retorno. El `longjmp` de Microsoft también desenrolla (para C++); para C, que es
//!   quien lo usa, alcanza con restaurar.
//! - **Funciones de Bessel** (`_j0`, `_y1`, `_jn`…) con las aproximaciones racionales de
//!   *Numerical Recipes* (error relativo ~1e-8) y recurrencias; `fdim` y `logb`.
//! - `mbsrtowcs`, `_assert`/`_wassert`, `PathIsRelative` (shlwapi) y
//!   `GetUserProfileDirectoryW` (userenv: la carpeta personal es la raíz, como en JARVIS-OS).

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::io::Write;

use crate::state;
use crate::win::{self, cstr, error, wstr};

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn kernel32_exports() -> Vec<(&'static str, usize)> {
    exports![
        ReadFileEx,
        WriteFileEx,
        GetThreadContext,
        SetThreadContext,
        SetProcessAffinityMask,
    ]
}

pub fn ntdll_exports() -> Vec<(&'static str, usize)> {
    exports![NtOpenFile, NtCreateNamedPipeFile]
}

pub fn crt_exports() -> Vec<(&'static str, usize)> {
    let mut v = exports![
        mbsrtowcs, _j0, _j1, _jn, _y0, _y1, _yn, fdim, logb, _assert, _wassert
    ];
    let setjmp = vino_setjmp as *const () as usize;
    v.push(("__intrinsic_setjmp", setjmp));
    v.push(("__intrinsic_setjmpex", setjmp));
    v.push(("_setjmp", setjmp));
    v.push(("_setjmpex", setjmp));
    v.push(("setjmp", setjmp));
    v.push(("longjmp", vino_longjmp as *const () as usize));
    v
}

pub fn userenv_exports() -> Vec<(&'static str, usize)> {
    exports![GetUserProfileDirectoryW, GetUserProfileDirectoryA]
}

pub fn shlwapi_exports() -> Vec<(&'static str, usize)> {
    exports![PathIsRelativeA, PathIsRelativeW]
}

// --- "no soportado" --------------------------------------------------------------------------------

fn not_supported() -> i32 {
    state::set_last_error(error::NOT_SUPPORTED);
    0
}

unsafe extern "win64" fn ReadFileEx(_h: u64, _b: u64, _n: u32, _o: u64, _cb: u64) -> i32 {
    not_supported()
}

unsafe extern "win64" fn WriteFileEx(_h: u64, _b: u64, _n: u32, _o: u64, _cb: u64) -> i32 {
    not_supported()
}

unsafe extern "win64" fn GetThreadContext(_t: u64, _ctx: u64) -> i32 {
    not_supported()
}

unsafe extern "win64" fn SetThreadContext(_t: u64, _ctx: u64) -> i32 {
    not_supported()
}

/// Todos los procesos usan todas las CPU: no hay nada que limitar.
unsafe extern "win64" fn SetProcessAffinityMask(_p: u64, _mask: u64) -> i32 {
    1
}

/// STATUS_NOT_IMPLEMENTED: la biblioteca de Rust lo toma como "esta vía no está" y sigue.
unsafe extern "win64" fn NtOpenFile(_h: u64, _a: u32, _o: u64, _io: u64, _s: u32, _f: u32) -> u32 {
    0xC000_0002
}

/// STATUS_NOT_SUPPORTED.
unsafe extern "win64" fn NtCreateNamedPipeFile() -> u32 {
    0xC000_00BB
}

// --- setjmp / longjmp ---------------------------------------------------------------------------
//
// El `jmp_buf` de Windows mide 256 bytes (16 de 16). Lo usamos así (`Frame`, el primer campo,
// no hace falta):
//   0x08 rbx · 0x10 rsp (después de volver) · 0x18 rbp · 0x20 rsi · 0x28 rdi · 0x30–0x48 r12–r15
//   0x50 rip (adónde vuelve setjmp) · 0x58 MXCSR · 0x60–0xF0 xmm6–xmm15

core::arch::global_asm!(
    ".globl vino_setjmp",
    "vino_setjmp:",
    "mov [rcx + 0x08], rbx",
    "lea rax, [rsp + 8]",
    "mov [rcx + 0x10], rax",
    "mov [rcx + 0x18], rbp",
    "mov [rcx + 0x20], rsi",
    "mov [rcx + 0x28], rdi",
    "mov [rcx + 0x30], r12",
    "mov [rcx + 0x38], r13",
    "mov [rcx + 0x40], r14",
    "mov [rcx + 0x48], r15",
    "mov rax, [rsp]",
    "mov [rcx + 0x50], rax",
    "stmxcsr [rcx + 0x58]",
    "movdqu [rcx + 0x60], xmm6",
    "movdqu [rcx + 0x70], xmm7",
    "movdqu [rcx + 0x80], xmm8",
    "movdqu [rcx + 0x90], xmm9",
    "movdqu [rcx + 0xA0], xmm10",
    "movdqu [rcx + 0xB0], xmm11",
    "movdqu [rcx + 0xC0], xmm12",
    "movdqu [rcx + 0xD0], xmm13",
    "movdqu [rcx + 0xE0], xmm14",
    "movdqu [rcx + 0xF0], xmm15",
    "xor eax, eax",
    "ret",
    "",
    ".globl vino_longjmp",
    "vino_longjmp:",
    // longjmp(buf, 0) vuelve como si fuera 1 (lo pide C).
    "mov eax, edx",
    "test eax, eax",
    "jnz 2f",
    "mov eax, 1",
    "2:",
    "mov rbx, [rcx + 0x08]",
    "mov rbp, [rcx + 0x18]",
    "mov rsi, [rcx + 0x20]",
    "mov rdi, [rcx + 0x28]",
    "mov r12, [rcx + 0x30]",
    "mov r13, [rcx + 0x38]",
    "mov r14, [rcx + 0x40]",
    "mov r15, [rcx + 0x48]",
    "ldmxcsr [rcx + 0x58]",
    "movdqu xmm6, [rcx + 0x60]",
    "movdqu xmm7, [rcx + 0x70]",
    "movdqu xmm8, [rcx + 0x80]",
    "movdqu xmm9, [rcx + 0x90]",
    "movdqu xmm10, [rcx + 0xA0]",
    "movdqu xmm11, [rcx + 0xB0]",
    "movdqu xmm12, [rcx + 0xC0]",
    "movdqu xmm13, [rcx + 0xD0]",
    "movdqu xmm14, [rcx + 0xE0]",
    "movdqu xmm15, [rcx + 0xF0]",
    "mov rsp, [rcx + 0x10]",
    "jmp qword ptr [rcx + 0x50]",
    "",
    // Para los tests: setjmp; si dio 0, longjmp(buf, valor); devuelve lo que dio setjmp la
    // segunda vez. Todo en ensamblador (en Rust, una función que "vuelve dos veces" no es válida).
    ".globl vino_test_jump",
    "vino_test_jump:",
    "push rbx",
    "sub rsp, 0x130",
    "mov ebx, ecx",
    "lea rcx, [rsp + 0x20]",
    "call vino_setjmp",
    "test eax, eax",
    "jnz 3f",
    "lea rcx, [rsp + 0x20]",
    "mov edx, ebx",
    "call vino_longjmp",
    "3:",
    "add rsp, 0x130",
    "pop rbx",
    "ret",
);

unsafe extern "win64" {
    fn vino_setjmp(buf: *mut u8) -> i32;
    fn vino_longjmp(buf: *mut u8, value: i32) -> !;
    #[allow(dead_code)]
    fn vino_test_jump(value: i32) -> i32;
}

// --- matemática ----------------------------------------------------------------------------------

/// 2/π (lo usan las fórmulas de Bessel de *Numerical Recipes*).
const TWO_OVER_PI: f64 = core::f64::consts::FRAC_2_PI;

pub fn bessel_j0(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 8.0 {
        let y = x * x;
        let a = 57_568_490_574.0
            + y * (-13_362_590_354.0
                + y * (651_619_640.7
                    + y * (-11_214_424.18 + y * (77_392.330_17 + y * -184.905_245_6))));
        let b = 57_568_490_411.0
            + y * (1_029_532_985.0
                + y * (9_494_680.718 + y * (59_272.648_53 + y * (267.853_271_2 + y))));
        a / b
    } else {
        let z = 8.0 / ax;
        let y = z * z;
        let xx = ax - core::f64::consts::FRAC_PI_4;
        let (a, b) = big_j0(y);
        (TWO_OVER_PI / ax).sqrt() * (xx.cos() * a - z * xx.sin() * b)
    }
}

fn big_j0(y: f64) -> (f64, f64) {
    (
        1.0 + y
            * (-0.109_862_862_7e-2
                + y * (0.273_451_040_7e-4 + y * (-0.207_337_063_9e-5 + y * 0.209_388_721_1e-6))),
        -0.156_249_999_5e-1
            + y * (0.143_048_876_5e-3
                + y * (-0.691_114_765_1e-5 + y * (0.762_109_516_1e-6 - y * 0.934_935_152e-7))),
    )
}

fn big_j1(y: f64) -> (f64, f64) {
    (
        1.0 + y
            * (0.183_105e-2
                + y * (-0.351_639_649_6e-4 + y * (0.245_752_017_4e-5 + y * -0.240_337_019e-6))),
        0.046_874_999_95
            + y * (-0.200_269_087_3e-3
                + y * (0.844_919_909_6e-5 + y * (-0.882_289_87e-6 + y * 0.105_787_412e-6))),
    )
}

pub fn bessel_j1(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 8.0 {
        let y = x * x;
        let a = x
            * (72_362_614_232.0
                + y * (-7_895_059_235.0
                    + y * (242_396_853.1
                        + y * (-2_972_611.439 + y * (15_704.482_60 + y * -30.160_366_06)))));
        let b = 144_725_228_442.0
            + y * (2_300_535_178.0
                + y * (18_583_304.74 + y * (99_447.433_94 + y * (376.999_139_7 + y))));
        a / b
    } else {
        let z = 8.0 / ax;
        let y = z * z;
        let xx = ax - 3.0 * core::f64::consts::FRAC_PI_4;
        let (a, b) = big_j1(y);
        let r = (TWO_OVER_PI / ax).sqrt() * (xx.cos() * a - z * xx.sin() * b);
        if x < 0.0 { -r } else { r }
    }
}

pub fn bessel_y0(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x < 8.0 {
        let y = x * x;
        let a = -2_957_821_389.0
            + y * (7_062_834_065.0
                + y * (-512_359_803.6
                    + y * (10_879_881.29 + y * (-86_327.927_57 + y * 228.462_273_3))));
        let b = 40_076_544_269.0
            + y * (745_249_964.8
                + y * (7_189_466.438 + y * (47_447.264_70 + y * (226.103_024_4 + y))));
        a / b + TWO_OVER_PI * bessel_j0(x) * x.ln()
    } else {
        let z = 8.0 / x;
        let y = z * z;
        let xx = x - core::f64::consts::FRAC_PI_4;
        let (a, b) = big_j0(y);
        (TWO_OVER_PI / x).sqrt() * (xx.sin() * a + z * xx.cos() * b)
    }
}

pub fn bessel_y1(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x < 8.0 {
        let y = x * x;
        let a = x
            * (-0.490_060_494_3e13
                + y * (0.127_527_439_0e13
                    + y * (-0.515_343_813_9e11
                        + y * (0.734_926_455_1e9
                            + y * (-0.423_792_272_6e7 + y * 0.851_193_793_5e4)))));
        let b = 0.249_958_057_0e14
            + y * (0.424_441_966_4e12
                + y * (0.373_365_036_7e10
                    + y * (0.224_590_400_2e8
                        + y * (0.102_042_605_0e6 + y * (0.354_963_288_5e3 + y)))));
        a / b + TWO_OVER_PI * (bessel_j1(x) * x.ln() - 1.0 / x)
    } else {
        let z = 8.0 / x;
        let y = z * z;
        let xx = x - 3.0 * core::f64::consts::FRAC_PI_4;
        let (a, b) = big_j1(y);
        (TWO_OVER_PI / x).sqrt() * (xx.sin() * a + z * xx.cos() * b)
    }
}

/// J de orden entero: hacia adelante si x > n (es estable); si no, la recurrencia de Miller
/// hacia atrás, normalizada con la suma J0 + 2 J2 + 2 J4 + … = 1.
pub fn bessel_jn(n: i32, x: f64) -> f64 {
    if n < 0 {
        let r = bessel_jn(-n, x);
        return if n % 2 != 0 { -r } else { r };
    }
    match n {
        0 => return bessel_j0(x),
        1 => return bessel_j1(x),
        _ => {}
    }
    let ax = x.abs();
    if ax == 0.0 {
        return 0.0;
    }
    let nf = f64::from(n);
    let tox = 2.0 / ax;
    let mut ans;
    if ax > nf {
        let (mut bjm, mut bj) = (bessel_j0(ax), bessel_j1(ax));
        for j in 1..n {
            let bjp = f64::from(j) * tox * bj - bjm;
            bjm = bj;
            bj = bjp;
        }
        ans = bj;
    } else {
        let m = 2 * ((n + (160.0 * nf).sqrt() as i32) / 2);
        let (mut jsum, mut bjp, mut sum, mut bj) = (false, 0.0f64, 0.0f64, 1.0f64);
        ans = 0.0;
        for j in (1..=m).rev() {
            let bjm = f64::from(j) * tox * bj - bjp;
            bjp = bj;
            bj = bjm;
            if bj.abs() > 1e10 {
                bj *= 1e-10;
                bjp *= 1e-10;
                ans *= 1e-10;
                sum *= 1e-10;
            }
            if jsum {
                sum += bj;
            }
            jsum = !jsum;
            if j == n {
                ans = bjp;
            }
        }
        sum = 2.0 * sum - bj;
        ans /= sum;
    }
    if x < 0.0 && n % 2 != 0 { -ans } else { ans }
}

/// Y de orden entero: hacia adelante desde Y0 e Y1 (siempre estable para Y).
pub fn bessel_yn(n: i32, x: f64) -> f64 {
    if n < 0 {
        let r = bessel_yn(-n, x);
        return if n % 2 != 0 { -r } else { r };
    }
    match n {
        0 => return bessel_y0(x),
        1 => return bessel_y1(x),
        _ => {}
    }
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    let tox = 2.0 / x;
    let (mut bym, mut by) = (bessel_y0(x), bessel_y1(x));
    for j in 1..n {
        let byp = f64::from(j) * tox * by - bym;
        bym = by;
        by = byp;
    }
    by
}

/// El exponente de `x` en base 2 (sin la mantisa), como `logb` de C.
pub fn logb_f64(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let bits = x.abs().to_bits();
    let e = ((bits >> 52) & 0x7ff) as i64;
    if e == 0 {
        // Subnormal: se escala por 2^54 y se descuenta.
        let scaled = (x.abs() * 2f64.powi(54)).to_bits();
        return (((scaled >> 52) & 0x7ff) as i64 - 1023 - 54) as f64;
    }
    (e - 1023) as f64
}

unsafe extern "win64" fn _j0(x: f64) -> f64 {
    bessel_j0(x)
}
unsafe extern "win64" fn _j1(x: f64) -> f64 {
    bessel_j1(x)
}
unsafe extern "win64" fn _jn(n: i32, x: f64) -> f64 {
    bessel_jn(n, x)
}
unsafe extern "win64" fn _y0(x: f64) -> f64 {
    bessel_y0(x)
}
unsafe extern "win64" fn _y1(x: f64) -> f64 {
    bessel_y1(x)
}
unsafe extern "win64" fn _yn(n: i32, x: f64) -> f64 {
    bessel_yn(n, x)
}
unsafe extern "win64" fn fdim(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        f64::NAN
    } else if x > y {
        x - y
    } else {
        0.0
    }
}
unsafe extern "win64" fn logb(x: f64) -> f64 {
    logb_f64(x)
}

// --- texto ---------------------------------------------------------------------------------------

/// `mbsrtowcs(dst, &src, len, state)`: como `mbstowcs`, pero deja `*src` donde quedó (nulo si
/// llegó al final). La "página de códigos" de vino es UTF-8.
unsafe extern "win64" fn mbsrtowcs(
    dst: *mut u16,
    src: *mut *const u8,
    len: usize,
    _state: u64,
) -> usize {
    if src.is_null() || (*src).is_null() {
        return 0;
    }
    let bytes = cstr(*src);
    let text = String::from_utf8_lossy(bytes);
    let wide = win::to_wide(&text);
    if dst.is_null() {
        return wide.len();
    }
    if wide.len() < len {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        *dst.add(wide.len()) = 0;
        *src = std::ptr::null();
        return wide.len();
    }
    // No entra todo: se copian `len` unidades y `*src` avanza lo que se consumió.
    let (mut units, mut consumed) = (0usize, 0usize);
    for ch in text.chars() {
        if units + ch.len_utf16() > len {
            break;
        }
        units += ch.len_utf16();
        consumed += ch.len_utf8();
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, units);
    *src = (*src).add(consumed.min(bytes.len()));
    units
}

fn assert_failed(msg: &str, file: &str, line: u32) -> ! {
    super::crt::flush_all();
    let _ = std::io::stderr()
        .write_all(format!("Assertion failed: {msg}, file {file}, line {line}\n").as_bytes());
    // Como `abort()` del CRT: código 3, sin las funciones de `atexit`.
    super::exit_now(3)
}

unsafe extern "win64" fn _assert(msg: *const u8, file: *const u8, line: u32) -> ! {
    assert_failed(
        &String::from_utf8_lossy(cstr(msg)),
        &String::from_utf8_lossy(cstr(file)),
        line,
    )
}

unsafe extern "win64" fn _wassert(msg: *const u16, file: *const u16, line: u32) -> ! {
    assert_failed(&wstr(msg), &wstr(file), line)
}

// --- shlwapi y userenv -----------------------------------------------------------------------

pub fn is_relative(p: &str) -> bool {
    let b = p.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    !(p.starts_with('\\') || p.starts_with('/') || drive)
}

unsafe extern "win64" fn PathIsRelativeA(p: *const u8) -> i32 {
    if p.is_null() {
        return 1;
    }
    i32::from(is_relative(&String::from_utf8_lossy(cstr(p))))
}

unsafe extern "win64" fn PathIsRelativeW(p: *const u16) -> i32 {
    if p.is_null() {
        return 1;
    }
    i32::from(is_relative(&wstr(p)))
}

fn profile_dir() -> String {
    win::to_windows(&std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

unsafe extern "win64" fn GetUserProfileDirectoryW(
    _token: u64,
    buf: *mut u16,
    size: *mut u32,
) -> i32 {
    if size.is_null() {
        state::set_last_error(error::INVALID_PARAMETER);
        return 0;
    }
    let dir = profile_dir();
    let need = win::to_wide(&dir).len() as u32 + 1;
    if buf.is_null() || *size < need {
        *size = need;
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return 0;
    }
    win::put_wide(buf, *size, &dir);
    *size = need;
    1
}

unsafe extern "win64" fn GetUserProfileDirectoryA(
    _token: u64,
    buf: *mut u8,
    size: *mut u32,
) -> i32 {
    if size.is_null() {
        state::set_last_error(error::INVALID_PARAMETER);
        return 0;
    }
    let dir = profile_dir();
    let need = dir.len() as u32 + 1;
    if buf.is_null() || *size < need {
        *size = need;
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return 0;
    }
    win::put_narrow(buf, *size, &dir);
    *size = need;
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-7 * b.abs().max(1.0)
    }

    #[test]
    fn bessel_contra_valores_conocidos() {
        // Abramowitz y Stegun, tabla 9.1.
        assert!(close(bessel_j0(1.0), 0.765_197_686_6));
        assert!(close(bessel_j1(1.0), 0.440_050_585_7));
        assert!(close(bessel_y0(1.0), 0.088_256_964_2));
        assert!(close(bessel_y1(1.0), -0.781_212_821_3));
        assert!(close(bessel_jn(2, 1.0), 0.114_903_484_9));
        assert!(close(bessel_yn(2, 1.0), -1.650_682_607));
        assert!(close(bessel_j0(10.0), -0.245_935_764_5));
        assert!(close(bessel_jn(5, 2.0), 0.007_039_629_8));
        assert!(close(bessel_jn(-1, 1.0), -0.440_050_585_7));
        assert!(bessel_y0(-1.0).is_nan());
        assert_eq!(bessel_y1(0.0), f64::NEG_INFINITY);
    }

    #[test]
    fn logb_y_fdim() {
        assert_eq!(logb_f64(8.0), 3.0);
        assert_eq!(logb_f64(-0.25), -2.0);
        assert_eq!(logb_f64(1.5), 0.0);
        assert_eq!(logb_f64(0.0), f64::NEG_INFINITY);
        assert_eq!(logb_f64(f64::MIN_POSITIVE / 4.0), -1024.0);
        // SAFETY: funciones puras.
        unsafe {
            assert_eq!(fdim(5.0, 3.0), 2.0);
            assert_eq!(fdim(3.0, 5.0), 0.0);
        }
    }

    #[test]
    fn setjmp_y_longjmp_vuelven_con_el_valor() {
        // SAFETY: todo pasa adentro de la función de ensamblador de prueba.
        unsafe {
            assert_eq!(vino_test_jump(42), 42);
            assert_eq!(vino_test_jump(0), 1); // longjmp(buf, 0) vuelve como 1
        }
    }

    #[test]
    fn mbsrtowcs_avanza_el_puntero() {
        let s = b"a\xc3\xb1o\0"; // "año"
        let mut src = s.as_ptr();
        let mut out = [0u16; 8];
        // SAFETY: buffers válidos.
        unsafe {
            assert_eq!(mbsrtowcs(std::ptr::null_mut(), &mut src, 0, 0), 3);
            assert_eq!(mbsrtowcs(out.as_mut_ptr(), &mut src, 2, 0), 2);
            assert_eq!(&out[..2], &[u16::from(b'a'), 0xF1]);
            assert_eq!(src, s.as_ptr().add(3));
            assert_eq!(mbsrtowcs(out.as_mut_ptr(), &mut src, 8, 0), 1);
            assert!(src.is_null());
        }
    }

    #[test]
    fn rutas_relativas() {
        assert!(is_relative("docs\\a.txt"));
        assert!(!is_relative("C:\\x"));
        assert!(!is_relative("\\\\server\\share"));
        assert!(!is_relative("/Documentos"));
    }

    #[test]
    fn carpeta_personal() {
        let mut size = 0u32;
        // SAFETY: sin buffer, solo pregunta el tamaño.
        let ok = unsafe { GetUserProfileDirectoryW(0, std::ptr::null_mut(), &mut size) };
        assert_eq!(ok, 0);
        assert!(size >= 3);
        let mut buf = vec![0u16; size as usize];
        // SAFETY: el buffer tiene el tamaño pedido.
        assert_eq!(
            unsafe { GetUserProfileDirectoryW(0, buf.as_mut_ptr(), &mut size) },
            1
        );
        assert!(win::from_wide(&buf).starts_with("C:"));
    }
}
