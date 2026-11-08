//! Heurística de ejecutables (ADR 0024), inspirada en **capa** (Mandiant): en vez de buscar un
//! malware conocido, mira **qué puede hacer** un programa por las funciones de Windows que
//! importa, y si está empaquetado o cifrado (la entropía de sus secciones).
//!
//! Cada indicio suma puntos; con muchos, el archivo es "sospechoso". Una combinación sola no
//! prueba nada (un depurador legítimo también lee la memoria de otros procesos): por eso la
//! heurística **avisa**, no pone en cuarentena. Lo seguro (hash conocido, regla de severidad
//! alta) sí va a cuarentena.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Desde cuántos puntos un archivo es sospechoso.
pub const SUSPICIOUS: u32 = 60;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub score: u32,
    /// Por qué (una línea por indicio).
    pub reasons: Vec<String>,
}

impl Verdict {
    pub fn suspicious(&self) -> bool {
        self.score >= SUSPICIOUS
    }

    fn add(&mut self, points: u32, why: &str) {
        self.score += points;
        self.reasons.push(format!("{why} (+{points})"));
    }
}

/// Entropía de Shannon en bits por byte (0 = todo igual, 8 = al azar). Un código normal anda por
/// 5–6,5; comprimido o cifrado, arriba de 7,2.
pub fn entropy(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let n = data.len() as f32;
    let mut h = 0.0f32;
    for &c in &counts {
        if c > 0 {
            let p = c as f32 / n;
            h -= p * log2(p);
        }
    }
    h
}

/// log2 sin `std` (el kernel no tiene libm): exponente + una serie para la mantisa.
fn log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000); // en [1, 2)
    // ln(m) con la serie de atanh: ln(m) = 2(t + t³/3 + t⁵/5 + …), t = (m-1)/(m+1).
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln = 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0))));
    exp as f32 + ln * core::f32::consts::LOG2_E
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Lo que importa de un PE: las secciones (nombre, entropía, ¿ejecutable?) y las funciones que
/// importa.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeFacts {
    pub sections: Vec<(String, f32, bool)>,
    pub imports: Vec<String>,
}

pub fn pe_facts(data: &[u8]) -> Option<PeFacts> {
    if data.get(0..2) != Some(b"MZ") {
        return None;
    }
    let pe = u32_at(data, 0x3C)? as usize;
    if data.get(pe..pe + 4) != Some(b"PE\0\0") {
        return None;
    }
    let nsec = usize::from(u16_at(data, pe + 6)?).min(96);
    let opt_size = usize::from(u16_at(data, pe + 20)?);
    let opt = pe + 24;
    let magic = u16_at(data, opt)?;
    // El directorio de imports: el segundo de la tabla de directorios.
    let dirs = opt + if magic == 0x20B { 112 } else { 96 };
    let import_rva = u32_at(data, dirs + 8).unwrap_or(0);
    let sec_table = opt + opt_size;
    let mut facts = PeFacts::default();
    let mut map: Vec<(u32, u32, u32)> = Vec::new(); // (rva, tamaño virtual, offset en el archivo)
    for i in 0..nsec {
        let s = sec_table + i * 40;
        let raw_name = data.get(s..s + 8)?;
        let name: String = raw_name
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        let vsize = u32_at(data, s + 8)?;
        let rva = u32_at(data, s + 12)?;
        let raw_size = u32_at(data, s + 16)? as usize;
        let raw_off = u32_at(data, s + 20)? as usize;
        let chars = u32_at(data, s + 36)?;
        let body = data
            .get(raw_off..raw_off.saturating_add(raw_size).min(data.len()))
            .unwrap_or(&[]);
        facts
            .sections
            .push((name, entropy(body), chars & 0x2000_0000 != 0));
        map.push((rva, vsize.max(raw_size as u32), raw_off as u32));
    }
    let to_off = |rva: u32| -> Option<usize> {
        map.iter()
            .find(|(r, sz, _)| rva >= *r && rva < r.saturating_add(*sz))
            .map(|(r, _, off)| (rva - r + off) as usize)
    };
    let wide = magic == 0x20B;
    if import_rva != 0
        && let Some(mut desc) = to_off(import_rva)
    {
        for _ in 0..256 {
            let thunk = u32_at(data, desc).unwrap_or(0);
            let name_rva = u32_at(data, desc + 12).unwrap_or(0);
            if name_rva == 0 {
                break;
            }
            let first = u32_at(data, desc + 16).unwrap_or(0);
            let table = if thunk != 0 { thunk } else { first };
            if let Some(mut t) = to_off(table) {
                for _ in 0..4096 {
                    let (entry, by_ordinal) = if wide {
                        let lo = u32_at(data, t).unwrap_or(0);
                        let hi = u32_at(data, t + 4).unwrap_or(0);
                        (lo, hi & 0x8000_0000 != 0)
                    } else {
                        let v = u32_at(data, t).unwrap_or(0);
                        (v, v & 0x8000_0000 != 0)
                    };
                    if entry == 0 && !by_ordinal {
                        break;
                    }
                    if !by_ordinal && let Some(off) = to_off(entry) {
                        let name: String = data
                            .get(off + 2..)
                            .unwrap_or(&[])
                            .iter()
                            .take(128)
                            .take_while(|&&b| b != 0)
                            .map(|&b| char::from(b))
                            .collect();
                        if !name.is_empty() {
                            facts.imports.push(name);
                        }
                    }
                    t += if wide { 8 } else { 4 };
                }
            }
            desc += 20;
        }
    }
    Some(facts)
}

fn has(imports: &[String], names: &[&str]) -> bool {
    names.iter().all(|n| {
        imports
            .iter()
            .any(|i| i == n || i.strip_suffix(['A', 'W']) == Some(n))
    })
}

fn has_any(imports: &[String], names: &[&str]) -> bool {
    names.iter().any(|n| has(imports, &[n]))
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Revisa un archivo. Solo mira ejecutables (PE y ELF); el resto da puntaje 0.
pub fn analyze(data: &[u8]) -> Verdict {
    let mut v = Verdict::default();
    if let Some(f) = pe_facts(data) {
        let im = &f.imports;
        if has(
            im,
            &["VirtualAllocEx", "WriteProcessMemory", "CreateRemoteThread"],
        ) {
            v.add(60, "inyecta código en otros procesos");
        } else if has(im, &["OpenProcess", "WriteProcessMemory"]) {
            v.add(25, "escribe en la memoria de otros procesos");
        }
        if has(im, &["SetWindowsHookEx"]) && has_any(im, &["GetAsyncKeyState", "GetKeyboardState"])
        {
            v.add(40, "registra lo que se tipea (keylogger)");
        }
        if has_any(im, &["CryptEncrypt", "BCryptEncrypt"])
            && has(im, &["FindFirstFile", "FindNextFile"])
            && has_any(im, &["DeleteFile", "MoveFileEx"])
        {
            v.add(
                40,
                "cifra archivos que recorre y borra los originales (ransomware)",
            );
        }
        if has(im, &["IsDebuggerPresent", "CheckRemoteDebuggerPresent"]) {
            v.add(10, "se fija si lo están depurando");
        }
        if has_any(im, &["URLDownloadToFile", "InternetOpenUrl"])
            && has_any(im, &["WinExec", "ShellExecute", "CreateProcess"])
        {
            v.add(20, "baja algo de internet y lo ejecuta");
        }
        if f.sections.iter().any(|(n, _, _)| n.starts_with("UPX")) {
            v.add(25, "está empaquetado con UPX");
        }
        if f.sections.iter().any(|(_, e, x)| *x && *e > 7.2) {
            v.add(25, "tiene código cifrado o comprimido (entropía muy alta)");
        }
        if im.len() <= 3 && f.sections.iter().any(|(_, e, _)| *e > 7.0) {
            v.add(20, "casi no importa funciones (las carga escondidas)");
        }
    } else if data.starts_with(b"\x7fELF") {
        if contains(data, b"stratum+tcp://") || contains(data, b"stratum+ssl://") {
            v.add(60, "se conecta a un pool de minería de criptomonedas");
        }
        if contains(data, b"xmrig") {
            v.add(40, "contiene el minero XMRig");
        }
        if contains(data, b"/etc/ld.so.preload") {
            v.add(40, "se instala en /etc/ld.so.preload (rootkit)");
        }
    }
    v
}

/// "sospechoso (85 puntos): inyecta código…; está empaquetado…"
pub fn describe(v: &Verdict) -> String {
    format!("sospechoso ({} puntos): {}", v.score, v.reasons.join("; ")).to_string()
}
