//! El efuse: la memoria de fábrica de la placa (512 bytes que se queman una sola vez).
//!
//! Guarda la dirección MAC y la calibración de cada unidad: potencia de transmisión por canal,
//! el ajuste del cristal, el tipo de antena (RFE). Como se escribe quemando fusibles, no se
//! puede corregir: cada actualización se *agrega* al final en bloques con un encabezado que dice
//! a qué parte del mapa lógico va. El mapa lógico empieza en 0xFF y se arma recorriendo los
//! bloques en orden (`rtw_dump_logical_efuse_map`).

use super::{Bus, BusExt, Error, regs};

pub const PHYSICAL_SIZE: usize = 512;
pub const LOGICAL_SIZE: usize = 512;
/// Los últimos 96 bytes físicos están protegidos (no son del mapa).
const PROTECT_SIZE: usize = 96;

/// Lee el efuse físico byte por byte (`rtw_dump_physical_efuse_map`).
pub fn read_physical<B: Bus + ?Sized>(bus: &mut B) -> Result<[u8; PHYSICAL_SIZE], Error> {
    // Banco del Wi-Fi y el regulador de 2,5 V apagado (solo hace falta para escribir).
    bus.write32_mask(regs::LDO_EFUSE_CTRL, (1 << 8) | (1 << 9), 0);
    bus.clr8(regs::LDO_EFUSE_CTRL + 3, 1 << 7);
    let mut map = [0u8; PHYSICAL_SIZE];
    let mut ctl = bus.read32(regs::EFUSE_CTRL);
    for (addr, byte) in map.iter_mut().enumerate() {
        ctl &= !(0xff | (0x3ff << 8));
        ctl |= (addr as u32 & 0x3ff) << 8;
        bus.write32(regs::EFUSE_CTRL, ctl & !regs::EF_FLAG);
        let mut tries = 0;
        loop {
            bus.delay_us(1);
            ctl = bus.read32(regs::EFUSE_CTRL);
            if ctl & regs::EF_FLAG != 0 {
                break;
            }
            tries += 1;
            if tries > 100_000 {
                return Err(Error::Efuse);
            }
        }
        *byte = ctl as u8;
    }
    Ok(map)
}

/// Del mapa físico al lógico.
///
/// Encabezado de un bloque: un byte `bbbb wwww` (bloque, palabras), o dos bytes si el bloque no
/// entra en 4 bits: `bbb0 1111` + `bbbb wwww`. Cada bit de `wwww` en 0 dice que sigue esa
/// palabra (2 bytes) del bloque; en 1, que esa palabra no se escribió.
pub fn logical_map(phy: &[u8]) -> Result<[u8; LOGICAL_SIZE], Error> {
    let mut log = [0xffu8; LOGICAL_SIZE];
    let end = phy.len().min(PHYSICAL_SIZE) - PROTECT_SIZE;
    let mut i = 0;
    while i + 1 < end {
        let (h1, h2) = (phy[i], phy[i + 1]);
        if h1 == 0xff || (h1 & 0x1f == 0x0f && h2 == 0xff) {
            break; // lo que sigue nunca se escribió
        }
        let (block, words) = if h1 & 0x1f == 0x0f {
            i += 2;
            (((h2 & 0xf0) >> 1) | ((h1 >> 5) & 0x07), h2 & 0x0f)
        } else {
            i += 1;
            ((h1 & 0xf0) >> 4, h1 & 0x0f)
        };
        for w in 0..4 {
            if words & (1 << w) != 0 {
                continue;
            }
            let at = (usize::from(block) << 3) + (w << 1);
            if i + 1 >= end || at + 1 >= LOGICAL_SIZE {
                return Err(Error::Efuse);
            }
            log[at] = phy[i];
            log[at + 1] = phy[i + 1];
            i += 2;
        }
    }
    Ok(log)
}

/// La potencia de un camino en 2,4 GHz: bases por grupo de canales y diferencias (4 bits con
/// signo) para OFDM y 20 MHz.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Power2g {
    pub cck_base: [u8; 6],
    pub bw40_base: [u8; 5],
    pub ofdm_diff: i8,
    pub bw20_diff: i8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Power5g {
    pub bw40_base: [u8; 14],
    pub ofdm_diff: i8,
    pub bw20_diff: i8,
    pub vht80_diff: i8,
}

/// Lo que importa del efuse de la RTL8821CE (`struct rtw8821c_efuse`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Efuse {
    pub mac: [u8; 6],
    /// Tipo de antena y conmutador (*RFE option*): elige variantes de las tablas.
    pub rfe_option: u8,
    pub crystal_cap: u8,
    pub thermal_meter: u8,
    pub rf_board_option: u8,
    pub tx_bb_swing_2g: u8,
    pub tx_bb_swing_5g: u8,
    pub country: [u8; 2],
    pub power_2g: Power2g,
    pub power_5g: Power5g,
    /// Comparte la antena con el Bluetooth.
    pub share_ant: bool,
}

/// 4 bits con signo.
fn nibble(b: u8) -> i8 {
    ((b << 4) as i8) >> 4
}

/// Interpreta el mapa lógico. Los campos sin escribir (0xFF) toman el valor de Linux.
pub fn parse(log: &[u8; LOGICAL_SIZE]) -> Efuse {
    let fix = |v: u8, default: u8| if v == 0xff { default } else { v };
    // Camino A: 0x10..0x3A (2,4 GHz, 18 bytes; 5 GHz, 24).
    let p = &log[0x10..0x10 + 42];
    let mut power_2g = Power2g::default();
    power_2g.cck_base.copy_from_slice(&p[0..6]);
    power_2g.bw40_base.copy_from_slice(&p[6..11]);
    power_2g.ofdm_diff = nibble(p[11] & 0x0f);
    power_2g.bw20_diff = nibble(p[11] >> 4);
    let q = &p[18..];
    let mut power_5g = Power5g::default();
    power_5g.bw40_base.copy_from_slice(&q[0..14]);
    power_5g.ofdm_diff = nibble(q[14] & 0x0f);
    power_5g.bw20_diff = nibble(q[14] >> 4);
    power_5g.vht80_diff = nibble(q[20] >> 4);
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&log[0xd0..0xd6]);
    let rfe = log[0xca];
    let board = fix(log[0xc1], 0);
    let bt = log[0xc3];
    Efuse {
        mac,
        rfe_option: if rfe == 0xff { 0 } else { rfe & 0x1f },
        crystal_cap: fix(log[0xb9], 0) & 0x3f,
        thermal_meter: log[0xba],
        rf_board_option: board,
        tx_bb_swing_2g: fix(log[0xc6], 0),
        tx_bb_swing_5g: fix(log[0xc7], 0),
        country: [log[0xcb], log[0xcc]],
        power_2g,
        power_5g,
        share_ant: bt != 0xff && bt & 1 != 0,
    }
}

impl Efuse {
    /// ¿La MAC sirve? (No todo ceros, no todo unos, no de grupo.)
    pub fn mac_valid(&self) -> bool {
        self.mac != [0; 6] && self.mac != [0xff; 6] && self.mac[0] & 1 == 0
    }
}
