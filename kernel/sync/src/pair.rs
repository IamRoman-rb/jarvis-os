//! Emparejado: un código de 20 caracteres que se genera en una máquina y se escribe en la otra.
//! Con HKDF-SHA256 salen de él el id de grupo (lo que ve el relé) y la clave del cifrado.

use alloc::string::String;
use hkdf::Hkdf;
use sha2::Sha256;

/// Sin 0/O ni 1/I, que se confunden al copiarlos a mano: 32 símbolos, 5 bits cada uno.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 20;
const SALT: &[u8] = b"jarvis-sync v1";

/// Un código nuevo, "ABCDE-FGHJK-LMNPQ-RSTUV", a partir de 20 bytes al azar (100 bits).
pub fn new_code(random: &[u8; CODE_LEN]) -> String {
    let mut s = String::new();
    for (i, b) in random.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            s.push('-');
        }
        s.push(ALPHABET[(b % 32) as usize] as char);
    }
    s
}

/// El código sin guiones ni espacios y en mayúsculas, o `None` si no es válido.
pub fn normalize(code: &str) -> Option<String> {
    let s: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (s.len() == CODE_LEN && s.bytes().all(|b| ALPHABET.contains(&b))).then_some(s)
}

/// El grupo que forman las máquinas con el mismo código.
#[derive(Clone)]
pub struct Group {
    pub id: [u8; 16],
    pub(crate) key: [u8; 32],
}

impl Group {
    pub fn from_code(code: &str) -> Option<Group> {
        let code = normalize(code)?;
        let hk = Hkdf::<Sha256>::new(Some(SALT), code.as_bytes());
        let mut g = Group {
            id: [0; 16],
            key: [0; 32],
        };
        hk.expand(b"grupo", &mut g.id).ok()?;
        hk.expand(b"clave", &mut g.key).ok()?;
        Some(g)
    }

    /// El id en hexadecimal (para el relé y los logs).
    pub fn id_hex(&self) -> String {
        crate::engine::hex(&self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codigo_y_grupo() {
        let c = new_code(&[7; 20]);
        assert_eq!(c.len(), 23);
        assert_eq!(normalize(&c.to_lowercase()).unwrap().len(), 20);
        assert!(normalize("ABC").is_none());
        assert!(normalize("OOOOO-OOOOO-OOOOO-OOOOO").is_none(), "sin O");
        let a = Group::from_code(&c).unwrap();
        let b = Group::from_code(&c.replace('-', " ").to_lowercase()).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.key, b.key);
        assert_ne!(&a.id[..], &a.key[..16], "id y clave distintos");
        let other = Group::from_code(&new_code(&[8; 20])).unwrap();
        assert_ne!(a.id, other.id);
    }
}
