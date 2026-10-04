//! Los descriptores: lo que va adelante de cada trama (al mandar y al recibir) y los de los
//! anillos de la PCI (`tx.h`, `rx.h`, `pci.h` de rtw88).
//!
//! **Al mandar**, cada paquete empieza con un descriptor de 48 bytes que dice a qué cola va,
//! a qué velocidad, si la placa pone el número de secuencia, etc. El anillo de la PCI no apunta
//! al paquete directo: tiene *buffer descriptors* de 16 bytes, uno con el descriptor y otro con
//! la trama.
//!
//! **Al recibir**, la placa escribe 24 bytes de descriptor, después (si se pidió) el estado de
//! la PHY (la señal con que llegó) y después la trama 802.11 con su CRC.

pub const TX_DESC_SIZE: usize = 48;
pub const RX_DESC_SIZE: usize = 24;
/// Tamaño de cada buffer de recepción (11 KiB de trama + descriptor), como Linux.
pub const RX_BUF_SIZE: usize = 11454 + RX_DESC_SIZE;

// Colas del descriptor (QSEL).
pub const QSEL_BE: u8 = 0;
pub const QSEL_BEACON: u8 = 16;
pub const QSEL_MGMT: u8 = 18;
pub const QSEL_H2C: u8 = 19;

// Velocidades (DESC_RATE*).
pub const RATE_1M: u8 = 0x00;
pub const RATE_6M: u8 = 0x04;

/// Lo que se puede pedir en un descriptor de transmisión.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxInfo {
    pub size: u16,
    /// Dónde empieza la trama (48, salvo en los H2C).
    pub offset: u8,
    pub broadcast: bool,
    /// Último segmento del paquete (todo, salvo los H2C).
    pub last: bool,
    pub mac_id: u8,
    pub qsel: u8,
    pub rate_id: u8,
    /// Usar esta velocidad (si no, la elige el firmware).
    pub use_rate: bool,
    pub rate: u8,
    pub no_fallback: bool,
    /// La placa pone el número de secuencia.
    pub hw_seq: bool,
    pub seq: u16,
}

/// Arma un descriptor de transmisión (`rtw_tx_fill_tx_desc`).
pub fn tx_desc(i: &TxInfo) -> [u8; TX_DESC_SIZE] {
    let mut w = [0u32; 12];
    w[0] = u32::from(i.size)
        | u32::from(i.offset) << 16
        | u32::from(i.broadcast) << 24
        | u32::from(i.last) << 26
        | u32::from(i.hw_seq) << 31;
    w[1] = u32::from(i.mac_id) | u32::from(i.qsel & 0x1f) << 8 | u32::from(i.rate_id & 0x1f) << 16;
    w[3] = u32::from(i.use_rate) << 8 | u32::from(i.no_fallback) << 10;
    w[4] = u32::from(i.rate & 0x7f);
    w[8] = u32::from(i.hw_seq) << 15;
    w[9] = u32::from(i.seq & 0xfff) << 12;
    let mut d = [0u8; TX_DESC_SIZE];
    for (k, word) in w.iter().enumerate() {
        d[k * 4..k * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    d
}

fn is_group(addr1: &[u8]) -> bool {
    addr1.first().is_some_and(|b| b & 1 != 0)
}

/// Descriptor de una página reservada (la carga del firmware): cola de beacons, 1 Mb/s fijo,
/// secuencia por hardware.
pub fn rsvd_page_desc(size: usize) -> [u8; TX_DESC_SIZE] {
    tx_desc(&TxInfo {
        size: size as u16,
        offset: TX_DESC_SIZE as u8,
        last: true,
        qsel: QSEL_BEACON,
        rate_id: 8, // RTW_RATEID_B_20M
        use_rate: true,
        rate: RATE_1M,
        no_fallback: true,
        hw_seq: true,
        ..TxInfo::default()
    })
}

/// Descriptor de un paquete H2C: solo el largo y la cola (así lo arma Linux, sin desplazamiento).
pub fn h2c_desc(size: usize) -> [u8; TX_DESC_SIZE] {
    tx_desc(&TxInfo {
        size: size as u16,
        qsel: QSEL_H2C,
        ..TxInfo::default()
    })
}

/// Descriptor de una trama de gestión: cola de gestión, velocidad básica fija (1 Mb/s en 2,4 GHz,
/// 6 en 5) y secuencia por hardware.
pub fn mgmt_desc(frame: &[u8], five_ghz: bool) -> [u8; TX_DESC_SIZE] {
    tx_desc(&TxInfo {
        size: frame.len() as u16,
        offset: TX_DESC_SIZE as u8,
        broadcast: is_group(frame.get(4..10).unwrap_or(&[])),
        last: true,
        qsel: QSEL_MGMT,
        rate_id: if five_ghz { 7 } else { 8 }, // RTW_RATEID_G / B_20M
        use_rate: true,
        rate: if five_ghz { RATE_6M } else { RATE_1M },
        no_fallback: true,
        hw_seq: true,
        ..TxInfo::default()
    })
}

/// Descriptor de una trama de datos: cola de mejor esfuerzo; la velocidad la elige el firmware
/// (si no está conectada todavía, la básica).
pub fn data_desc(frame: &[u8], rate_id: u8, connected: bool, five_ghz: bool) -> [u8; TX_DESC_SIZE] {
    let seq = frame
        .get(22..24)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]) >> 4);
    tx_desc(&TxInfo {
        size: frame.len() as u16,
        offset: TX_DESC_SIZE as u8,
        broadcast: is_group(frame.get(4..10).unwrap_or(&[])),
        last: true,
        qsel: QSEL_BE,
        rate_id,
        use_rate: !connected,
        rate: if five_ghz { RATE_6M } else { RATE_1M },
        no_fallback: !connected,
        seq,
        ..TxInfo::default()
    })
}

/// Los dos *buffer descriptors* de la PCI de un paquete (descriptor + trama) en `addr`
/// (física). La cola de beacons lleva además el bit OWN.
pub fn tx_bd(addr: u64, total: usize, beacon: bool) -> [u8; 16] {
    let mut d = [0u8; 16];
    let mut psb = ((total.max(1) - 1) / 128 + 1) as u16;
    if beacon {
        psb |= 1 << 15;
    }
    d[0..2].copy_from_slice(&(TX_DESC_SIZE as u16).to_le_bytes());
    d[2..4].copy_from_slice(&psb.to_le_bytes());
    d[4..8].copy_from_slice(&(addr as u32).to_le_bytes());
    d[8..10].copy_from_slice(&((total - TX_DESC_SIZE) as u16).to_le_bytes());
    d[12..16].copy_from_slice(&((addr + TX_DESC_SIZE as u64) as u32).to_le_bytes());
    d
}

/// El *buffer descriptor* de recepción: tamaño del buffer y su dirección física.
pub fn rx_bd(addr: u64) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[0..2].copy_from_slice(&(RX_BUF_SIZE as u16).to_le_bytes());
    d[4..8].copy_from_slice(&(addr as u32).to_le_bytes());
    d
}

/// Qué llegó en un buffer de recepción.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rx {
    /// Dónde empieza la trama 802.11 y cuánto mide (sin el CRC).
    pub offset: usize,
    pub len: usize,
    /// Un mensaje del firmware (C2H), no una trama.
    pub c2h: bool,
    /// La señal en dBm, si vino el estado de la PHY.
    pub rssi: Option<i8>,
    /// La placa la descifró (con la seguridad apagada, nunca).
    pub decrypted: bool,
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// La señal de un paquete CCK (página 0 del estado de la PHY): ganancia del LNA menos el doble
/// de la del VGA.
fn cck_power(phy: &[u8], rfe: u8) -> i8 {
    const LNA0: [i8; 8] = [22, 8, -6, -22, -31, -40, -46, -52];
    const LNA1: [i8; 16] = [
        10, 6, 2, -2, -6, -10, -14, -17, -20, -24, -28, -31, -34, -37, -40, -44,
    ];
    let w3 = le32(phy, 12);
    let vga = ((w3 >> 8) & 0x1f) as i8;
    let lna = (((w3 >> 23) & 1) << 3 | ((w3 >> 13) & 7)) as usize;
    let gain = if rfe == 0 {
        LNA0.get(lna).copied()
    } else {
        LNA1.get(lna).copied()
    };
    gain.map_or(-120, |g| g.saturating_sub(vga.saturating_mul(2)))
}

/// Lee el descriptor de recepción (`rtw_rx_query_rx_desc`). `None` si está mal formado o la
/// trama llegó con error de CRC.
pub fn parse_rx(buf: &[u8], rfe: u8) -> Option<Rx> {
    if buf.len() < RX_DESC_SIZE {
        return None;
    }
    let w0 = le32(buf, 0);
    let w2 = le32(buf, 8);
    let pkt_len = (w0 & 0x3fff) as usize;
    let crc_err = w0 & (1 << 14) != 0;
    let drv_info = ((w0 >> 16) & 0xf) as usize;
    let enc = (w0 >> 20) & 7;
    let shift = ((w0 >> 24) & 3) as usize;
    let physt = w0 & (1 << 26) != 0;
    let swdec = w0 & (1 << 27) != 0;
    let c2h = w2 & (1 << 28) != 0;
    // El estado de la PHY mide 4 × 8 bytes (PHY_STATUS_SIZE); otro tamaño es basura.
    if (drv_info != 0 && drv_info != 4) || (physt && drv_info == 0) || pkt_len > 11454 {
        return None;
    }
    let phy_at = RX_DESC_SIZE + shift;
    let offset = phy_at + drv_info * 8;
    if c2h {
        return Some(Rx {
            offset,
            len: pkt_len.min(buf.len().saturating_sub(offset)),
            c2h: true,
            rssi: None,
            decrypted: false,
        });
    }
    if crc_err || pkt_len <= 4 || offset + pkt_len > buf.len() {
        return None;
    }
    let rssi = physt.then(|| {
        let phy = &buf[phy_at..offset];
        match phy[0] & 0xf {
            0 => cck_power(phy, rfe),
            _ => (((le32(phy, 0) >> 8) & 0xff) as i16 - 110).clamp(-120, 0) as i8,
        }
    });
    Some(Rx {
        offset,
        len: pkt_len - 4,
        c2h: false,
        rssi,
        decrypted: !swdec && enc != 0,
    })
}
