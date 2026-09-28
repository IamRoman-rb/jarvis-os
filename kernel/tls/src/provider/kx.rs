//! Intercambio de claves efímero (ECDHE): cada conexión inventa un par de claves, manda la pública
//! y combina la suya privada con la pública del servidor. Los dos llegan al mismo secreto sin que
//! viaje, y como las claves se tiran al terminar, robar después la clave del certificado no
//! descifra conversaciones grabadas (secreto hacia adelante).

use alloc::boxed::Box;
use alloc::vec::Vec;

use p256::elliptic_curve::sec1::ToEncodedPoint;
use rustls::crypto::{ActiveKeyExchange, SecureRandom, SharedSecret, SupportedKxGroup};
use rustls::{NamedGroup, PeerMisbehaved};

fn random32(random: &dyn SecureRandom) -> Result<[u8; 32], rustls::Error> {
    let mut b = [0u8; 32];
    random
        .fill(&mut b)
        .map_err(|_| rustls::Error::FailedToGetRandomBytes)?;
    Ok(b)
}

/// X25519 (RFC 7748): la curva preferida de TLS 1.3.
#[derive(Debug)]
pub(crate) struct X25519 {
    pub(crate) random: &'static dyn SecureRandom,
}

impl SupportedKxGroup for X25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        // Toda cadena de 32 bytes es una clave privada válida (se "recorta" al usarla).
        let secret = x25519_dalek::StaticSecret::from(random32(self.random)?);
        let public = x25519_dalek::PublicKey::from(&secret);
        Ok(Box::new(X25519Active { secret, public }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

struct X25519Active {
    secret: x25519_dalek::StaticSecret,
    public: x25519_dalek::PublicKey,
}

impl ActiveKeyExchange for X25519Active {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        let peer: [u8; 32] = peer
            .try_into()
            .map_err(|_| rustls::Error::from(PeerMisbehaved::InvalidKeyShare))?;
        let shared = self
            .secret
            .diffie_hellman(&x25519_dalek::PublicKey::from(peer));
        // Un punto de orden chico da un secreto en cero: el servidor está haciendo trampa.
        if !shared.was_contributory() {
            return Err(PeerMisbehaved::InvalidKeyShare.into());
        }
        Ok(SharedSecret::from(&shared.as_bytes()[..]))
    }

    fn pub_key(&self) -> &[u8] {
        self.public.as_bytes()
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

/// P-256 (secp256r1): la que soportan todos los servidores, incluso los viejos.
#[derive(Debug)]
pub(crate) struct P256 {
    pub(crate) random: &'static dyn SecureRandom,
}

impl SupportedKxGroup for P256 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        // No toda cadena es un escalar válido (tiene que ser 0 < k < n); casi siempre la primera
        // lo es (la probabilidad de fallar es ~2^-32).
        let secret = loop {
            if let Ok(k) = p256::SecretKey::from_slice(&random32(self.random)?) {
                break k;
            }
        };
        // TLS manda los puntos sin comprimir: 0x04 || x || y.
        let public = secret
            .public_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        Ok(Box::new(P256Active { secret, public }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

struct P256Active {
    secret: p256::SecretKey,
    public: Vec<u8>,
}

impl ActiveKeyExchange for P256Active {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        // `from_sec1_bytes` verifica que el punto esté en la curva (si no, se filtraría la clave).
        let peer = p256::PublicKey::from_sec1_bytes(peer)
            .map_err(|_| rustls::Error::from(PeerMisbehaved::InvalidKeyShare))?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok(SharedSecret::from(&shared.raw_secret_bytes()[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}
