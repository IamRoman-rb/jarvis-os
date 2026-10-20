//! `printf` y familia, como el CRT de Microsoft (`ucrtbase`/`msvcrt`).
//!
//! Los argumentos llegan como una `va_list` de Windows x86-64: un puntero a lugares de 8 bytes
//! seguidos, uno por argumento (los `double` van como sus 64 bits; los `float` ya llegan
//! convertidos a `double`). El formato se recorre una vez: banderas, ancho, precisión, tamaño y
//! conversión.
//!
//! Diferencias de Windows con glibc que se respetan: `%ls`/`%S`/`%ws` son textos UTF-16, `%I64d`
//! es de 64 bits, `long` es de 32 bits (`%ld` corta a 32) y `%p` sale en 16 dígitos hexa en
//! mayúsculas.

/// De dónde salen los argumentos.
pub trait Args {
    /// El próximo lugar de 8 bytes.
    fn next(&mut self) -> u64;
    /// El texto de bytes en `ptr` (hasta el cero o `max` bytes).
    fn str(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8>;
    /// El texto UTF-16 en `ptr` (hasta el cero o `max` valores), en UTF-8.
    fn wstr(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8>;
}

/// La `va_list` de verdad, sobre la memoria del programa.
pub struct VaList(pub *const u64);

impl Args for VaList {
    fn next(&mut self) -> u64 {
        // SAFETY: el programa pasó un argumento por cada conversión del formato (si no, es un
        // bug del programa, igual que en Windows).
        let v = unsafe { self.0.read_unaligned() };
        // SAFETY: el siguiente argumento está 8 bytes después.
        self.0 = unsafe { self.0.add(1) };
        v
    }
    fn str(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8> {
        let p = ptr as *const u8;
        let mut out = Vec::new();
        // SAFETY: el argumento de `%s` apunta a un texto terminado en cero (o de `max` bytes).
        while max.is_none_or(|m| out.len() < m) {
            let c = unsafe { *p.add(out.len()) };
            if c == 0 {
                break;
            }
            out.push(c);
        }
        out
    }
    fn wstr(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8> {
        let p = ptr as *const u16;
        let mut w = Vec::new();
        // SAFETY: ídem, en UTF-16.
        while max.is_none_or(|m| w.len() < m) {
            let c = unsafe { *p.add(w.len()) };
            if c == 0 {
                break;
            }
            w.push(c);
        }
        String::from_utf16_lossy(&w).into_bytes()
    }
}

#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Size {
    Default,
    Char,
    Short,
    /// `l`: 32 bits en Windows (salvo en `%ls`, `%lc`, `%lf`).
    Long,
    /// `ll`, `I64`, `j`, `z`, `t`, `I`: 64 bits.
    Wide64,
    /// `I32`.
    Wide32,
    /// `w` (el `%ws` de Microsoft).
    WideChar,
}

/// Arma el texto. `wide` dice si `%s` y `%c` son de 16 bits por defecto (las funciones `w…`).
pub fn format(fmt: &[u8], wide: bool, args: &mut dyn Args) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        let mut s = Spec::default();
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => s.left = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                b'0' => s.zero = true,
                _ => break,
            }
            i += 1;
        }
        if fmt.get(i) == Some(&b'*') {
            i += 1;
            let w = args.next() as i32;
            if w < 0 {
                s.left = true;
            }
            s.width = w.unsigned_abs() as usize;
        } else {
            while let Some(d) = fmt.get(i).filter(|d| d.is_ascii_digit()) {
                s.width = s.width * 10 + (d - b'0') as usize;
                i += 1;
            }
        }
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            if fmt.get(i) == Some(&b'*') {
                i += 1;
                let p = args.next() as i32;
                s.prec = (p >= 0).then_some(p as usize);
            } else {
                let mut p = 0;
                while let Some(d) = fmt.get(i).filter(|d| d.is_ascii_digit()) {
                    p = p * 10 + (d - b'0') as usize;
                    i += 1;
                }
                s.prec = Some(p);
            }
        }
        let rest = &fmt[i..];
        let (size, n) = if rest.starts_with(b"hh") {
            (Size::Char, 2)
        } else if rest.starts_with(b"ll") || rest.starts_with(b"I64") {
            (Size::Wide64, if rest[0] == b'I' { 3 } else { 2 })
        } else if rest.starts_with(b"I32") {
            (Size::Wide32, 3)
        } else {
            match rest.first() {
                Some(b'h') => (Size::Short, 1),
                Some(b'l') => (Size::Long, 1),
                Some(b'j' | b'z' | b't' | b'I') => (Size::Wide64, 1),
                Some(b'L') => (Size::Default, 1),
                Some(b'w') => (Size::WideChar, 1),
                _ => (Size::Default, 0),
            }
        };
        i += n;
        let Some(&conv) = fmt.get(i) else {
            break;
        };
        i += 1;
        let body: Vec<u8> = match conv {
            b'%' => {
                out.push(b'%');
                continue;
            }
            b'd' | b'i' => {
                let v = signed(args.next(), size);
                let mut digits = v.unsigned_abs().to_string().into_bytes();
                apply_int_precision(&mut digits, s.prec);
                let sign: &[u8] = if v < 0 {
                    b"-"
                } else if s.plus {
                    b"+"
                } else if s.space {
                    b" "
                } else {
                    b""
                };
                pad_number(&s, sign, &digits, s.prec.is_none())
            }
            b'u' | b'x' | b'X' | b'o' => {
                let v = unsigned(args.next(), size);
                let mut digits = match conv {
                    b'u' => v.to_string(),
                    b'x' => format!("{v:x}"),
                    b'X' => format!("{v:X}"),
                    _ => format!("{v:o}"),
                }
                .into_bytes();
                apply_int_precision(&mut digits, s.prec);
                let prefix: &[u8] = match conv {
                    b'x' if s.alt && v != 0 => b"0x",
                    b'X' if s.alt && v != 0 => b"0X",
                    b'o' if s.alt && digits[0] != b'0' => b"0",
                    _ => b"",
                };
                pad_number(&s, prefix, &digits, s.prec.is_none())
            }
            b'p' => {
                let v = args.next();
                let digits = format!("{v:016X}").into_bytes();
                pad(&s, &digits)
            }
            b'c' | b'C' => {
                let v = args.next();
                let w = wide_text(conv == b'C', size, wide);
                let b = if w {
                    char::from_u32(v as u16 as u32)
                        .unwrap_or('\u{fffd}')
                        .to_string()
                        .into_bytes()
                } else {
                    vec![v as u8]
                };
                pad(&s, &b)
            }
            b's' | b'S' => {
                let p = args.next();
                let w = wide_text(conv == b'S', size, wide);
                let b = if p == 0 {
                    b"(null)".to_vec()
                } else if w {
                    args.wstr(p, s.prec)
                } else {
                    args.str(p, s.prec)
                };
                pad(&s, &b)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let v = f64::from_bits(args.next());
                let (sign, digits) = float(v, conv, &s);
                pad_number(&s, sign, &digits, v.is_finite())
            }
            b'n' => {
                // Escribir la cantidad de caracteres en un puntero: el CRT de Microsoft lo tiene
                // apagado por seguridad (y `vino` también).
                let _ = args.next();
                continue;
            }
            other => {
                out.push(b'%');
                out.push(other);
                continue;
            }
        };
        out.extend_from_slice(&body);
    }
    out
}

fn wide_text(upper: bool, size: Size, wide: bool) -> bool {
    match size {
        Size::Long | Size::WideChar => true,
        Size::Short => false,
        // `%S` es "el otro ancho": en printf, UTF-16; en wprintf, de bytes.
        _ => upper != wide,
    }
}

fn signed(v: u64, size: Size) -> i64 {
    match size {
        Size::Char => v as i8 as i64,
        Size::Short => v as i16 as i64,
        Size::Wide64 => v as i64,
        _ => v as i32 as i64,
    }
}

fn unsigned(v: u64, size: Size) -> u64 {
    match size {
        Size::Char => v as u8 as u64,
        Size::Short => v as u16 as u64,
        Size::Wide64 => v,
        _ => v as u32 as u64,
    }
}

fn apply_int_precision(digits: &mut Vec<u8>, prec: Option<usize>) {
    if let Some(p) = prec {
        if p == 0 && digits == b"0" {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, b'0');
        }
    }
}

/// Un número con signo o prefijo y el relleno: ceros entre el signo y los dígitos (`%05d`), o
/// espacios a la izquierda o a la derecha.
fn pad_number(s: &Spec, sign: &[u8], digits: &[u8], zero_ok: bool) -> Vec<u8> {
    let len = sign.len() + digits.len();
    let mut out = Vec::with_capacity(s.width.max(len));
    let fill = s.width.saturating_sub(len);
    if s.left {
        out.extend_from_slice(sign);
        out.extend_from_slice(digits);
        out.resize(out.len() + fill, b' ');
    } else if s.zero && zero_ok {
        out.extend_from_slice(sign);
        out.resize(out.len() + fill, b'0');
        out.extend_from_slice(digits);
    } else {
        out.resize(fill, b' ');
        out.extend_from_slice(sign);
        out.extend_from_slice(digits);
    }
    out
}

fn pad(s: &Spec, b: &[u8]) -> Vec<u8> {
    let plain = Spec {
        left: s.left,
        width: s.width,
        ..Default::default()
    };
    pad_number(&plain, b"", b, false)
}

/// Un real: el signo aparte y los dígitos. Rust redondea bien (al más cercano, como el CRT
/// desde Visual Studio 2015); lo que cambia es la forma del exponente (`e+04`, con al menos
/// dos dígitos) y las reglas de `%g`.
fn float(v: f64, conv: u8, s: &Spec) -> (&'static [u8], Vec<u8>) {
    let upper = conv.is_ascii_uppercase();
    let sign: &'static [u8] = if v.is_sign_negative() && !v.is_nan() {
        b"-"
    } else if s.plus {
        b"+"
    } else if s.space {
        b" "
    } else {
        b""
    };
    let a = v.abs();
    if !a.is_finite() {
        let t = if a.is_nan() { "nan" } else { "inf" };
        let t = if upper {
            t.to_uppercase()
        } else {
            t.to_string()
        };
        return (sign, t.into_bytes());
    }
    let prec = s.prec.unwrap_or(6);
    let text = match conv.to_ascii_lowercase() {
        b'f' => fixed(a, prec, s.alt),
        b'e' => exp(a, prec, s.alt),
        b'a' => exp(a, prec, s.alt),
        _ => {
            // %g: con P cifras significativas; exponente X de la forma %e. Si P > X >= -4,
            // como %f con P-1-X decimales; si no, como %e con P-1. Sin ceros de más al final
            // (salvo con #).
            let p = if prec == 0 { 1 } else { prec };
            let x = if a == 0.0 {
                0
            } else {
                let e = exp(a, p - 1, false);
                e.rsplit('e')
                    .next()
                    .and_then(|n| n.parse::<i32>().ok())
                    .unwrap_or(0)
            };
            let mut t = if (p as i32) > x && x >= -4 {
                fixed(a, (p as i32 - 1 - x) as usize, s.alt)
            } else {
                exp(a, p - 1, s.alt)
            };
            if !s.alt {
                t = strip_zeros(&t);
            }
            t
        }
    };
    let text = if upper { text.to_uppercase() } else { text };
    (sign, text.into_bytes())
}

fn fixed(a: f64, prec: usize, alt: bool) -> String {
    let mut t = format!("{a:.prec$}");
    if alt && prec == 0 {
        t.push('.');
    }
    t
}

fn exp(a: f64, prec: usize, alt: bool) -> String {
    let t = format!("{a:.prec$e}");
    let (mant, e) = t.split_once('e').unwrap_or((&t, "0"));
    let n: i32 = e.parse().unwrap_or(0);
    let mut mant = mant.to_string();
    if alt && prec == 0 {
        mant.push('.');
    }
    format!(
        "{mant}e{}{:02}",
        if n < 0 { '-' } else { '+' },
        n.unsigned_abs()
    )
}

fn strip_zeros(t: &str) -> String {
    let (mant, e) = match t.find('e') {
        Some(i) => (&t[..i], &t[i..]),
        None => (t, ""),
    };
    let mant = if mant.contains('.') {
        mant.trim_end_matches('0').trim_end_matches('.')
    } else {
        mant
    };
    format!("{mant}{e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Argumentos de mentira: números, y los textos en una tabla (el "puntero" es su índice + 1).
    struct Fake {
        vals: Vec<u64>,
        texts: Vec<&'static str>,
    }

    impl Args for Fake {
        fn next(&mut self) -> u64 {
            self.vals.remove(0)
        }
        fn str(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8> {
            let t = self.texts[ptr as usize - 1].as_bytes();
            t[..max.unwrap_or(t.len()).min(t.len())].to_vec()
        }
        fn wstr(&mut self, ptr: u64, max: Option<usize>) -> Vec<u8> {
            self.str(ptr, max)
        }
    }

    fn f(fmt: &str, vals: &[u64]) -> String {
        f_t(fmt, vals, &[])
    }

    fn f_t(fmt: &str, vals: &[u64], texts: &[&'static str]) -> String {
        let mut a = Fake {
            vals: vals.to_vec(),
            texts: texts.to_vec(),
        };
        String::from_utf8(format(fmt.as_bytes(), false, &mut a)).unwrap()
    }

    fn d(x: f64) -> u64 {
        x.to_bits()
    }

    #[test]
    fn enteros_como_el_crt() {
        // Los de hola-win.c.
        assert_eq!(
            f(
                "enteros: %d %5d|%-5d|%05d %u %x %X %lld",
                &[
                    -42i64 as u64,
                    7,
                    7,
                    7,
                    3_000_000_000,
                    255,
                    255,
                    1_234_567_890_123
                ]
            ),
            "enteros: -42     7|7    |00007 3000000000 ff FF 1234567890123"
        );
        assert_eq!(f("%ld", &[0x1_0000_0005]), "5", "long es de 32 bits");
        assert_eq!(
            f("%I64u %I32d", &[u64::MAX, u32::MAX as u64]),
            "18446744073709551615 -1"
        );
        assert_eq!(f("%#x %#o %+d % d", &[255, 8, 3, 3]), "0xff 010 +3  3");
        assert_eq!(
            f("%.3d|%.0d|%*d|%-*d|", &[5, 0, 4, 1, 3, 2]),
            "005||   1|2  |"
        );
        assert_eq!(f("%hhd %hu", &[0x1ff, 0x1_0002]), "-1 2");
        assert_eq!(f("%p", &[0x1400]), "0000000000001400");
        assert_eq!(f("100%%", &[]), "100%");
    }

    #[test]
    fn textos_y_caracteres() {
        assert_eq!(
            f_t(
                "textos: [%s] [%10s] [%-6s] %c%c",
                &[1, 2, 3, b'O' as u64, b'K' as u64],
                &["jarvis", "der", "izq"]
            ),
            "textos: [jarvis] [       der] [izq   ] OK"
        );
        assert_eq!(f_t("%.3s|%s", &[1, 0], &["abcdef"]), "abc|(null)");
    }

    // 3.14159 es el valor de hola-win.c, no una aproximación de π.
    #[allow(clippy::approx_constant)]
    #[test]
    fn reales_como_el_crt() {
        assert_eq!(
            f(
                "reales: %.2f %8.3f %e %g",
                &[d(3.14159), d(-2.5), d(12345.678), d(0.0001)]
            ),
            "reales: 3.14   -2.500 1.234568e+04 0.0001"
        );
        assert_eq!(
            f(
                "raiz de 2 = %.6f, seno(1) = %.4f, 2^10 = %.0f",
                &[d(2f64.sqrt()), d(1f64.sin()), d(1024.0)]
            ),
            "raiz de 2 = 1.414214, seno(1) = 0.8415, 2^10 = 1024"
        );
        assert_eq!(
            f("%g %g %g %g", &[d(100000.0), d(1e6), d(1.5e-5), d(0.0)]),
            "100000 1e+06 1.5e-05 0"
        );
        assert_eq!(f("%G %E", &[d(1e-10), d(-0.5)]), "1E-10 -5.000000E-01");
        assert_eq!(
            f(
                "%f %F %08.2f",
                &[d(f64::INFINITY), d(f64::NAN), d(-3.14159)]
            ),
            "inf NAN -0003.14"
        );
        assert_eq!(
            f("%#.0f %.3g %#g", &[d(3.0), d(3.14159), d(2.0)]),
            "3. 3.14 2.00000"
        );
    }
}
