//! Proveedor de criptografía de JARVIS-OS para rustls, sobre las primitivas de RustCrypto.
//!
//! rustls hace el protocolo (mensajes, estados, la cadena de certificados) y le pide la
//! matemática a un `CryptoProvider`. Los que trae (aws-lc-rs, ring) tienen C y ensamblador que no
//! compilan para el kernel, y `rustls-rustcrypto` sigue en alfa: por eso este, chico y a la vista
//! (se basa en el `provider-example` de rustls). Qué hay:
//!
//! - **Suites** (qué cifrado simétrico y qué hash): TLS 1.3 con AES-128-GCM, AES-256-GCM y
//!   ChaCha20-Poly1305; TLS 1.2 con ECDHE + las mismas AEAD (nada de CBC ni RSA sin ECDHE: no dan
//!   secreto hacia adelante o tienen ataques conocidos).
//! - **Intercambio de claves**: X25519 y P-256 (ECDHE: claves efímeras por conexión).
//! - **Firmas** que se verifican (los certificados y el handshake): ECDSA P-256/P-384 y RSA
//!   PKCS#1 v1.5 / PSS con SHA-256/384/512.
//! - **Azar**: lo pone el llamador (en el kernel, `entropy.rs`).

use alloc::boxed::Box;
use alloc::sync::Arc;

use rustls::crypto::{CryptoProvider, KeyProvider, SecureRandom, SupportedKxGroup};
use rustls::pki_types::PrivateKeyDer;

mod aead;
mod hash;
mod kx;
mod verify;

/// El proveedor completo. `random` es la fuente de azar (tiene que ser criptográfica).
///
/// Los grupos de intercambio de claves guardan `random`, así que se crean acá y viven para
/// siempre (`Box::leak`): se llama una vez por configuración, no por conexión.
pub fn provider(random: &'static dyn SecureRandom) -> CryptoProvider {
    let x25519: &'static dyn SupportedKxGroup = Box::leak(Box::new(kx::X25519 { random }));
    let p256: &'static dyn SupportedKxGroup = Box::leak(Box::new(kx::P256 { random }));
    CryptoProvider {
        cipher_suites: aead::ALL_CIPHER_SUITES.to_vec(),
        kx_groups: alloc::vec![x25519, p256],
        signature_verification_algorithms: verify::ALGORITHMS,
        secure_random: random,
        key_provider: &NoKeys,
    }
}

/// Un cliente que no se autentica con certificado propio: no carga claves privadas.
#[derive(Debug)]
struct NoKeys;

impl KeyProvider for NoKeys {
    fn load_private_key(
        &self,
        _key_der: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
        Err(rustls::Error::General(
            "JARVIS-OS no usa certificados de cliente".into(),
        ))
    }
}
