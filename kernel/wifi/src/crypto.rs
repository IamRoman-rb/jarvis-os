//! De la contraseña a las claves de WPA2-PSK (IEEE 802.11-2020, 12.7.1).
//!
//! - **PMK** (la clave maestra): PBKDF2-HMAC-SHA1 de la contraseña, con el SSID como sal y 4096
//!   vueltas. Es lenta a propósito: probar contraseñas de a millones cuesta caro.
//! - **PTK** (las claves de la sesión): la PRF de 802.11 sobre la PMK, las dos MAC y los dos
//!   números al azar (ANonce del punto de acceso, SNonce nuestro). Se parte en KCK (firma los
//!   mensajes EAPOL), KEK (cifra la clave de grupo) y TK (cifra los datos, con CCMP).
//! - El **MIC** de EAPOL (descriptor versión 2): HMAC-SHA1 truncado a 16 bytes.
//! - **AES Key Wrap** (RFC 3394): cómo viaja la clave de grupo en el mensaje 3.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use hmac::{Hmac, Mac as _};
use sha1::Sha1;

use crate::Mac;

type HmacSha1 = Hmac<Sha1>;

/// La PMK de una red WPA2-PSK.
pub fn pmk(passphrase: &str, ssid: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha1>(passphrase.as_bytes(), ssid, 4096, &mut out);
    out
}

fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    // HMAC acepta claves de cualquier largo: no puede fallar.
    let mut mac = <HmacSha1 as hmac::Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    for p in parts {
        mac.update(p);
    }
    mac.finalize().into_bytes().into()
}

/// La PRF de 802.11: `out` se llena con HMAC-SHA1(K, etiqueta || 0 || datos || i), i = 0, 1…
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    for (i, chunk) in out.chunks_mut(20).enumerate() {
        let block = hmac_sha1(key, &[label, &[0], data, &[i as u8]]);
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
}

/// Las claves de la sesión (PTK de 48 bytes, para CCMP).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ptk {
    /// Firma los mensajes EAPOL.
    pub kck: [u8; 16],
    /// Cifra la clave de grupo.
    pub kek: [u8; 16],
    /// Cifra los datos (CCMP).
    pub tk: [u8; 16],
}

/// La PTK: lo de cada lado se ordena (menor primero), así las dos puntas llegan a lo mismo.
pub fn ptk(pmk: &[u8; 32], aa: &Mac, spa: &Mac, anonce: &[u8; 32], snonce: &[u8; 32]) -> Ptk {
    let mut data = [0u8; 6 * 2 + 32 * 2];
    let (lo, hi) = if aa <= spa { (aa, spa) } else { (spa, aa) };
    data[..6].copy_from_slice(lo);
    data[6..12].copy_from_slice(hi);
    let (lo, hi) = if anonce <= snonce {
        (anonce, snonce)
    } else {
        (snonce, anonce)
    };
    data[12..44].copy_from_slice(lo);
    data[44..].copy_from_slice(hi);
    let mut out = [0u8; 48];
    prf(pmk, b"Pairwise key expansion", &data, &mut out);
    let mut k = Ptk {
        kck: [0; 16],
        kek: [0; 16],
        tk: [0; 16],
    };
    k.kck.copy_from_slice(&out[..16]);
    k.kek.copy_from_slice(&out[16..32]);
    k.tk.copy_from_slice(&out[32..]);
    k
}

/// El MIC de un mensaje EAPOL-Key (con su campo MIC en cero).
pub fn eapol_mic(kck: &[u8; 16], frame: &[u8]) -> [u8; 16] {
    let full = hmac_sha1(kck, &[frame]);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&full[..16]);
    mic
}

/// Comparación en tiempo constante (que el tiempo no diga cuántos bytes coincidieron).
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const WRAP_IV: [u8; 8] = [0xa6; 8];

/// Desenvuelve una clave cifrada con AES Key Wrap (RFC 3394, 2.2.2). `None` si el control de
/// integridad no da (otra KEK, o el mensaje se modificó en el camino).
pub fn key_unwrap(kek: &[u8; 16], wrapped: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    if wrapped.len() < 24 || !wrapped.len().is_multiple_of(8) {
        return None;
    }
    let n = wrapped.len() / 8 - 1;
    let cipher = Aes128::new(GenericArray::from_slice(kek));
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    let mut r = wrapped[8..].to_vec();
    for j in (0..6).rev() {
        for i in (1..=n).rev() {
            let t = (n * j + i) as u64;
            let mut block = [0u8; 16];
            for (k, b) in t.to_be_bytes().iter().enumerate() {
                block[k] = a[k] ^ b;
            }
            block[8..].copy_from_slice(&r[(i - 1) * 8..i * 8]);
            let mut b = GenericArray::from(block);
            cipher.decrypt_block(&mut b);
            a.copy_from_slice(&b[..8]);
            r[(i - 1) * 8..i * 8].copy_from_slice(&b[8..]);
        }
    }
    same(&a, &WRAP_IV).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> alloc::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn pmk_del_anexo_j_de_802_11() {
        // IEEE 802.11-2020, J.4.2 (el mismo vector que publica la Wi-Fi Alliance).
        assert_eq!(
            pmk("password", b"IEEE").to_vec(),
            hex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e")
        );
        assert_eq!(
            pmk("ThisIsAPassword", b"ThisIsASSID").to_vec(),
            hex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af")
        );
    }

    #[test]
    fn key_wrap_del_rfc_3394() {
        // RFC 3394, 4.1: KEK de 128 bits y una clave de 128 bits.
        let kek: [u8; 16] = hex("000102030405060708090A0B0C0D0E0F").try_into().unwrap();
        let wrapped = hex("1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5");
        assert_eq!(
            key_unwrap(&kek, &wrapped).unwrap(),
            hex("00112233445566778899AABBCCDDEEFF")
        );
        let mut bad = wrapped.clone();
        bad[10] ^= 1;
        assert!(key_unwrap(&kek, &bad).is_none());
        assert!(key_unwrap(&kek, &wrapped[..16]).is_none());
    }

    #[test]
    fn la_ptk_no_depende_del_orden_de_las_puntas() {
        let pmk = pmk("contraseña", b"Casa");
        let (a, b) = ([2u8; 6], [1u8; 6]);
        let (na, nb) = ([7u8; 32], [9u8; 32]);
        assert_eq!(ptk(&pmk, &a, &b, &na, &nb), ptk(&pmk, &b, &a, &nb, &na));
        assert_ne!(
            ptk(&pmk, &a, &b, &na, &nb).tk,
            ptk(&pmk, &a, &b, &nb, &nb).tk
        );
    }
}
