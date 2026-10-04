//! El saludo de 4 vías de WPA2 (IEEE 802.11-2020, 12.7.6), del lado de la estación.
//!
//! Los dos lados ya saben la PMK (sale de la contraseña) pero nunca la mandan. En cambio:
//!
//! 1. El punto de acceso manda un número al azar (ANonce).
//! 2. La estación elige el suyo (SNonce), calcula la PTK y contesta con su SNonce, firmado con la
//!    KCK. Si el punto de acceso puede verificar la firma, la estación sabe la contraseña.
//! 3. El punto de acceso contesta firmado (así la estación sabe que él también la sabe), con la
//!    clave de grupo (GTK) cifrada con la KEK y su elemento RSN, que tiene que ser el mismo del
//!    beacon: si alguien cambió el beacon para bajar la seguridad, acá se nota.
//! 4. La estación confirma, y los dos instalan las claves.
//!
//! Cada mensaje lleva un contador (*replay counter*) que solo crece; los mensajes firmados con uno
//! viejo se descartan. El mensaje 1 no va firmado: no se le cree el contador.
//!
//! Los mensajes van en tramas EAPOL (ethertype 0x888E) dentro de tramas de datos, todavía sin
//! cifrar. [`Supplicant::handle`] recibe el EAPOL (lo que sigue al ethertype) y devuelve qué
//! mandar y qué claves instalar.

use alloc::vec;
use alloc::vec::Vec;

use crate::Mac;
use crate::crypto::{self, Ptk};
use crate::frame::{IE_RSN, elements};

pub const ETHERTYPE: u16 = 0x888e;

// Bits de "key information".
const VERSION_AES: u16 = 2; // HMAC-SHA1 + AES Key Wrap
const PAIRWISE: u16 = 1 << 3;
const INSTALL: u16 = 1 << 6;
const ACK: u16 = 1 << 7;
const MIC: u16 = 1 << 8;
const SECURE: u16 = 1 << 9;
const ENCRYPTED: u16 = 1 << 12;

/// Dónde está el MIC dentro del EAPOL: 4 de encabezado + 77 del descriptor.
const MIC_AT: usize = 81;
/// Largo fijo del EAPOL-Key (sin los datos).
const FIXED: usize = 99;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No es un EAPOL-Key RSN versión 2, o está cortado.
    Malformed,
    /// Llegó un mensaje en un momento que no corresponde (el 3 antes del 1, por ejemplo).
    State,
    /// Contador viejo.
    Replay,
    /// Firma inválida: la contraseña no es la misma (o alguien está en el medio).
    Mic,
    /// El ANonce del mensaje 3 no es el del 1.
    Nonce,
    /// El RSN del mensaje 3 no es el del beacon.
    Rsn,
    /// La clave de grupo no se pudo desenvolver o no vino.
    Gtk,
}

/// Lo que hay que hacer después de un mensaje.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Mandar este EAPOL al punto de acceso.
    Send(Vec<u8>),
    /// Instalar la clave de la estación (TK, para CCMP).
    Pairwise([u8; 16]),
    /// Instalar la clave de grupo, con su índice y el último PN que usó el punto de acceso.
    Group { key: Vec<u8>, id: u8, rsc: u64 },
}

/// Un EAPOL-Key leído.
struct KeyFrame<'a> {
    version: u8,
    info: u16,
    replay: u64,
    nonce: [u8; 32],
    rsc: u64,
    mic: [u8; 16],
    data: &'a [u8],
}

fn parse(f: &[u8]) -> Option<KeyFrame<'_>> {
    // Encabezado: versión, tipo (3 = Key), largo. Descriptor 2 = RSN.
    if f.len() < FIXED || f[1] != 3 || f[4] != 2 {
        return None;
    }
    let body = u16::from_be_bytes([f[2], f[3]]) as usize;
    let f = f.get(..4 + body)?;
    let data_len = u16::from_be_bytes([f[97], f[98]]) as usize;
    Some(KeyFrame {
        version: f[0],
        info: u16::from_be_bytes([f[5], f[6]]),
        replay: u64::from_be_bytes(f[9..17].try_into().ok()?),
        nonce: f[17..49].try_into().ok()?,
        rsc: u64::from_le_bytes(f[65..73].try_into().ok()?),
        mic: f[MIC_AT..MIC_AT + 16].try_into().ok()?,
        data: f.get(FIXED..FIXED + data_len)?,
    })
}

/// Arma un EAPOL-Key (largo de clave 0, IV y RSC en cero) y lo firma con `kck`.
fn build(
    version: u8,
    info: u16,
    replay: u64,
    nonce: &[u8; 32],
    data: &[u8],
    kck: &[u8; 16],
) -> Vec<u8> {
    let mut f = vec![0u8; FIXED + data.len()];
    f[0] = version;
    f[1] = 3;
    f[2..4].copy_from_slice(&((FIXED - 4 + data.len()) as u16).to_be_bytes());
    f[4] = 2;
    f[5..7].copy_from_slice(&info.to_be_bytes());
    f[9..17].copy_from_slice(&replay.to_be_bytes());
    f[17..49].copy_from_slice(nonce);
    f[97..99].copy_from_slice(&(data.len() as u16).to_be_bytes());
    f[FIXED..].copy_from_slice(data);
    let mic = crypto::eapol_mic(kck, &f);
    f[MIC_AT..MIC_AT + 16].copy_from_slice(&mic);
    f
}

/// ¿El MIC de `f` es el que da la KCK?
fn mic_ok(kck: &[u8; 16], f: &[u8], k: &KeyFrame<'_>) -> bool {
    let mut copy = f[..FIXED + k.data.len()].to_vec();
    copy[MIC_AT..MIC_AT + 16].fill(0);
    crypto::same(&crypto::eapol_mic(kck, &copy), &k.mic)
}

/// La clave de grupo que viene en los datos (ya descifrados) de un mensaje: el KDE 00-0F-AC:1.
fn gtk(data: &[u8]) -> Option<(u8, Vec<u8>)> {
    elements(data).find_map(|(id, d)| {
        (id == 0xdd && d.len() > 6 && d[..4] == [0x00, 0x0f, 0xac, 1])
            .then(|| (d[4] & 3, d[6..].to_vec()))
    })
}

/// La estación en el saludo.
pub struct Supplicant {
    own: Mac,
    ap: Mac,
    pmk: [u8; 32],
    snonce: [u8; 32],
    own_rsn: Vec<u8>,
    ap_rsn: Option<Vec<u8>>,
    anonce: Option<[u8; 32]>,
    ptk: Option<Ptk>,
    /// El último contador de un mensaje firmado que se aceptó.
    replay: Option<u64>,
    /// Ya terminó el saludo de 4 vías (después solo llegan cambios de la clave de grupo).
    done: bool,
}

impl Supplicant {
    /// `snonce`: 32 bytes al azar (del generador del kernel). `own_rsn`: el elemento RSN que se
    /// mandó al asociarse; `ap_rsn`: el del beacon.
    pub fn new(
        own: Mac,
        ap: Mac,
        pmk: [u8; 32],
        snonce: [u8; 32],
        own_rsn: Vec<u8>,
        ap_rsn: Option<Vec<u8>>,
    ) -> Supplicant {
        Supplicant {
            own,
            ap,
            pmk,
            snonce,
            own_rsn,
            ap_rsn,
            anonce: None,
            ptk: None,
            replay: None,
            done: false,
        }
    }

    pub fn ptk(&self) -> Option<&Ptk> {
        self.ptk.as_ref()
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn handle(&mut self, f: &[u8]) -> Result<Vec<Output>, Error> {
        let k = parse(f).ok_or(Error::Malformed)?;
        if k.info & 7 != VERSION_AES || k.info & ACK == 0 {
            return Err(Error::Malformed);
        }
        let pairwise = k.info & PAIRWISE != 0;
        if pairwise && k.info & MIC == 0 {
            return Ok(self.message1(&k));
        }
        // Todo lo demás va firmado.
        let ptk = self.ptk.clone().ok_or(Error::State)?;
        if self.replay.is_some_and(|r| k.replay <= r) {
            return Err(Error::Replay);
        }
        if !mic_ok(&ptk.kck, f, &k) {
            return Err(Error::Mic);
        }
        if k.info & ENCRYPTED == 0 {
            return Err(Error::Malformed);
        }
        if pairwise && k.info & INSTALL != 0 {
            if self.anonce != Some(k.nonce) {
                return Err(Error::Nonce);
            }
            self.replay = Some(k.replay);
            self.message3(&k, &ptk)
        } else if !pairwise && self.done {
            self.replay = Some(k.replay);
            // Cambio de la clave de grupo (mensaje 1 de 2 del saludo de grupo).
            let data = crypto::key_unwrap(&ptk.kek, k.data).ok_or(Error::Gtk)?;
            let (id, key) = gtk(&data).ok_or(Error::Gtk)?;
            let ack = build(
                k.version,
                VERSION_AES | MIC | SECURE,
                k.replay,
                &[0; 32],
                &[],
                &ptk.kck,
            );
            Ok(vec![
                Output::Send(ack),
                Output::Group {
                    key,
                    id,
                    rsc: k.rsc,
                },
            ])
        } else {
            Err(Error::State)
        }
    }

    fn message1(&mut self, k: &KeyFrame<'_>) -> Vec<Output> {
        let ptk = crypto::ptk(&self.pmk, &self.ap, &self.own, &k.nonce, &self.snonce);
        let m2 = build(
            k.version,
            VERSION_AES | PAIRWISE | MIC,
            k.replay,
            &self.snonce,
            &self.own_rsn,
            &ptk.kck,
        );
        self.anonce = Some(k.nonce);
        self.ptk = Some(ptk);
        vec![Output::Send(m2)]
    }

    fn message3(&mut self, k: &KeyFrame<'_>, ptk: &Ptk) -> Result<Vec<Output>, Error> {
        let data = crypto::key_unwrap(&ptk.kek, k.data).ok_or(Error::Gtk)?;
        if let Some(beacon) = &self.ap_rsn {
            let rsn = elements(&data)
                .find(|(id, _)| *id == IE_RSN)
                .ok_or(Error::Rsn)?;
            if beacon.get(2..) != Some(rsn.1) {
                return Err(Error::Rsn);
            }
        }
        let (id, key) = gtk(&data).ok_or(Error::Gtk)?;
        let m4 = build(
            k.version,
            VERSION_AES | PAIRWISE | MIC | SECURE,
            k.replay,
            &[0; 32],
            &[],
            &ptk.kck,
        );
        self.done = true;
        Ok(vec![
            Output::Send(m4),
            Output::Pairwise(ptk.tk),
            Output::Group {
                key,
                id,
                rsc: k.rsc,
            },
        ])
    }
}
