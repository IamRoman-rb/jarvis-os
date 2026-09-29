//! Cifrado autenticado (AEAD) de los registros de TLS y las suites que lo usan.
//!
//! Una AEAD cifra y además agrega una etiqueta (16 bytes) que detecta cualquier cambio: si un
//! byte del registro se tocó en el camino, descifrar falla. El **nonce** (número que no se repite
//! con la misma clave) sale del número de secuencia del registro:
//! - TLS 1.3 (y ChaCha20 en TLS 1.2): nonce = IV XOR secuencia; no viaja.
//! - AES-GCM en TLS 1.2: nonce = 4 bytes fijos + 8 "explícitos" que viajan al principio de cada
//!   registro (acá son el mismo XOR con la secuencia, como hacen ring y aws-lc).

use alloc::boxed::Box;
use core::marker::PhantomData;

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, Buffer, KeyInit};
use rustls::crypto::cipher::{
    AeadKey, BorrowedPayload, InboundOpaqueMessage, InboundPlainMessage, Iv, KeyBlockShape,
    MessageDecrypter, MessageEncrypter, NONCE_LEN, Nonce, OutboundOpaqueMessage,
    OutboundPlainMessage, PrefixedPayload, Tls12AeadAlgorithm, Tls13AeadAlgorithm,
    UnsupportedOperationError, make_tls12_aad, make_tls13_aad,
};
use rustls::crypto::tls12::PrfUsingHmac;
use rustls::crypto::tls13::HkdfUsingHmac;
use rustls::crypto::{CipherSuiteCommon, KeyExchangeAlgorithm};
use rustls::{
    CipherSuite, ConnectionTrafficSecrets, ContentType, ProtocolVersion, SignatureScheme,
    SupportedCipherSuite, Tls12CipherSuite, Tls13CipherSuite,
};

use super::hash::{HMAC_SHA256, HMAC_SHA384, SHA256, SHA384};

/// Largo de la etiqueta de las tres AEAD.
const TAG_LEN: usize = 16;
/// Bytes explícitos del nonce de AES-GCM en TLS 1.2.
const GCM_EXPLICIT: usize = 8;

/// Una AEAD de RustCrypto con su largo de clave.
pub(crate) trait Algo: Send + Sync + 'static {
    type Cipher: AeadInPlace + KeyInit + Send + Sync + 'static;
    /// ¿El nonce de TLS 1.2 lleva parte explícita? (sí en GCM, no en ChaCha20-Poly1305)
    const TLS12_EXPLICIT: bool;

    fn cipher(key: &AeadKey) -> Self::Cipher {
        // rustls genera la clave con el largo que pide `key_len`/`key_block_shape`, que sale del
        // mismo tipo: nunca puede no coincidir.
        Self::Cipher::new_from_slice(key.as_ref()).unwrap_or_else(|_| unreachable!())
    }
}

pub(crate) struct Aes128;
pub(crate) struct Aes256;
pub(crate) struct ChaCha;

impl Algo for Aes128 {
    type Cipher = aes_gcm::Aes128Gcm;
    const TLS12_EXPLICIT: bool = true;
}
impl Algo for Aes256 {
    type Cipher = aes_gcm::Aes256Gcm;
    const TLS12_EXPLICIT: bool = true;
}
impl Algo for ChaCha {
    type Cipher = chacha20poly1305::ChaCha20Poly1305;
    const TLS12_EXPLICIT: bool = false;
}

pub(crate) struct Aead<A>(PhantomData<fn() -> A>);

static AES_128_GCM: Aead<Aes128> = Aead(PhantomData);
static AES_256_GCM: Aead<Aes256> = Aead(PhantomData);
static CHACHA20_POLY1305: Aead<ChaCha> = Aead(PhantomData);

fn key_len<A: Algo>() -> usize {
    <A::Cipher as aes_gcm::aead::KeySizeUser>::key_size()
}

impl<A: Algo> Tls13AeadAlgorithm for Aead<A> {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13Cipher::<A>(A::cipher(&key), iv))
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13Cipher::<A>(A::cipher(&key), iv))
    }

    fn key_len(&self) -> usize {
        key_len::<A>()
    }

    fn extract_keys(
        &self,
        _key: AeadKey,
        _iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        // Solo sirve para pasarle las claves al kernel de Linux (kTLS): acá no.
        Err(UnsupportedOperationError)
    }
}

impl<A: Algo> Tls12AeadAlgorithm for Aead<A> {
    fn encrypter(&self, key: AeadKey, iv: &[u8], extra: &[u8]) -> Box<dyn MessageEncrypter> {
        Box::new(Tls12Cipher::<A>(A::cipher(&key), tls12_iv::<A>(iv, extra)))
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        // Al descifrar, la parte explícita viene en cada registro: basta con la fija.
        Box::new(Tls12Cipher::<A>(
            A::cipher(&key),
            tls12_iv::<A>(iv, &[0; GCM_EXPLICIT]),
        ))
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: key_len::<A>(),
            fixed_iv_len: if A::TLS12_EXPLICIT { 4 } else { NONCE_LEN },
            explicit_nonce_len: if A::TLS12_EXPLICIT { GCM_EXPLICIT } else { 0 },
        }
    }

    fn extract_keys(
        &self,
        _key: AeadKey,
        _iv: &[u8],
        _explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Err(UnsupportedOperationError)
    }
}

/// El IV de 12 bytes de TLS 1.2: en GCM, los 4 fijos + los 8 que da el bloque de claves.
fn tls12_iv<A: Algo>(iv: &[u8], extra: &[u8]) -> Iv {
    let mut full = [0u8; NONCE_LEN];
    if A::TLS12_EXPLICIT {
        full[..4].copy_from_slice(&iv[..4]);
        full[4..].copy_from_slice(&extra[..GCM_EXPLICIT]);
    } else {
        full.copy_from_slice(&iv[..NONCE_LEN]);
    }
    Iv::new(full)
}

struct Tls13Cipher<A: Algo>(A::Cipher, Iv);

impl<A: Algo> MessageEncrypter for Tls13Cipher<A> {
    fn encrypt(
        &mut self,
        m: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, rustls::Error> {
        let total_len = self.encrypted_payload_len(m.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);
        payload.extend_from_chunks(&m.payload);
        // En TLS 1.3 el tipo real del registro va cifrado, al final.
        payload.extend_from_slice(&m.typ.to_array());
        let nonce = Nonce::new(&self.1, seq).0;
        let nonce = GenericArray::from_slice(&nonce);
        let aad = make_tls13_aad(total_len);
        self.0
            .encrypt_in_place(nonce, &aad, &mut EncryptBuffer(&mut payload))
            .map_err(|_| rustls::Error::EncryptError)?;
        // Por fuera todo registro de TLS 1.3 dice "datos de aplicación, TLS 1.2".
        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + 1 + TAG_LEN
    }
}

impl<A: Algo> MessageDecrypter for Tls13Cipher<A> {
    fn decrypt<'a>(
        &mut self,
        mut m: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, rustls::Error> {
        let payload = &mut m.payload;
        let nonce = Nonce::new(&self.1, seq).0;
        let nonce = GenericArray::from_slice(&nonce);
        let aad = make_tls13_aad(payload.len());
        self.0
            .decrypt_in_place(nonce, &aad, &mut DecryptBuffer(payload))
            .map_err(|_| rustls::Error::DecryptError)?;
        m.into_tls13_unpadded_message()
    }
}

struct Tls12Cipher<A: Algo>(A::Cipher, Iv);

impl<A: Algo> MessageEncrypter for Tls12Cipher<A> {
    fn encrypt(
        &mut self,
        m: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, rustls::Error> {
        let total_len = self.encrypted_payload_len(m.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);
        let nonce = Nonce::new(&self.1, seq).0;
        let aad = make_tls12_aad(seq, m.typ, m.version, m.payload.len());
        let skip = if A::TLS12_EXPLICIT {
            payload.extend_from_slice(&nonce[4..]);
            GCM_EXPLICIT
        } else {
            0
        };
        payload.extend_from_chunks(&m.payload);
        let tag = self
            .0
            .encrypt_in_place_detached(
                GenericArray::from_slice(&nonce),
                &aad,
                &mut payload.as_mut()[skip..],
            )
            .map_err(|_| rustls::Error::EncryptError)?;
        payload.extend_from_slice(&tag);
        Ok(OutboundOpaqueMessage::new(m.typ, m.version, payload))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + TAG_LEN + if A::TLS12_EXPLICIT { GCM_EXPLICIT } else { 0 }
    }
}

impl<A: Algo> MessageDecrypter for Tls12Cipher<A> {
    fn decrypt<'a>(
        &mut self,
        mut m: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, rustls::Error> {
        let skip = if A::TLS12_EXPLICIT { GCM_EXPLICIT } else { 0 };
        let len = m.payload.len();
        if len < skip + TAG_LEN {
            return Err(rustls::Error::DecryptError);
        }
        let plain_len = len - skip - TAG_LEN;
        let aad = make_tls12_aad(seq, m.typ, m.version, plain_len);
        let payload: &mut BorrowedPayload<'a> = &mut m.payload;
        let mut nonce = Nonce::new(&self.1, seq).0;
        if A::TLS12_EXPLICIT {
            nonce[4..].copy_from_slice(&payload[..GCM_EXPLICIT]);
        }
        let (body, tag) = payload[skip..].split_at_mut(plain_len);
        self.0
            .decrypt_in_place_detached(
                GenericArray::from_slice(&nonce),
                &aad,
                body,
                GenericArray::from_slice(tag),
            )
            .map_err(|_| rustls::Error::DecryptError)?;
        // El texto descifrado quedó después de la parte explícita: se corre al principio.
        payload.copy_within(skip..skip + plain_len, 0);
        payload.truncate(plain_len);
        Ok(m.into_plain_message())
    }
}

/// `PrefixedPayload` como `aead::Buffer` (para cifrar en el lugar).
struct EncryptBuffer<'a>(&'a mut PrefixedPayload);

impl AsRef<[u8]> for EncryptBuffer<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl AsMut<[u8]> for EncryptBuffer<'_> {
    fn as_mut(&mut self) -> &mut [u8] {
        self.0.as_mut()
    }
}

impl Buffer for EncryptBuffer<'_> {
    fn extend_from_slice(&mut self, other: &[u8]) -> aes_gcm::aead::Result<()> {
        self.0.extend_from_slice(other);
        Ok(())
    }

    fn truncate(&mut self, len: usize) {
        self.0.truncate(len)
    }
}

/// `BorrowedPayload` como `aead::Buffer` (para descifrar en el lugar: solo achica).
struct DecryptBuffer<'a, 'p>(&'a mut BorrowedPayload<'p>);

impl AsRef<[u8]> for DecryptBuffer<'_, '_> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl AsMut<[u8]> for DecryptBuffer<'_, '_> {
    fn as_mut(&mut self) -> &mut [u8] {
        self.0
    }
}

impl Buffer for DecryptBuffer<'_, '_> {
    fn extend_from_slice(&mut self, _: &[u8]) -> aes_gcm::aead::Result<()> {
        // `decrypt_in_place` nunca agranda el buffer.
        Err(aes_gcm::aead::Error)
    }

    fn truncate(&mut self, len: usize) {
        self.0.truncate(len)
    }
}

// --- Las suites -------------------------------------------------------------------------------

const fn common(
    suite: CipherSuite,
    hash: &'static dyn rustls::crypto::hash::Hash,
) -> CipherSuiteCommon {
    CipherSuiteCommon {
        suite,
        hash_provider: hash,
        // Registros que se pueden cifrar con una clave antes de cambiarla (RFC 8446 §5.5):
        // 2^24,5 para AES-GCM; ChaCha20-Poly1305 no tiene límite práctico.
        confidentiality_limit: 1 << 23,
    }
}

pub(crate) static TLS13_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: common(CipherSuite::TLS13_AES_128_GCM_SHA256, &SHA256),
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA256),
        aead_alg: &AES_128_GCM,
        quic: None,
    });

pub(crate) static TLS13_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: common(CipherSuite::TLS13_AES_256_GCM_SHA384, &SHA384),
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA384),
        aead_alg: &AES_256_GCM,
        quic: None,
    });

pub(crate) static TLS13_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: common(CipherSuite::TLS13_CHACHA20_POLY1305_SHA256, &SHA256),
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA256),
        aead_alg: &CHACHA20_POLY1305,
        quic: None,
    });

const ECDSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::ECDSA_NISTP384_SHA384,
    SignatureScheme::ECDSA_NISTP256_SHA256,
];

const RSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PKCS1_SHA512,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA256,
];

macro_rules! tls12_suite {
    ($name:ident, $suite:ident, $sign:expr, $hash:expr, $hmac:expr, $aead:expr) => {
        pub(crate) static $name: SupportedCipherSuite =
            SupportedCipherSuite::Tls12(&Tls12CipherSuite {
                common: common(CipherSuite::$suite, $hash),
                kx: KeyExchangeAlgorithm::ECDHE,
                sign: $sign,
                prf_provider: &PrfUsingHmac($hmac),
                aead_alg: $aead,
            });
    };
}

tls12_suite!(
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    ECDSA_SCHEMES,
    &SHA256,
    &HMAC_SHA256,
    &AES_128_GCM
);
tls12_suite!(
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    ECDSA_SCHEMES,
    &SHA384,
    &HMAC_SHA384,
    &AES_256_GCM
);
tls12_suite!(
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    ECDSA_SCHEMES,
    &SHA256,
    &HMAC_SHA256,
    &CHACHA20_POLY1305
);
tls12_suite!(
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    RSA_SCHEMES,
    &SHA256,
    &HMAC_SHA256,
    &AES_128_GCM
);
tls12_suite!(
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    RSA_SCHEMES,
    &SHA384,
    &HMAC_SHA384,
    &AES_256_GCM
);
tls12_suite!(
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    RSA_SCHEMES,
    &SHA256,
    &HMAC_SHA256,
    &CHACHA20_POLY1305
);

/// En orden de preferencia: TLS 1.3 primero; AES-GCM antes que ChaCha20 porque es lo que
/// prefieren casi todos los servidores (en el kernel los dos son por software).
pub(crate) static ALL_CIPHER_SUITES: &[SupportedCipherSuite] = &[
    TLS13_AES_128_GCM_SHA256,
    TLS13_AES_256_GCM_SHA384,
    TLS13_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
];
