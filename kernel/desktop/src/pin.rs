//! El PIN de desbloqueo, guardado sin el texto: PBKDF2-HMAC-SHA256 con una sal al azar.
//!
//! En `/Sistema/config.ini` queda `pin_hash=pbkdf2-sha256$<vueltas>$<sal>$<hash>` (sal y hash en
//! hexadecimal). Para comprobar un PIN se repite la cuenta con la misma sal y se comparan los
//! hashes; el PIN escrito nunca se guarda ni se compara como texto.
//!
//! Por qué PBKDF2 y no Argon2: las crates (`pbkdf2`, `sha2`) ya están en el kernel por el Wi-Fi y
//! el TLS, y corren sin SSE. Lo que lo hace "lento" son las vueltas: cada intento cuesta
//! [`ITERATIONS`] HMAC. Un PIN de hasta 8 dígitos igual se puede adivinar probando todos si
//! alguien se lleva el disco; el hash evita que se lea a simple vista y la sal, que se use una
//! tabla precalculada o que dos máquinas con el mismo PIN tengan la misma línea.
//!
//! La sal sale de [`salt`]: el kernel le pasa azar del arranque con [`add_entropy`]
//! (`kernel/entropy.rs`); sin eso (los tests) solo queda un contador, que alcanza para que sea
//! distinta cada vez, pero no es impredecible.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// Vueltas de PBKDF2 para los PIN nuevos. Las de cada hash quedan guardadas con él: subir este
/// número no invalida los PIN viejos.
pub const ITERATIONS: u32 = 100_000;
/// Más vueltas que esto no se aceptan al leer (un archivo editado a mano no puede colgar el
/// desbloqueo).
const MAX_ITERATIONS: u32 = 10_000_000;
const SCHEME: &str = "pbkdf2-sha256";

/// Un PIN hasheado. No hay forma de volver al texto.
#[derive(Clone, PartialEq, Eq)]
pub struct PinHash {
    iterations: u32,
    salt: [u8; 16],
    hash: [u8; 32],
}

// Sin el hash en los logs ni en los `assert` que fallan.
impl core::fmt::Debug for PinHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PinHash(..)")
    }
}

fn derive(pin: &str, salt: &[u8; 16], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(pin.as_bytes(), salt, iterations, &mut out);
    out
}

impl PinHash {
    /// Hashea `pin` con la sal dada y las vueltas de [`ITERATIONS`].
    pub fn with_salt(pin: &str, salt: [u8; 16]) -> PinHash {
        PinHash {
            iterations: ITERATIONS,
            salt,
            hash: derive(pin, &salt, ITERATIONS),
        }
    }

    /// ¿`pin` es el PIN guardado? Compara los hashes sin cortar en la primera diferencia.
    pub fn verify(&self, pin: &str) -> bool {
        let h = derive(pin, &self.salt, self.iterations);
        h.iter()
            .zip(self.hash.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    /// `pbkdf2-sha256$100000$<sal>$<hash>`
    pub fn encode(&self) -> String {
        format!(
            "{SCHEME}${}${}${}",
            self.iterations,
            hex(&self.salt),
            hex(&self.hash)
        )
    }

    /// Lo contrario de [`encode`](Self::encode). `None` si la línea está rota.
    pub fn decode(s: &str) -> Option<PinHash> {
        let mut parts = s.trim().split('$');
        if parts.next()? != SCHEME {
            return None;
        }
        let iterations = parts
            .next()?
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=MAX_ITERATIONS).contains(n))?;
        let salt = unhex(parts.next()?)?;
        let hash = unhex(parts.next()?)?;
        parts.next().is_none().then_some(PinHash {
            iterations,
            salt,
            hash,
        })
    }
}

/// Hashea un PIN nuevo con una sal de [`salt`]. Vacío = sin PIN (`None`).
pub fn hash_pin(pin: &str, extra: &[u8]) -> Option<PinHash> {
    (!pin.is_empty()).then(|| PinHash::with_salt(pin, salt(extra)))
}

/// Azar del kernel (32 bytes), en cuatro atómicos: el escritorio no tiene locks ni `unsafe`.
static SEED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// Cuántas sales se generaron: hace que dos sales seguidas nunca sean iguales.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn seed_bytes() -> [u8; 32] {
    let mut s = [0u8; 32];
    for (chunk, w) in s.as_chunks_mut::<8>().0.iter_mut().zip(SEED.iter()) {
        *chunk = w.load(Ordering::Relaxed).to_le_bytes();
    }
    s
}

/// Mezcla bytes al azar en la semilla de las sales (el kernel lo llama al arrancar).
pub fn add_entropy(data: &[u8]) {
    let mixed = Sha256::new()
        .chain_update(seed_bytes())
        .chain_update(data)
        .finalize();
    for (chunk, w) in mixed.as_chunks::<8>().0.iter().zip(SEED.iter()) {
        w.store(u64::from_le_bytes(*chunk), Ordering::Relaxed);
    }
}

/// Una sal nueva de 16 bytes: SHA-256 de la semilla, un contador y `extra` (la hora, el
/// usuario: lo que el llamador tenga a mano).
pub fn salt(extra: &[u8]) -> [u8; 16] {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let h = Sha256::new()
        .chain_update(b"jarvis-pin-sal")
        .chain_update(seed_bytes())
        .chain_update(n.to_le_bytes())
        .chain_update(extra)
        .finalize();
    let mut s = [0u8; 16];
    s.copy_from_slice(&h[..16]);
    s
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifica_solo_el_pin_correcto() {
        let h = hash_pin("1234", b"").expect("no vacío");
        assert!(h.verify("1234"));
        assert!(!h.verify("1235"));
        assert!(!h.verify(""));
        assert!(hash_pin("", b"").is_none(), "vacío = sin PIN");
    }

    #[test]
    fn la_sal_cambia_y_el_texto_no_queda() {
        let a = hash_pin("1234", b"").expect("no vacío");
        let b = hash_pin("1234", b"").expect("no vacío");
        assert_ne!(a, b, "mismo PIN, otra sal, otro hash");
        let line = a.encode();
        assert!(!line.contains("1234"));
        assert!(!format!("{a:?}").contains(&hex(&a.hash)));
        assert_eq!(PinHash::decode(&line), Some(a));
    }

    #[test]
    fn vector_conocido_de_pbkdf2_sha256() {
        // RFC 7914, sección 11: P="passwd", S="salt", c=1 (los primeros 32 bytes).
        let mut out = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(b"passwd", b"salt", 1, &mut out);
        assert_eq!(
            hex(&out),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
    }

    #[test]
    fn lineas_rotas_no_se_aceptan() {
        let ok = hash_pin("1", b"").expect("no vacío").encode();
        assert!(PinHash::decode(&ok).is_some());
        for bad in [
            "",
            "1234",
            "md5$1$00$00",
            &ok.replace("pbkdf2-sha256$100000", "pbkdf2-sha256$0"),
            &ok.replace("pbkdf2-sha256$100000", "pbkdf2-sha256$999999999"),
            &format!("{ok}$extra"),
            &ok[..ok.len() - 2],
        ] {
            assert_eq!(PinHash::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn la_entropia_cambia_la_sal() {
        let a = salt(b"x");
        add_entropy(b"azar del arranque");
        assert_ne!(a, salt(b"x"));
    }
}
