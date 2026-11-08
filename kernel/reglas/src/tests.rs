//! Las reglas y la heurística con muestras **inofensivas** armadas acá (nada de malware real).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::heuristica::{analyze, entropy, pe_facts};
use super::*;

fn scan(src: &str, data: &[u8]) -> Vec<String> {
    let s = Scanner::new(compile(src).unwrap());
    s.scan(data).into_iter().map(|m| m.rule).collect()
}

#[test]
fn cadenas_de_texto_nocase_y_wide() {
    let src = r#"
        rule Texto { strings: $a = "Hola Mundo" condition: $a }
        rule SinMayus { strings: $a = "stratum+tcp://" nocase condition: $a }
        rule Ancho { strings: $a = "PASSWORD" wide condition: $a }
        rule Ambos { strings: $a = "clave" wide ascii condition: #a == 2 }
    "#;
    assert_eq!(scan(src, b"xx Hola Mundo xx"), ["Texto"]);
    assert!(scan(src, b"hola mundo").is_empty());
    assert_eq!(scan(src, b"conectar a STRATUM+TCP://pool"), ["SinMayus"]);
    let w: Vec<u8> = "PASSWORD".bytes().flat_map(|b| [b, 0]).collect();
    assert_eq!(scan(src, &w), ["Ancho"]);
    let mut both = b"clave ".to_vec();
    both.extend("clave".bytes().flat_map(|b| [b, 0]));
    assert_eq!(scan(src, &both), ["Ambos"]);
}

#[test]
fn hexadecimal_con_comodines_y_saltos() {
    let src = r#"rule H { strings: $h = { 4D 5A ?? 00 [2-4] 50 4? } condition: $h at 0 }"#;
    assert_eq!(
        scan(src, &[0x4D, 0x5A, 0x90, 0x00, 1, 2, 3, 0x50, 0x45]),
        ["H"]
    );
    assert_eq!(
        scan(src, &[0x4D, 0x5A, 0x90, 0x00, 1, 2, 0x50, 0x4F]),
        ["H"]
    );
    // Salto demasiado largo, o un nibble que no coincide.
    assert!(scan(src, &[0x4D, 0x5A, 0x90, 0x00, 1, 2, 3, 4, 5, 0x50, 0x45]).is_empty());
    assert!(scan(src, &[0x4D, 0x5A, 0x90, 0x00, 1, 2, 0x50, 0x55]).is_empty());
    // No al principio: `at 0` falla.
    assert!(scan(src, &[0, 0x4D, 0x5A, 0x90, 0x00, 1, 2, 0x50, 0x45]).is_empty());
}

#[test]
fn condiciones() {
    let src = r#"
        rule Cuantas { strings: $a = "x1" condition: #a >= 3 }
        rule Algunas { strings: $a = "uno" $b = "dos" $c = "tres" condition: 2 of them }
        rule Todas { strings: $p1 = "aa" $p2 = "bb" $q = "zz" condition: all of ($p*) and not $q }
        rule Mz { condition: uint16(0) == 0x5A4D and filesize < 1KB }
        rule Rango { strings: $a = "fin" condition: $a in (0..4) or uint32be(0) == 0x7F454C46 }
    "#;
    assert_eq!(scan(src, b"x1 x1 x1"), ["Cuantas"]);
    assert_eq!(scan(src, b"uno y tres"), ["Algunas"]);
    assert_eq!(scan(src, b"aa bb"), ["Todas"]);
    assert!(!scan(src, b"aa bb zz").contains(&"Todas".into()));
    assert_eq!(scan(src, b"MZ chico"), ["Mz"]);
    let mut big = b"MZ".to_vec();
    big.resize(2000, 0);
    assert!(scan(src, &big).is_empty());
    assert_eq!(scan(src, b"fin del archivo"), ["Rango"]);
    assert_eq!(scan(src, b"\x7fELF..."), ["Rango"]);
}

#[test]
fn meta_y_errores() {
    let rules = compile(
        r#"import "pe"
        rule A : malware minero {
            meta:
                description = "un minero"
                severity = "alta"
                version = 2
            strings: $a = "x"
            condition: $a
        }"#,
    )
    .unwrap();
    assert_eq!(rules[0].tags, ["malware", "minero"]);
    assert_eq!(rules[0].meta("description"), Some("un minero"));
    assert_eq!(rules[0].severity(), "alta");
    for bad in [
        "rule { condition: true }",
        "rule A { strings: $a = \"x\" condition: $b }",
        "rule A { strings: $a = { 4D ( 5A | 4D ) } condition: $a }",
        "rule A { strings: $a = { [2] 4D } condition: $a }",
        "rule A { condition: true } rule A { condition: true }",
        "rule A { strings: $a = \"sin cerrar }",
    ] {
        assert!(compile(bad).is_err(), "{bad}");
    }
}

#[test]
fn muchas_reglas_una_pasada() {
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!(
            "rule R{i} {{ strings: $a = \"patron-{i}-fin\" condition: $a }}\n"
        ));
    }
    let s = Scanner::new(compile(&src).unwrap());
    let found: Vec<String> = s
        .scan(b"... patron-7-fin ... patron-250-fin ...")
        .into_iter()
        .map(|m| m.rule)
        .collect();
    assert_eq!(found, ["R7", "R250"]);
}

/// Un PE de 64 bits mínimo con una sección y los imports que se le pidan.
fn tiny_pe(imports: &[&str], section_data: Option<&[u8]>) -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    d[0..2].copy_from_slice(b"MZ");
    d[0x3C] = 0x80;
    let pe = 0x80;
    d[pe..pe + 4].copy_from_slice(b"PE\0\0");
    d[pe + 4..pe + 6].copy_from_slice(&0x8664u16.to_le_bytes());
    d[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes()); // una sección
    d[pe + 20..pe + 22].copy_from_slice(&240u16.to_le_bytes());
    let opt = pe + 24;
    d[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
    let dirs = opt + 112;
    d[dirs + 8..dirs + 12].copy_from_slice(&0x1000u32.to_le_bytes());
    let sec = opt + 240;
    d[sec..sec + 5].copy_from_slice(b".text");
    d[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // tamaño virtual
    d[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // RVA
    d[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes()); // tamaño en el archivo
    d[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // offset
    d[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes()); // código, ejecutable
    // La sección: el descriptor de imports, los nombres y la tabla.
    let base = 0x200;
    let put32 =
        |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put32(&mut d, base, 0x1100); // OriginalFirstThunk
    put32(&mut d, base + 12, 0x1180); // nombre de la DLL
    put32(&mut d, base + 16, 0x1100); // FirstThunk
    d[base + 0x180..base + 0x18C].copy_from_slice(b"kernel32.dll");
    let mut name_at = 0x40;
    for (i, f) in imports.iter().enumerate() {
        let rva = 0x1000 + name_at as u32;
        d[base + name_at + 2..base + name_at + 2 + f.len()].copy_from_slice(f.as_bytes());
        d[base + 0x100 + i * 8..base + 0x100 + i * 8 + 4].copy_from_slice(&rva.to_le_bytes());
        name_at += 2 + f.len() + 1;
    }
    if let Some(body) = section_data {
        // Relleno de la parte de abajo (no pisa los imports).
        let start = base + 0x1A0;
        for (i, b) in body.iter().take(0x60).enumerate() {
            d[start + i] = *b;
        }
    }
    d
}

#[test]
fn lee_los_imports_de_un_pe() {
    let pe = tiny_pe(&["GetStdHandle", "WriteConsoleW"], None);
    let f = pe_facts(&pe).unwrap();
    assert_eq!(f.imports, ["GetStdHandle", "WriteConsoleW"]);
    assert_eq!(f.sections.len(), 1);
    assert_eq!(f.sections[0].0, ".text");
    assert!(f.sections[0].2);
    assert!(!analyze(&pe).suspicious());
}

#[test]
fn heuristica_de_inyeccion_y_keylogger() {
    let pe = tiny_pe(
        &[
            "OpenProcess",
            "VirtualAllocEx",
            "WriteProcessMemory",
            "CreateRemoteThread",
            "SetWindowsHookExW",
            "GetAsyncKeyState",
        ],
        None,
    );
    let v = analyze(&pe);
    assert!(v.suspicious(), "{v:?}");
    assert_eq!(v.score, 100);
    assert!(v.reasons[0].contains("inyecta"));
    let elf = b"\x7fELF.... conectando a stratum+tcp://pool.example:3333 con xmrig";
    let v = analyze(elf);
    assert_eq!(v.score, 100);
    assert!(!analyze(b"\x7fELF un programa comun").suspicious());
    assert!(!analyze(b"texto cualquiera").suspicious());
}

#[test]
fn entropia() {
    assert!(entropy(&[0u8; 1000]) < 0.01);
    let mut x = 0x1234_5678u32;
    let random: Vec<u8> = (0..65536)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let e = entropy(&random);
    assert!(e > 7.9 && e <= 8.01, "{e}");
    let text = b"hola hola hola mundo mundo";
    let e = entropy(text);
    assert!(e > 1.5 && e < 3.5, "{e}");
}
