//! Verificación de firmas: de la cadena de certificados (cada uno firmado por el de arriba, hasta
//! una raíz de confianza) y del handshake (el servidor prueba que tiene la clave privada del
//! certificado firmando la conversación).

use rsa::pkcs1::der::Decode;
use rsa::signature::Verifier;
use rsa::{BigUint, RsaPublicKey, pkcs1, pkcs1v15, pss};
use rustls::SignatureScheme;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{
    AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id,
};
use sha2::Digest;

/// Largo mínimo y máximo de las claves RSA que se aceptan (lo mismo que ring y aws-lc).
const RSA_MIN_BITS: usize = 2048;
const RSA_MAX_BITS: usize = 8192;

pub(crate) static ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        ECDSA_P256_SHA256,
        ECDSA_P256_SHA384,
        ECDSA_P384_SHA256,
        ECDSA_P384_SHA384,
        RSA_PSS_SHA256,
        RSA_PSS_SHA384,
        RSA_PSS_SHA512,
        RSA_PKCS1_SHA256,
        RSA_PKCS1_SHA384,
        RSA_PKCS1_SHA512,
    ],
    // En TLS 1.3 cada esquema fija curva y hash; en TLS 1.2 ECDSA no fija la curva.
    mapping: &[
        (
            SignatureScheme::ECDSA_NISTP384_SHA384,
            &[ECDSA_P384_SHA384, ECDSA_P256_SHA384],
        ),
        (
            SignatureScheme::ECDSA_NISTP256_SHA256,
            &[ECDSA_P256_SHA256, ECDSA_P384_SHA256],
        ),
        (SignatureScheme::RSA_PSS_SHA512, &[RSA_PSS_SHA512]),
        (SignatureScheme::RSA_PSS_SHA384, &[RSA_PSS_SHA384]),
        (SignatureScheme::RSA_PSS_SHA256, &[RSA_PSS_SHA256]),
        (SignatureScheme::RSA_PKCS1_SHA512, &[RSA_PKCS1_SHA512]),
        (SignatureScheme::RSA_PKCS1_SHA384, &[RSA_PKCS1_SHA384]),
        (SignatureScheme::RSA_PKCS1_SHA256, &[RSA_PKCS1_SHA256]),
    ],
};

// --- ECDSA -----------------------------------------------------------------------------------

macro_rules! ecdsa {
    ($name:ident, $ty:ident, $curve:ident, $hash:ty, $key_alg:ident, $sig_alg:ident) => {
        static $name: &dyn SignatureVerificationAlgorithm = &$ty;

        #[derive(Debug)]
        struct $ty;

        impl SignatureVerificationAlgorithm for $ty {
            fn public_key_alg_id(&self) -> AlgorithmIdentifier {
                alg_id::$key_alg
            }

            fn signature_alg_id(&self) -> AlgorithmIdentifier {
                alg_id::$sig_alg
            }

            fn verify_signature(
                &self,
                public_key: &[u8],
                message: &[u8],
                signature: &[u8],
            ) -> Result<(), InvalidSignature> {
                use $curve::ecdsa::signature::hazmat::PrehashVerifier;
                let key = $curve::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
                    .map_err(|_| InvalidSignature)?;
                // En X.509 y TLS la firma ECDSA va en DER: SEQUENCE { r, s }.
                let sig =
                    $curve::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?;
                // Se hashea aparte porque el hash puede no ser el "natural" de la curva
                // (P-256 con SHA-384): `verify_prehash` lo trunca como dice el estándar.
                let digest = <$hash>::digest(message);
                key.verify_prehash(&digest, &sig)
                    .map_err(|_| InvalidSignature)
            }
        }
    };
}

ecdsa!(
    ECDSA_P256_SHA256,
    EcdsaP256Sha256,
    p256,
    sha2::Sha256,
    ECDSA_P256,
    ECDSA_SHA256
);
ecdsa!(
    ECDSA_P256_SHA384,
    EcdsaP256Sha384,
    p256,
    sha2::Sha384,
    ECDSA_P256,
    ECDSA_SHA384
);
ecdsa!(
    ECDSA_P384_SHA256,
    EcdsaP384Sha256,
    p384,
    sha2::Sha256,
    ECDSA_P384,
    ECDSA_SHA256
);
ecdsa!(
    ECDSA_P384_SHA384,
    EcdsaP384Sha384,
    p384,
    sha2::Sha384,
    ECDSA_P384,
    ECDSA_SHA384
);

// --- RSA -------------------------------------------------------------------------------------

macro_rules! rsa {
    ($name:ident, $ty:ident, $scheme:ident, $hash:ty, $sig_alg:ident) => {
        static $name: &dyn SignatureVerificationAlgorithm = &$ty;

        #[derive(Debug)]
        struct $ty;

        impl SignatureVerificationAlgorithm for $ty {
            fn public_key_alg_id(&self) -> AlgorithmIdentifier {
                alg_id::RSA_ENCRYPTION
            }

            fn signature_alg_id(&self) -> AlgorithmIdentifier {
                alg_id::$sig_alg
            }

            fn verify_signature(
                &self,
                public_key: &[u8],
                message: &[u8],
                signature: &[u8],
            ) -> Result<(), InvalidSignature> {
                let key = rsa_key(public_key)?;
                let sig = $scheme::Signature::try_from(signature).map_err(|_| InvalidSignature)?;
                $scheme::VerifyingKey::<$hash>::new(key)
                    .verify(message, &sig)
                    .map_err(|_| InvalidSignature)
            }
        }
    };
}

rsa!(
    RSA_PKCS1_SHA256,
    RsaPkcs1Sha256,
    pkcs1v15,
    sha2::Sha256,
    RSA_PKCS1_SHA256
);
rsa!(
    RSA_PKCS1_SHA384,
    RsaPkcs1Sha384,
    pkcs1v15,
    sha2::Sha384,
    RSA_PKCS1_SHA384
);
rsa!(
    RSA_PKCS1_SHA512,
    RsaPkcs1Sha512,
    pkcs1v15,
    sha2::Sha512,
    RSA_PKCS1_SHA512
);
rsa!(
    RSA_PSS_SHA256,
    RsaPssSha256,
    pss,
    sha2::Sha256,
    RSA_PSS_SHA256
);
rsa!(
    RSA_PSS_SHA384,
    RsaPssSha384,
    pss,
    sha2::Sha384,
    RSA_PSS_SHA384
);
rsa!(
    RSA_PSS_SHA512,
    RsaPssSha512,
    pss,
    sha2::Sha512,
    RSA_PSS_SHA512
);

/// La clave pública RSA viene como `RSAPublicKey` en DER: SEQUENCE { n, e }.
fn rsa_key(der: &[u8]) -> Result<RsaPublicKey, InvalidSignature> {
    let k = pkcs1::RsaPublicKey::from_der(der).map_err(|_| InvalidSignature)?;
    let n = BigUint::from_bytes_be(k.modulus.as_bytes());
    let e = BigUint::from_bytes_be(k.public_exponent.as_bytes());
    if n.bits() < RSA_MIN_BITS {
        return Err(InvalidSignature);
    }
    RsaPublicKey::new_with_max_size(n, e, RSA_MAX_BITS).map_err(|_| InvalidSignature)
}
