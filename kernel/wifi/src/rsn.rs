//! El elemento RSN (IEEE 802.11-2020, 9.4.2.24): qué cifrado y qué autenticación usa una red.
//!
//! Cada "suite" es un OUI de 3 bytes (00-0F-AC, el de 802.11) y un número: cifrados 2 = TKIP
//! y 4 = CCMP (AES); autenticaciones 2 = PSK (WPA2 personal), 8 = SAE (WPA3 personal),
//! 1 = 802.1X (empresarial).

use alloc::vec::Vec;

const OUI: [u8; 3] = [0x00, 0x0f, 0xac];
pub const CIPHER_TKIP: u8 = 2;
pub const CIPHER_CCMP: u8 = 4;
pub const AKM_PSK: u8 = 2;
pub const AKM_SAE: u8 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rsn {
    /// El cifrado de lo que va a todos (difusión).
    pub group: u8,
    pub pairwise: Vec<u8>,
    pub akm: Vec<u8>,
    pub caps: u16,
}

impl Rsn {
    /// Lee los datos del elemento (sin id ni largo). Suites de otro fabricante → 0.
    pub fn parse(d: &[u8]) -> Option<Rsn> {
        let u16le = |at: usize| Some(u16::from_le_bytes([*d.get(at)?, *d.get(at + 1)?]));
        let suite = |at: usize| -> Option<u8> {
            let s = d.get(at..at + 4)?;
            Some(if s[..3] == OUI { s[3] } else { 0 })
        };
        if u16le(0)? != 1 {
            return None;
        }
        let group = suite(2)?;
        let mut at = 6;
        let list = |at: &mut usize| -> Option<Vec<u8>> {
            let n = u16le(*at)? as usize;
            *at += 2;
            let v = (0..n)
                .map(|i| suite(*at + i * 4))
                .collect::<Option<Vec<_>>>()?;
            *at += n * 4;
            Some(v)
        };
        let pairwise = list(&mut at)?;
        let akm = list(&mut at)?;
        Some(Rsn {
            group,
            pairwise,
            akm,
            caps: u16le(at).unwrap_or(0),
        })
    }

    /// WPA2 personal con AES: lo que sabe hacer JARVIS-OS. (Una red WPA2/WPA3 mixta también:
    /// ofrece PSK además de SAE.) El grupo tiene que ser CCMP: TKIP está roto y no lo implementamos.
    pub fn is_wpa2_psk(&self) -> bool {
        self.group == CIPHER_CCMP
            && self.pairwise.contains(&CIPHER_CCMP)
            && self.akm.contains(&AKM_PSK)
    }
}

/// El elemento RSN que manda JARVIS-OS (id y largo incluidos): CCMP y PSK.
#[rustfmt::skip]
pub fn own_element() -> Vec<u8> {
    let [a, b, c] = OUI;
    alloc::vec![
        48, 20, // id y largo
        1, 0, // versión
        a, b, c, CIPHER_CCMP, // grupo
        1, 0, a, b, c, CIPHER_CCMP, // un cifrado por estación
        1, 0, a, b, c, AKM_PSK, // una autenticación
        0, 0, // capacidades
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_propio_se_lee_como_wpa2_psk() {
        let ie = own_element();
        assert_eq!(ie.len(), 22);
        let r = Rsn::parse(&ie[2..]).unwrap();
        assert!(r.is_wpa2_psk());
        assert_eq!(r.pairwise, alloc::vec![CIPHER_CCMP]);
    }

    #[test]
    fn wpa3_solo_y_tkip_no_se_soportan() {
        let mut ie = own_element();
        ie[2 + 2 + 4 + 2 + 4 + 2 + 3] = AKM_SAE; // la AKM
        assert!(!Rsn::parse(&ie[2..]).unwrap().is_wpa2_psk());
        let mut ie = own_element();
        ie[2 + 2 + 3] = CIPHER_TKIP; // el grupo
        assert!(!Rsn::parse(&ie[2..]).unwrap().is_wpa2_psk());
        assert!(Rsn::parse(&[2, 0]).is_none(), "versión desconocida");
        assert!(Rsn::parse(&ie[2..10]).is_none(), "cortado");
    }
}
