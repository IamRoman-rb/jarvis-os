//! CCMP (IEEE 802.11-2020, 12.5.3): el cifrado de las tramas de datos de WPA2.
//!
//! Es AES-CCM con un MIC de 8 bytes. Lo interesante es qué se protege y cómo:
//!
//! - Cada trama lleva un **PN** (número de paquete) de 48 bits que nunca se repite con la misma
//!   clave. Va en claro, en el encabezado CCMP de 8 bytes, y el que recibe descarta todo PN que
//!   no sea mayor al último que aceptó: así nadie puede repetir una trama vieja.
//! - El **nonce** junta la prioridad, la dirección del que manda (A2) y el PN.
//! - Los **datos adicionales** (AAD) son el encabezado MAC, autenticado pero no cifrado, con los
//!   bits que pueden cambiar en el camino en cero (reintento, ahorro de energía, el número de
//!   secuencia…): si no, una retransmisión no pasaría el MIC.

use alloc::vec::Vec;

use aes::Aes128;
use ccm::Ccm;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U8, U13};

use crate::frame::{Header, PROTECTED, TYPE_DATA};

type Aes128Ccm = Ccm<Aes128, U8, U13>;

/// Largo del encabezado CCMP y del MIC.
pub const HEADER: usize = 8;
pub const MIC: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No es una trama de datos protegida, o está cortada.
    Malformed,
    /// El PN no es mayor que el último: una trama repetida (o reenviada por un atacante).
    Replay,
    /// El MIC no coincide: otra clave, o la trama se modificó.
    Mic,
}

/// Una clave CCMP (la de la estación, TK, o la de grupo, GTK) con sus contadores.
pub struct Key {
    cipher: Aes128Ccm,
    /// Índice de la clave (0 para la de la estación; 1 o 2 para la de grupo).
    pub id: u8,
    /// El próximo PN que se usa al mandar.
    tx_pn: u64,
    /// El último PN aceptado de cada prioridad (TID 0..15). Empieza en `rsc`.
    rx_pn: [u64; 16],
}

impl Key {
    /// `rsc`: el último PN que ya usó el otro lado (el punto de acceso lo dice en el mensaje 3
    /// para la clave de grupo; para la de la estación es 0).
    pub fn new(key: &[u8; 16], id: u8, rsc: u64) -> Key {
        Key {
            cipher: Aes128Ccm::new(GenericArray::from_slice(key)),
            id,
            tx_pn: 1,
            rx_pn: [rsc; 16],
        }
    }

    fn nonce(h: &Header, pn: u64) -> [u8; 13] {
        let mut n = [0u8; 13];
        n[0] = h.qos.map_or(0, |q| (q & 0x0f) as u8);
        n[1..7].copy_from_slice(&h.addr2);
        n[7..].copy_from_slice(&pn.to_be_bytes()[2..]);
        n
    }

    fn aad(h: &Header) -> Vec<u8> {
        let mut a = Vec::with_capacity(30);
        // Subtipo: se borran los bits 4–6 (solo queda si es QoS). Banderas: se borran reintento,
        // ahorro de energía y "hay más"; protegida va en 1. Con QoS, también el bit de orden.
        let mut fc1 = (h.fc[1] & 0xc7) | PROTECTED;
        if h.qos.is_some() {
            fc1 &= 0x7f;
        }
        a.extend_from_slice(&[h.fc[0] & 0x8f, fc1]);
        a.extend_from_slice(&h.addr1);
        a.extend_from_slice(&h.addr2);
        a.extend_from_slice(&h.addr3);
        // De la secuencia queda solo el número de fragmento.
        a.extend_from_slice(&(h.seq & 0x000f).to_le_bytes());
        if let Some(a4) = h.addr4 {
            a.extend_from_slice(&a4);
        }
        if let Some(q) = h.qos {
            a.extend_from_slice(&[(q & 0x0f) as u8, 0]);
        }
        a
    }

    /// Cifra una trama de datos en claro (con su encabezado MAC) con el próximo PN.
    pub fn encrypt(&mut self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        let mut h = Header::parse(frame).ok_or(Error::Malformed)?;
        if h.kind() != TYPE_DATA {
            return Err(Error::Malformed);
        }
        h.fc[1] |= PROTECTED;
        let pn = self.tx_pn;
        self.tx_pn += 1;
        let mut out = Vec::with_capacity(frame.len() + HEADER + MIC);
        h.write(&mut out);
        let p = pn.to_le_bytes();
        // PN0, PN1, reservado, ExtIV (0x20) + índice de la clave, PN2..PN5.
        out.extend_from_slice(&[p[0], p[1], 0, 0x20 | (self.id << 6), p[2], p[3], p[4], p[5]]);
        let start = out.len();
        out.extend_from_slice(&frame[h.size()..]);
        let tag = self
            .cipher
            .encrypt_in_place_detached(
                GenericArray::from_slice(&Self::nonce(&h, pn)),
                &Self::aad(&h),
                &mut out[start..],
            )
            .map_err(|_| Error::Malformed)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Descifra una trama protegida: devuelve la trama en claro (sin el encabezado CCMP ni el
    /// MIC, y sin la bandera de protegida). Solo acepta PN nuevos.
    pub fn decrypt(&mut self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        let h = Header::parse(frame).ok_or(Error::Malformed)?;
        if h.kind() != TYPE_DATA || !h.protected() {
            return Err(Error::Malformed);
        }
        let at = h.size();
        let ccmp = frame.get(at..at + HEADER).ok_or(Error::Malformed)?;
        if ccmp[3] & 0x20 == 0 || frame.len() < at + HEADER + MIC {
            return Err(Error::Malformed);
        }
        let pn = u64::from_le_bytes([ccmp[0], ccmp[1], ccmp[4], ccmp[5], ccmp[6], ccmp[7], 0, 0]);
        let tid = h.qos.map_or(0, |q| (q & 0x0f) as usize);
        if pn <= self.rx_pn[tid] {
            return Err(Error::Replay);
        }
        let end = frame.len() - MIC;
        let mut body = frame[at + HEADER..end].to_vec();
        self.cipher
            .decrypt_in_place_detached(
                GenericArray::from_slice(&Self::nonce(&h, pn)),
                &Self::aad(&h),
                &mut body,
                GenericArray::from_slice(&frame[end..]),
            )
            .map_err(|_| Error::Mic)?;
        // Recién ahora (con el MIC bien) se avanza el contador: una trama falsa con un PN
        // enorme no puede bloquear las verdaderas.
        self.rx_pn[tid] = pn;
        let mut plain = h.clone();
        plain.fc[1] &= !PROTECTED;
        let mut out = Vec::with_capacity(at + body.len());
        plain.write(&mut out);
        out.extend_from_slice(&body);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;

    #[test]
    fn ida_y_vuelta_y_repeticiones() {
        let tk = [0x42; 16];
        let (ap, sta) = ([0xa0; 6], [0x5e; 6]);
        let eth = [&ap[..], &sta, &[0x08, 0x00], b"hola, punto de acceso"].concat();
        let plain = frame::from_ethernet(&eth, ap, 7).unwrap();
        let mut tx = Key::new(&tk, 0, 0);
        let mut rx = Key::new(&tk, 0, 0);
        let c1 = tx.encrypt(&plain).unwrap();
        assert_eq!(c1.len(), plain.len() + HEADER + MIC);
        assert!(
            !c1.windows(4).any(|w| w == b"hola"),
            "el texto no va en claro"
        );
        assert_eq!(rx.decrypt(&c1).unwrap(), plain);
        assert_eq!(
            rx.decrypt(&c1),
            Err(Error::Replay),
            "la misma trama otra vez"
        );
        let c2 = tx.encrypt(&plain).unwrap();
        let mut bad = c2.clone();
        bad[40] ^= 1;
        assert_eq!(rx.decrypt(&bad), Err(Error::Mic));
        assert_eq!(rx.decrypt(&c2).unwrap(), plain, "la falsa no gastó el PN");
        // Un reintento cambia una bandera del encabezado y el número de secuencia: pasa igual.
        let mut retry = tx.encrypt(&plain).unwrap();
        retry[1] |= frame::RETRY;
        retry[22] ^= 0x50;
        assert!(rx.decrypt(&retry).is_ok());
    }
}
