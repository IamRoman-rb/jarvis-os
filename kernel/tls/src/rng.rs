//! Generador de números al azar criptográfico (CSPRNG).
//!
//! Dos partes:
//! - [`Pool`] junta entropía de las fuentes del kernel (RDSEED/RDRAND, variación del TSC, el RTC)
//!   con SHA-256 y lleva la cuenta de cuántos bits de entropía se le **acreditaron**. Cada fuente
//!   acredita de menos a propósito: sobreestimar es el error peligroso.
//! - [`Rng`] estira esa semilla con ChaCha20 y **borrado rápido de la clave** (Bernstein, "Fast-key-
//!   erasure random-number generators", 2017): cada pedido genera 32 bytes de más que pasan a ser
//!   la clave nueva, y la vieja se pisa. Si alguien lee el estado después, no puede reconstruir lo
//!   que ya salió (secreto hacia atrás).

use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};

/// Bits de entropía que hacen falta antes de generar claves (el nivel de seguridad de AES-256 y
/// de X25519 es 128; se pide el doble por lo conservador de las estimaciones).
pub const SEED_BITS: u32 = 256;

/// Separa este uso de SHA-256 de cualquier otro (así una semilla nunca coincide con otro hash).
const DOMAIN: &[u8] = b"jarvis-os rng v1";

/// Acumulador de entropía.
pub struct Pool {
    hash: Sha256,
    credited: u32,
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Pool {
    pub fn new() -> Pool {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        Pool { hash, credited: 0 }
    }

    /// Mezcla `data` y le acredita `bits` de entropía. Datos sin entropía (la MAC, la hora) igual
    /// suman: no hacen daño y distinguen máquinas, pero se acreditan con 0.
    pub fn add(&mut self, data: &[u8], bits: u32) {
        // El largo va antes para que ("ab","c") y ("a","bc") no den lo mismo.
        self.hash.update((data.len() as u64).to_le_bytes());
        self.hash.update(data);
        self.credited = self.credited.saturating_add(bits);
    }

    /// Bits acreditados hasta ahora.
    pub fn credited(&self) -> u32 {
        self.credited
    }

    /// ¿Alcanza para generar claves?
    pub fn ready(&self) -> bool {
        self.credited >= SEED_BITS
    }

    /// La semilla: 32 bytes. Consume el acumulador.
    pub fn seed(self) -> [u8; 32] {
        self.hash.finalize().into()
    }
}

/// Cuántos bits acreditar a una serie de mediciones del TSC (cuánto tardó, en ciclos, un mismo
/// trabajo corto repetido). Lo impredecible es la **variación**: cachés, interrupciones del
/// hipervisor, la frecuencia del bus. Como el "stuck test" de jitterentropy (Linux), una muestra
/// solo cuenta si ni ella ni su variación repiten lo anterior (primera y segunda diferencia
/// distintas de cero), y vale a lo sumo 1 bit. En un emulador puro (QEMU sin aceleración) casi
/// todo se repite y la cuenta da cerca de 0: ahí hace falta otra fuente.
pub fn jitter_credit(deltas: &[u64]) -> u32 {
    let mut bits = 0;
    for w in deltas.windows(3) {
        let d1 = w[2].wrapping_sub(w[1]);
        let d0 = w[1].wrapping_sub(w[0]);
        if d1 != 0 && d1 != d0 {
            bits += 1;
        }
    }
    bits
}

/// ChaCha20 con borrado rápido de la clave.
pub struct Rng {
    key: [u8; 32],
}

impl core::fmt::Debug for Rng {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Nunca la clave: un log no puede revelar el estado del generador.
        f.write_str("Rng { .. }")
    }
}

/// Cuánto se genera por vuelta: 32 bytes de clave nueva + hasta 224 de salida (4 bloques de
/// ChaCha20). Los pedidos más largos se parten, y cada parte rota la clave.
const CHUNK: usize = 224;

impl Rng {
    /// Un generador a partir de una semilla de [`Pool::seed`].
    pub fn from_seed(seed: [u8; 32]) -> Rng {
        Rng { key: seed }
    }

    /// Mezcla entropía nueva en la clave (por ejemplo, cada tanto desde el timer).
    pub fn reseed(&mut self, data: &[u8]) {
        let mut h = Sha256::new();
        h.update(DOMAIN);
        h.update(self.key);
        h.update(data);
        self.key = h.finalize().into();
    }

    /// Llena `out` con bytes al azar.
    pub fn fill(&mut self, out: &mut [u8]) {
        for part in out.chunks_mut(CHUNK) {
            let mut block = [0u8; 32 + CHUNK];
            let n = 32 + part.len();
            // La clave se usa una sola vez, así que el nonce fijo en cero no se repite nunca con
            // la misma clave (la condición de seguridad de un cifrador de flujo).
            let mut c = ChaCha20::new(&self.key.into(), &[0u8; 12].into());
            c.apply_keystream(&mut block[..n]);
            self.key.copy_from_slice(&block[..32]);
            part.copy_from_slice(&block[32..n]);
            block.fill(0);
        }
    }

    /// Un `u64` al azar.
    pub fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill(&mut b);
        u64::from_le_bytes(b)
    }
}
