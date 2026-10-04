//! Tramas 802.11 (IEEE 802.11-2020, capítulo 9).
//!
//! Toda trama empieza con el *frame control* (tipo, subtipo y banderas), la duración y hasta
//! cuatro direcciones. Qué es cada dirección depende de las banderas ToDS/FromDS: una estación
//! que le manda al punto de acceso pone ToDS, y entonces A1 = el punto de acceso (BSSID), A2 = ella
//! y A3 = el destino final. Los datos llevan adelante un encabezado LLC/SNAP con el ethertype: así
//! una trama Ethernet entra y sale de una de Wi-Fi sin perder nada.
//!
//! Las tramas de gestión (beacons, sondeo, autenticación, asociación) llevan campos fijos y
//! después *elementos de información*: id, largo y datos.

use alloc::vec::Vec;

use crate::Mac;

pub const TYPE_MGMT: u8 = 0;
pub const TYPE_DATA: u8 = 2;

pub const ASSOC_REQ: u8 = 0;
pub const ASSOC_RESP: u8 = 1;
pub const PROBE_REQ: u8 = 4;
pub const PROBE_RESP: u8 = 5;
pub const BEACON: u8 = 8;
pub const DISASSOC: u8 = 10;
pub const AUTH: u8 = 11;
pub const DEAUTH: u8 = 12;

// Banderas (segundo byte del frame control).
pub const TO_DS: u8 = 0x01;
pub const FROM_DS: u8 = 0x02;
pub const RETRY: u8 = 0x08;
pub const PROTECTED: u8 = 0x40;

// Elementos de información.
pub const IE_SSID: u8 = 0;
pub const IE_RATES: u8 = 1;
pub const IE_DS: u8 = 3;
pub const IE_HT_CAPS: u8 = 45;
pub const IE_RSN: u8 = 48;
pub const IE_EXT_RATES: u8 = 50;
pub const IE_VHT_CAPS: u8 = 191;
pub const IE_VENDOR: u8 = 221;
pub const ACTION: u8 = 13;

/// Velocidades básicas de 2,4 GHz (802.11b/g, en unidades de 500 kb/s; el bit alto = básica).
pub const RATES_2G: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
pub const EXT_RATES_2G: [u8; 4] = [0x30, 0x48, 0x60, 0x6c];
/// Las de 5 GHz (802.11a: no hay b).
pub const RATES_5G: [u8; 8] = [0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c];

/// LLC/SNAP: "lo que sigue es un ethertype".
const SNAP: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

/// El encabezado MAC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Los dos bytes del frame control, como vienen.
    pub fc: [u8; 2],
    pub duration: u16,
    pub addr1: Mac,
    pub addr2: Mac,
    pub addr3: Mac,
    /// Número de secuencia (12 bits) y de fragmento (4 bits).
    pub seq: u16,
    /// Solo con ToDS y FromDS (puentes inalámbricos).
    pub addr4: Option<Mac>,
    /// Solo en las tramas de datos QoS.
    pub qos: Option<u16>,
}

impl Header {
    pub fn kind(&self) -> u8 {
        (self.fc[0] >> 2) & 3
    }

    pub fn subtype(&self) -> u8 {
        self.fc[0] >> 4
    }

    pub fn flags(&self) -> u8 {
        self.fc[1]
    }

    pub fn protected(&self) -> bool {
        self.fc[1] & PROTECTED != 0
    }

    /// Largo del encabezado en la trama.
    pub fn size(&self) -> usize {
        24 + if self.addr4.is_some() { 6 } else { 0 } + if self.qos.is_some() { 2 } else { 0 }
    }

    /// El encabezado de una trama de gestión de `src` a `dst`, en la red `bssid`.
    pub fn mgmt(subtype: u8, dst: Mac, src: Mac, bssid: Mac, seq: u16) -> Header {
        Header {
            fc: [(subtype << 4) | (TYPE_MGMT << 2), 0],
            duration: 0,
            addr1: dst,
            addr2: src,
            addr3: bssid,
            seq: seq << 4,
            addr4: None,
            qos: None,
        }
    }

    pub fn parse(buf: &[u8]) -> Option<Header> {
        if buf.len() < 24 {
            return None;
        }
        let mac = |at: usize| -> Mac { buf[at..at + 6].try_into().unwrap_or([0; 6]) };
        let fc = [buf[0], buf[1]];
        let mut h = Header {
            fc,
            duration: u16::from_le_bytes([buf[2], buf[3]]),
            addr1: mac(4),
            addr2: mac(10),
            addr3: mac(16),
            seq: u16::from_le_bytes([buf[22], buf[23]]),
            addr4: None,
            qos: None,
        };
        let mut at = 24;
        if h.kind() == TYPE_DATA && fc[1] & (TO_DS | FROM_DS) == TO_DS | FROM_DS {
            if buf.len() < at + 6 {
                return None;
            }
            h.addr4 = Some(mac(at));
            at += 6;
        }
        // Subtipos de datos con el bit 3 = QoS.
        if h.kind() == TYPE_DATA && h.subtype() & 0x8 != 0 {
            if buf.len() < at + 2 {
                return None;
            }
            h.qos = Some(u16::from_le_bytes([buf[at], buf[at + 1]]));
        }
        Some(h)
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.fc);
        out.extend_from_slice(&self.duration.to_le_bytes());
        out.extend_from_slice(&self.addr1);
        out.extend_from_slice(&self.addr2);
        out.extend_from_slice(&self.addr3);
        out.extend_from_slice(&self.seq.to_le_bytes());
        if let Some(a) = self.addr4 {
            out.extend_from_slice(&a);
        }
        if let Some(q) = self.qos {
            out.extend_from_slice(&q.to_le_bytes());
        }
    }
}

// --- datos ↔ Ethernet ---------------------------------------------------------------------------

/// Una trama de datos que llega del punto de acceso (FromDS, ya descifrada) → trama Ethernet
/// (destino, origen, ethertype, datos). `None` si no es de datos o no trae LLC/SNAP.
pub fn to_ethernet(frame: &[u8]) -> Option<Vec<u8>> {
    let h = Header::parse(frame)?;
    if h.kind() != TYPE_DATA || h.subtype() & 0x4 != 0 {
        return None; // no es de datos, o es "sin datos" (null)
    }
    let (dst, src) = match h.flags() & (TO_DS | FROM_DS) {
        0 => (h.addr1, h.addr2),
        FROM_DS => (h.addr1, h.addr3),
        TO_DS => (h.addr3, h.addr2),
        _ => (h.addr3, h.addr4?),
    };
    let body = frame.get(h.size()..)?;
    if body.len() < 8 || body[..6] != SNAP {
        return None;
    }
    let mut out = Vec::with_capacity(14 + body.len() - 8);
    out.extend_from_slice(&dst);
    out.extend_from_slice(&src);
    out.extend_from_slice(&body[6..]);
    Some(out)
}

/// Una trama Ethernet que sale → trama de datos para el punto de acceso `bssid` (ToDS, sin
/// cifrar: eso lo hace [`crate::ccmp`]). `None` si la trama Ethernet es demasiado corta.
pub fn from_ethernet(eth: &[u8], bssid: Mac, seq: u16) -> Option<Vec<u8>> {
    data_frame(eth, bssid, seq, false)
}

/// Como [`from_ethernet`], pero con QoS (subtipo 8, prioridad 0 = mejor esfuerzo): la que usa
/// una estación 802.11n/ac con un punto de acceso con WMM.
pub fn from_ethernet_qos(eth: &[u8], bssid: Mac, seq: u16) -> Option<Vec<u8>> {
    data_frame(eth, bssid, seq, true)
}

fn data_frame(eth: &[u8], bssid: Mac, seq: u16, qos: bool) -> Option<Vec<u8>> {
    if eth.len() < 14 {
        return None;
    }
    let dst: Mac = eth[..6].try_into().ok()?;
    let src: Mac = eth[6..12].try_into().ok()?;
    let h = Header {
        fc: [(TYPE_DATA << 2) | if qos { 0x80 } else { 0 }, TO_DS],
        duration: 0,
        addr1: bssid,
        addr2: src,
        addr3: dst,
        seq: seq << 4,
        addr4: None,
        qos: qos.then_some(0),
    };
    let mut out = Vec::with_capacity(24 + 8 + eth.len() - 12);
    h.write(&mut out);
    out.extend_from_slice(&SNAP);
    out.extend_from_slice(&eth[12..]);
    Some(out)
}

// --- elementos de información -------------------------------------------------------------------

/// Recorre los elementos (id, datos). Se detiene en el primero mal formado.
pub fn elements(mut buf: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        if buf.len() < 2 || buf.len() < 2 + buf[1] as usize {
            return None;
        }
        let (id, len) = (buf[0], buf[1] as usize);
        let data = &buf[2..2 + len];
        buf = &buf[2 + len..];
        Some((id, data))
    })
}

fn element(out: &mut Vec<u8>, id: u8, data: &[u8]) {
    out.push(id);
    out.push(data.len().min(255) as u8);
    out.extend_from_slice(&data[..data.len().min(255)]);
}

// --- beacons y respuestas al sondeo -------------------------------------------------------------

/// Qué seguridad tiene una red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// Abierta.
    Open,
    /// WPA2 personal (PSK + CCMP): la que soporta JARVIS-OS.
    Wpa2Psk,
    /// Otra cosa (WEP, WPA1/TKIP, WPA3 solo, empresarial): se lista pero no se conecta.
    Unsupported,
}

/// Una red que se anunció.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bss {
    pub bssid: Mac,
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub security: Security,
    /// El elemento RSN tal cual vino (para compararlo con el del mensaje 3: si un atacante
    /// cambiara el beacon para bajar la seguridad, no coincidirían).
    pub rsn: Option<Vec<u8>>,
    /// Las velocidades que anuncia (para pedirle las mismas al asociarse).
    pub rates: Vec<u8>,
    /// 802.11n (HT), 802.11ac (VHT) y calidad de servicio (WMM).
    pub ht: bool,
    pub vht: bool,
    pub wmm: bool,
}

/// Las velocidades de 802.11b/g como bits (1, 2, 5,5 y 11 Mb/s en los bits 0–3; 6 a 54 en los
/// 4–11): el formato que espera el firmware de la placa.
pub fn rate_bits(rates: &[u8]) -> u16 {
    const ORDER: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];
    rates.iter().fold(0, |acc, r| {
        ORDER
            .iter()
            .position(|o| *o == r & 0x7f)
            .map_or(acc, |i| acc | (1 << i))
    })
}

/// El elemento WMM de Microsoft (OUI 00-50-F2, tipo 2).
fn is_wmm(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == [0x00, 0x50, 0xf2, 0x02]
}

/// Lee un beacon o una respuesta al sondeo.
pub fn parse_beacon(frame: &[u8]) -> Option<Bss> {
    let h = Header::parse(frame)?;
    if h.kind() != TYPE_MGMT || !matches!(h.subtype(), BEACON | PROBE_RESP) {
        return None;
    }
    // Marca de tiempo (8), intervalo (2), capacidades (2).
    let fixed = frame.get(24..36)?;
    let privacy = u16::from_le_bytes([fixed[10], fixed[11]]) & 0x0010 != 0;
    let mut bss = Bss {
        bssid: h.addr3,
        ssid: Vec::new(),
        channel: 0,
        security: if privacy {
            Security::Unsupported
        } else {
            Security::Open
        },
        rsn: None,
        rates: Vec::new(),
        ht: false,
        vht: false,
        wmm: false,
    };
    for (id, data) in elements(&frame[36..]) {
        match id {
            IE_SSID => bss.ssid = data.to_vec(),
            IE_RATES | IE_EXT_RATES => bss.rates.extend_from_slice(data),
            IE_DS if !data.is_empty() => bss.channel = data[0],
            IE_HT_CAPS => bss.ht = true,
            IE_VHT_CAPS => bss.vht = true,
            IE_VENDOR if is_wmm(data) => bss.wmm = true,
            IE_RSN => {
                let mut ie = Vec::with_capacity(data.len() + 2);
                element(&mut ie, IE_RSN, data);
                if crate::rsn::Rsn::parse(data).is_some_and(|r| r.is_wpa2_psk()) {
                    bss.security = Security::Wpa2Psk;
                }
                bss.rsn = Some(ie);
            }
            _ => {}
        }
    }
    Some(bss)
}

/// Pedido de sondeo: "¿quién anda por acá?" (SSID vacío = cualquiera).
pub fn probe_request(src: Mac, ssid: &[u8], seq: u16) -> Vec<u8> {
    let mut out = Vec::new();
    let b = crate::BROADCAST;
    Header::mgmt(PROBE_REQ, b, src, b, seq).write(&mut out);
    element(&mut out, IE_SSID, ssid);
    element(&mut out, IE_RATES, &RATES_2G);
    element(&mut out, IE_EXT_RATES, &EXT_RATES_2G);
    out
}

// --- autenticación y asociación -----------------------------------------------------------------

/// Autenticación abierta (algoritmo 0, paso 1). Con WPA2 la seguridad de verdad viene después
/// (el saludo de 4 vías): este paso solo existe por compatibilidad con WEP.
pub fn auth_request(src: Mac, bssid: Mac, seq: u16) -> Vec<u8> {
    let mut out = Vec::new();
    Header::mgmt(AUTH, bssid, src, bssid, seq).write(&mut out);
    out.extend_from_slice(&[0, 0, 1, 0, 0, 0]); // algoritmo, paso, estado
    out
}

/// La respuesta a la autenticación: `Some(estado)` (0 = aceptada) si es el paso 2 de `bssid`.
pub fn auth_response(frame: &[u8], bssid: Mac) -> Option<u16> {
    let h = Header::parse(frame)?;
    if h.kind() != TYPE_MGMT || h.subtype() != AUTH || h.addr2 != bssid {
        return None;
    }
    let f = frame.get(24..30)?;
    (u16::from_le_bytes([f[2], f[3]]) == 2).then(|| u16::from_le_bytes([f[4], f[5]]))
}

/// Pedido de asociación, con el elemento RSN propio si la red es WPA2.
pub fn assoc_request(src: Mac, bss: &Bss, rsn: Option<&[u8]>, seq: u16) -> Vec<u8> {
    let mut out = Vec::new();
    Header::mgmt(ASSOC_REQ, bss.bssid, src, bss.bssid, seq).write(&mut out);
    // Capacidades: ESS (somos una estación de una red con punto de acceso); privacidad si cifra.
    let caps: u16 = 0x0001 | if rsn.is_some() { 0x0010 } else { 0 };
    out.extend_from_slice(&caps.to_le_bytes());
    out.extend_from_slice(&10u16.to_le_bytes()); // intervalo de escucha, en beacons
    element(&mut out, IE_SSID, &bss.ssid);
    let rates = if bss.rates.is_empty() {
        &RATES_2G[..]
    } else {
        &bss.rates[..]
    };
    let (first, rest) = rates.split_at(rates.len().min(8));
    element(&mut out, IE_RATES, first);
    if !rest.is_empty() {
        element(&mut out, IE_EXT_RATES, rest);
    }
    if let Some(ie) = rsn {
        out.extend_from_slice(ie);
    }
    if bss.ht && bss.wmm {
        // 802.11n: un flujo, 20 MHz, sin ahorro de energía por MIMO (SMPS deshabilitado), y
        // recibe MCS 0–7.
        let mut ht = [0u8; 26];
        ht[0] = 0x0c;
        ht[3] = 0xff;
        element(&mut out, IE_HT_CAPS, &ht);
        if bss.vht && bss.channel > 14 {
            // 802.11ac: un flujo, MCS 0–9 al recibir y al mandar, el resto "no soportado".
            let mut vht = [0u8; 12];
            vht[4..6].copy_from_slice(&0xfffeu16.to_le_bytes());
            vht[8..10].copy_from_slice(&0xfffeu16.to_le_bytes());
            element(&mut out, IE_VHT_CAPS, &vht);
        }
    }
    if bss.wmm {
        // WMM: estación con QoS, sin U-APSD.
        element(
            &mut out,
            IE_VENDOR,
            &[0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00],
        );
    }
    out
}

/// La respuesta a la asociación, con lo que el punto de acceso acepta (sus velocidades y si
/// confirmó HT/VHT).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssocInfo {
    pub status: u16,
    pub aid: u16,
    pub rates: Vec<u8>,
    pub ht: bool,
    pub vht: bool,
}

pub fn assoc_info(frame: &[u8], bssid: Mac) -> Option<AssocInfo> {
    let (status, aid) = assoc_response(frame, bssid)?;
    let mut info = AssocInfo {
        status,
        aid,
        ..AssocInfo::default()
    };
    for (id, data) in elements(frame.get(30..).unwrap_or(&[])) {
        match id {
            IE_RATES | IE_EXT_RATES => info.rates.extend_from_slice(data),
            IE_HT_CAPS => info.ht = true,
            IE_VHT_CAPS => info.vht = true,
            _ => {}
        }
    }
    Some(info)
}

/// Un pedido de Block Ack (ADDBA): la respuesta "rechazado" (estado 37). La estación no junta
/// tramas en ráfagas (A-MPDU): el punto de acceso sigue mandando de a una.
pub fn addba_refusal(frame: &[u8], own: Mac, seq: u16) -> Option<Vec<u8>> {
    let h = Header::parse(frame)?;
    let body = frame.get(24..)?;
    // Categoría 3 (Block Ack), acción 0 (pedido): token, parámetros (2), tiempo (2), inicio (2).
    if h.kind() != TYPE_MGMT || h.subtype() != ACTION || body.len() < 9 || body[..2] != [3, 0] {
        return None;
    }
    let mut out = Vec::new();
    Header::mgmt(ACTION, h.addr2, own, h.addr3, seq).write(&mut out);
    out.extend_from_slice(&[3, 1, body[2]]); // respuesta, mismo token
    out.extend_from_slice(&37u16.to_le_bytes());
    out.extend_from_slice(&body[3..5]); // los parámetros que pidió
    out.extend_from_slice(&0u16.to_le_bytes());
    Some(out)
}

/// Desautenticarse (al desconectarse por pedido).
pub fn deauth(own: Mac, bssid: Mac, reason: u16, seq: u16) -> Vec<u8> {
    let mut out = Vec::new();
    Header::mgmt(DEAUTH, bssid, own, bssid, seq).write(&mut out);
    out.extend_from_slice(&reason.to_le_bytes());
    out
}

/// La respuesta a la asociación: `Some((estado, AID))`.
pub fn assoc_response(frame: &[u8], bssid: Mac) -> Option<(u16, u16)> {
    let h = Header::parse(frame)?;
    if h.kind() != TYPE_MGMT || h.subtype() != ASSOC_RESP || h.addr2 != bssid {
        return None;
    }
    let f = frame.get(24..30)?;
    let status = u16::from_le_bytes([f[2], f[3]]);
    Some((status, u16::from_le_bytes([f[4], f[5]]) & 0x3fff))
}

/// Si es una desautenticación o desasociación de `bssid`: el motivo.
pub fn disconnected(frame: &[u8], bssid: Mac) -> Option<u16> {
    let h = Header::parse(frame)?;
    let sub = h.subtype();
    if h.kind() != TYPE_MGMT || !matches!(sub, DEAUTH | DISASSOC) || h.addr2 != bssid {
        return None;
    }
    let f = frame.get(24..26)?;
    Some(u16::from_le_bytes([f[0], f[1]]))
}
