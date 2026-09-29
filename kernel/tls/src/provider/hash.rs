//! Hashes (SHA-256 y SHA-384) y HMAC para rustls. Con HMAC, rustls arma solo el HKDF de TLS 1.3
//! y la PRF de TLS 1.2.

use alloc::boxed::Box;
use core::marker::PhantomData;

use hmac::{Mac, SimpleHmac};
use rustls::crypto::{hash, hmac as rhmac};
use sha2::Digest;

/// Un hash de la familia SHA-2 con su nombre para rustls.
pub(crate) trait Sha2:
    Digest + Clone + Send + Sync + 'static + hmac::digest::core_api::BlockSizeUser
{
    const ALG: hash::HashAlgorithm;
}

impl Sha2 for sha2::Sha256 {
    const ALG: hash::HashAlgorithm = hash::HashAlgorithm::SHA256;
}

impl Sha2 for sha2::Sha384 {
    const ALG: hash::HashAlgorithm = hash::HashAlgorithm::SHA384;
}

pub(crate) struct Hash<D>(PhantomData<fn() -> D>);

pub(crate) static SHA256: Hash<sha2::Sha256> = Hash(PhantomData);
pub(crate) static SHA384: Hash<sha2::Sha384> = Hash(PhantomData);

impl<D: Sha2> hash::Hash for Hash<D> {
    fn start(&self) -> Box<dyn hash::Context> {
        Box::new(Context(D::new()))
    }

    fn hash(&self, data: &[u8]) -> hash::Output {
        hash::Output::new(&D::digest(data)[..])
    }

    fn algorithm(&self) -> hash::HashAlgorithm {
        D::ALG
    }

    fn output_len(&self) -> usize {
        <D as Digest>::output_size()
    }
}

struct Context<D>(D);

impl<D: Sha2> hash::Context for Context<D> {
    fn fork_finish(&self) -> hash::Output {
        hash::Output::new(&self.0.clone().finalize()[..])
    }

    fn fork(&self) -> Box<dyn hash::Context> {
        Box::new(Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> hash::Output {
        hash::Output::new(&self.0.finalize()[..])
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

pub(crate) struct Hmac<D>(PhantomData<fn() -> D>);

pub(crate) static HMAC_SHA256: Hmac<sha2::Sha256> = Hmac(PhantomData);
pub(crate) static HMAC_SHA384: Hmac<sha2::Sha384> = Hmac(PhantomData);

impl<D: Sha2> rhmac::Hmac for Hmac<D> {
    fn with_key(&self, key: &[u8]) -> Box<dyn rhmac::Key> {
        // HMAC acepta claves de cualquier largo (las largas se hashean), así que no falla.
        let mac = <SimpleHmac<D> as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
        Box::new(HmacKey(mac))
    }

    fn hash_output_len(&self) -> usize {
        <D as Digest>::output_size()
    }
}

struct HmacKey<D: Sha2>(SimpleHmac<D>);

impl<D: Sha2> rhmac::Key for HmacKey<D> {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> rhmac::Tag {
        let mut ctx = self.0.clone();
        ctx.update(first);
        for m in middle {
            ctx.update(m);
        }
        ctx.update(last);
        rhmac::Tag::new(&ctx.finalize().into_bytes()[..])
    }

    fn tag_len(&self) -> usize {
        <D as Digest>::output_size()
    }
}
