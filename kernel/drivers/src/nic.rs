//! Placas de red Intel (e1000) y Realtek (RTL8139, RTL8168/8111): el formato de sus anillos de
//! descriptores.
//!
//! Una placa de red moderna no copia tramas de a una por la CPU: el driver le deja en memoria
//! un **anillo de descriptores**, cada uno apuntando a un buffer. Para recibir, la placa llena
//! el siguiente buffer libre y marca el descriptor como "listo"; para mandar, el driver completa
//! un descriptor y le avisa a la placa moviendo el índice de la cola. Cada fabricante tiene su
//! formato. Acá están los tres que usa JARVIS-OS, sin registros.
//!
//! Referencias: Intel "PCI/PCI-X Family of Gigabit Ethernet Controllers Software Developer's
//! Manual" (8254x, §3.2 y §3.3) y 82574 datasheet; Realtek RTL8139D datasheet y "RTL8168/8111
//! Programming Guide"; drivers `e1000`, `8139too` y `r8169` de Linux.

use crate::le;

// --- Intel e1000 (8254x, 82574) -----------------------------------------------------------------

/// Descriptor de recepción "legacy" (16 bytes): dirección del buffer, largo, checksum, estado,
/// errores y VLAN.
pub fn e1000_rx_desc(buffer: u64) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&buffer.to_le_bytes());
    d
}

/// Qué llegó en un descriptor de recepción: `None` si la placa todavía no lo llenó (DD, bit 0
/// del estado); `Some(None)` si llegó con error o partido en varios buffers (se descarta).
pub fn e1000_rx_done(d: &[u8]) -> Option<Option<usize>> {
    let status = d[12];
    if status & 1 == 0 {
        return None;
    }
    let len = le(d, 8, 2) as usize;
    let eop = status & 2 != 0;
    let errors = d[13];
    Some((eop && errors == 0 && len > 0).then_some(len))
}

/// Descriptor de transmisión "legacy": fin de paquete (EOP), que la placa agregue el CRC (IFCS)
/// y que informe cuando terminó (RS).
pub fn e1000_tx_desc(buffer: u64, len: u16) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&buffer.to_le_bytes());
    d[8..10].copy_from_slice(&len.to_le_bytes());
    d[11] = 0b1011; // CMD: RS | IFCS | EOP
    d
}

/// ¿La placa terminó de mandar este descriptor? (DD en el estado.)
pub fn e1000_tx_done(d: &[u8]) -> bool {
    d[12] & 1 != 0
}

// --- Realtek RTL8139 ------------------------------------------------------------------------------

/// Tamaño del anillo de recepción del 8139 (8 KiB) más el margen para una trama que da la vuelta.
pub const RTL8139_RX_RING: usize = 8192;
pub const RTL8139_RX_ALLOC: usize = RTL8139_RX_RING + 16 + 1536;

/// Una trama en el anillo del 8139: cabecera de 4 bytes (estado, largo con CRC) y los datos.
/// Devuelve (inicio de los datos, largo sin CRC, desplazamiento de la próxima), o `None` si la
/// cabecera no es válida (hay que reiniciar la recepción).
pub fn rtl8139_rx(ring: &[u8], offset: usize) -> Option<(usize, usize, usize)> {
    let status = le(ring, offset, 2) as u16;
    let len = le(ring, offset + 2, 2) as usize;
    // Bit 0 (ROK): llegó bien. Errores: 1 (FAE), 2 (CRC), 3 (largo), 4 (corto).
    if status & 1 == 0 || status & 0b1_1110 != 0 || !(64..=1522).contains(&len) {
        return None;
    }
    let next = (offset + 4 + len + 3) & !3;
    Some((offset + 4, len - 4, next % RTL8139_RX_RING))
}

/// Qué escribir en CAPR después de consumir hasta `next`: el 8139 espera el desplazamiento
/// menos 16 (una peculiaridad del chip; así lo hace Linux).
pub fn rtl8139_capr(next: usize) -> u16 {
    (next as u16).wrapping_sub(16)
}

// --- Realtek RTL8168/8111 (driver r8169) ----------------------------------------------------------

/// Bits de `opts1` de los descriptores del 8168.
pub const R8169_OWN: u32 = 1 << 31;
pub const R8169_EOR: u32 = 1 << 30;
pub const R8169_FS: u32 = 1 << 29;
pub const R8169_LS: u32 = 1 << 28;
/// Recepción: error (RES).
pub const R8169_RX_RES: u32 = 1 << 21;

/// Un descriptor (16 bytes): `opts1` (dueño, fin de anillo, primer/último tramo, largo),
/// `opts2` (VLAN) y la dirección del buffer.
pub fn r8169_desc(opts1: u32, buffer: u64) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[..4].copy_from_slice(&opts1.to_le_bytes());
    d[8..16].copy_from_slice(&buffer.to_le_bytes());
    d
}

/// Descriptor de recepción vacío, de la placa, con buffer de `size` bytes.
pub fn r8169_rx_desc(buffer: u64, size: u16, last: bool) -> [u8; 16] {
    let eor = if last { R8169_EOR } else { 0 };
    r8169_desc(R8169_OWN | eor | (size as u32 & 0x3FFF), buffer)
}

/// Descriptor de transmisión de una trama completa.
pub fn r8169_tx_desc(buffer: u64, len: u16, last: bool) -> [u8; 16] {
    let eor = if last { R8169_EOR } else { 0 };
    r8169_desc(
        R8169_OWN | eor | R8169_FS | R8169_LS | (len as u32 & 0x3FFF),
        buffer,
    )
}

/// Igual que en e1000: `None` = todavía es de la placa; `Some(None)` = llegó mal; si no, el
/// largo sin el CRC.
pub fn r8169_rx_done(opts1: u32) -> Option<Option<usize>> {
    if opts1 & R8169_OWN != 0 {
        return None;
    }
    let whole = opts1 & R8169_FS != 0 && opts1 & R8169_LS != 0;
    let len = (opts1 & 0x3FFF) as usize;
    Some((whole && opts1 & R8169_RX_RES == 0 && len > 4).then(|| len - 4))
}
