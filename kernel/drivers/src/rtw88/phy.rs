//! La banda base (BB) y la radio (RF): las tablas de Realtek, el cambio de canal y la potencia.
//!
//! **Tablas con condiciones.** Cada tabla es una lista de pares (dirección, valor). Algunos pares
//! no son escrituras sino condiciones: `if`/`elif` con un tipo de placa, `else` y `endif`. Así
//! una sola tabla sirve para todas las variantes del chip (encapsulado, tipo de antena). El bit
//! 31 de la dirección marca "condición positiva" (el `if` y su encabezado) y el 30 la
//! "negativa" (la que cierra la condición y decide si se cumple).
//!
//! **La radio** se escribe por un bus serie (SIPI): una escritura de 32 bits a `0xC90` con la
//! dirección de 8 bits y el dato de 20. Se lee directo en `0x2800 + dirección × 4`.

use super::{Bus, BusExt, regs};
use crate::rtw88::efuse::Efuse;

/// Lo que eligen las condiciones de las tablas (`struct rtw_phy_cond`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cond {
    pub rfe: u8,
    /// Interfaz: PCIe = 1.
    pub intf: u8,
    pub pkg: u8,
    pub cut: u8,
}

impl Cond {
    pub fn new(efuse: &Efuse, cut: u8, pkg: u8) -> Cond {
        Cond {
            rfe: efuse.rfe_option,
            intf: 1,
            pkg: if pkg == 0 { 15 } else { pkg },
            cut: if cut == 0 { 15 } else { cut },
        }
    }

    /// ¿Se cumple la condición del encabezado `word`? (Un campo en 0 no se mira.)
    fn matches(&self, word: u32) -> bool {
        let rfe = word as u8;
        let intf = ((word >> 8) & 0xf) as u8;
        let pkg = ((word >> 12) & 0xf) as u8;
        let cut = ((word >> 24) & 0xf) as u8;
        (cut == 0 || cut == self.cut)
            && (pkg == 0 || pkg == self.pkg)
            && (intf == 0 || intf == self.intf)
            && rfe == self.rfe
    }
}

/// Recorre una tabla y llama a `apply(dirección, valor)` con los pares que tocan a esta placa
/// (`rtw_parse_tbl_phy_cond`).
pub fn load_table(table: &[u32], cond: &Cond, mut apply: impl FnMut(u32, u32)) {
    let mut pos_cond = 0u32;
    let (mut matched, mut skipped) = (true, false);
    for &[addr, data] in table.as_chunks::<2>().0 {
        if addr & (1 << 31) != 0 {
            match (addr >> 28) & 3 {
                3 => {
                    // endif
                    matched = true;
                    skipped = false;
                }
                2 => matched = !skipped, // else
                _ => pos_cond = addr,    // if / elif: la condición
            }
        } else if addr & (1 << 30) != 0 {
            if skipped {
                matched = false;
            } else if cond.matches(pos_cond) {
                matched = true;
                skipped = true;
            } else {
                matched = false;
            }
        } else if matched {
            apply(addr, data);
        }
    }
}

/// Lee un registro de la radio (camino A).
pub fn read_rf<B: Bus + ?Sized>(bus: &mut B, addr: u32, mask: u32) -> u32 {
    bus.read32_mask(regs::RF_BASE_A + ((addr & 0xff) << 2), mask & regs::RF_MASK)
}

/// Escribe un registro de la radio por SIPI (`rtw_phy_write_rf_reg_sipi`).
pub fn write_rf<B: Bus + ?Sized>(bus: &mut B, addr: u32, mask: u32, data: u32) {
    let addr = addr & 0xff;
    let mask = mask & regs::RF_MASK;
    let data = if mask == regs::RF_MASK {
        data
    } else {
        let old = read_rf(bus, addr, regs::RF_MASK);
        (old & !mask) | ((data << mask.trailing_zeros()) & mask)
    };
    bus.write32(
        regs::RF_SIPI_A,
        ((addr << 20) | (data & 0xfffff)) & 0x0fff_ffff,
    );
    bus.delay_us(13);
}

/// Aplica las tablas de MAC, BB, AGC y radio (`rtw_phy_load_tables`). Las direcciones 0xF9..0xFE
/// de la BB (y 0xFE, 0xFFE de la radio) son esperas, no registros.
pub fn load_tables<B: Bus + ?Sized>(bus: &mut B, cond: &Cond, btg: bool) {
    use super::tablas;
    load_table(&tablas::MAC, cond, |a, d| bus.write8(a, d as u8));
    load_table(&tablas::BB, cond, |a, d| match a {
        0xfe => bus.delay_us(50_000),
        0xfd => bus.delay_us(5_000),
        0xfc => bus.delay_us(1_000),
        0xfb => bus.delay_us(50),
        0xfa => bus.delay_us(5),
        0xf9 => bus.delay_us(1),
        _ => bus.write32(a, d),
    });
    load_table(&tablas::AGC, cond, |a, d| bus.write32(a, d));
    if btg {
        load_table(&tablas::AGC_BTG, cond, |a, d| bus.write32(a, d));
    }
    load_table(&tablas::RF_A, cond, |a, d| match a {
        0xffe => bus.delay_us(50_000),
        0xfe => bus.delay_us(100),
        _ => {
            write_rf(bus, a, regs::RF_MASK, d);
            bus.delay_us(1);
        }
    });
}

/// Las placas cuya antena de 2,4 GHz pasa por la del Bluetooth (BTG).
pub fn rfe_btg(rfe: u8) -> bool {
    matches!(rfe, 0x2 | 0x4 | 0x7 | 0xa | 0xc | 0xf)
}

/// Por dónde va la antena (`rtw8821c_switch_rf_set`).
fn switch_rf<B: Bus + ?Sized>(bus: &mut B, five_ghz: bool, btg: bool) {
    bus.set32(regs::DMEM_CTRL, regs::WL_RST);
    bus.set32(regs::SYS_CTRL, regs::FEN_EN);
    let mut reg = bus.read32(regs::RFECTL);
    let all = regs::BTG_SWITCH
        | regs::CTRL_SWITCH
        | regs::WL_SWITCH
        | regs::WLG_SWITCH
        | regs::WLA_SWITCH;
    reg &= !all;
    if five_ghz {
        reg |= regs::WL_SWITCH | regs::WLA_SWITCH;
    } else if btg {
        reg |= regs::BTG_SWITCH;
        bus.write32_mask(regs::ENRXCCA, 0x00ff_0000, 0x0e);
        bus.write32_mask(regs::ENTXCCK, 0x0000_ffff, 0xfc84);
    } else {
        reg |= regs::WL_SWITCH | regs::WLG_SWITCH;
        bus.write32_mask(regs::ENRXCCA, 0x00ff_0000, 0x12);
        bus.write32_mask(regs::ENTXCCK, 0x0000_ffff, 0x7532);
    }
    bus.write32(regs::RFECTL, reg);
}

/// El valor del registro 0x18 de la radio para un canal de 20 MHz.
pub fn rf18(old: u32, channel: u8) -> u32 {
    let band_mask = (1 << 16) | (1 << 9) | (1 << 8);
    let rfsi_mask = (1 << 18) | (1 << 17);
    let bw_mask = (1 << 11) | (1 << 10);
    let mut v = old & !(band_mask | 0xff | rfsi_mask | bw_mask);
    if channel > 14 {
        v |= (1 << 16) | (1 << 8);
    }
    v |= u32::from(channel);
    if (100..=140).contains(&channel) {
        v |= 1 << 17;
    } else if channel > 140 {
        v |= 1 << 18;
    }
    v | bw_mask // 20 MHz
}

/// Los tres valores de los filtros de CCK que deja la tabla de la BB (se guardan después de
/// cargarla, para volver a ellos al dejar el canal 14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChParams(pub [u32; 3]);

/// Cambia de canal, en 20 MHz (`rtw8821c_set_channel`: BB, potencia de la BB, MAC, radio y
/// filtro de recepción).
pub fn set_channel<B: Bus + ?Sized>(bus: &mut B, channel: u8, efuse: &Efuse, ch: &ChParams) {
    // --- banda base
    if channel <= 14 {
        bus.write32_mask(regs::RXPSEL, 1 << 28, 1);
        bus.write32_mask(regs::CCK_CHECK, 1 << 7, 0);
        bus.write32_mask(regs::ENTXCCK, 1 << 18, 0);
        bus.write32_mask(regs::RXCCAMSK, 0x0000_fc00, 15);
        bus.write32_mask(regs::TXSCALE_A, 0xf00, 0);
        bus.write32_mask(regs::CLKTRK, 0x1ffe_0000, 0x96a);
        if channel == 14 {
            bus.write32(regs::TXSF2, 0x0000_b81c);
            bus.write32_mask(regs::TXSF6, 0xffff, 0);
            bus.write32(regs::TXFILTER, 0x0000_3667);
        } else {
            bus.write32(regs::TXSF2, ch.0[0]);
            bus.write32_mask(regs::TXSF6, 0xffff, ch.0[1] & 0xffff);
            bus.write32(regs::TXFILTER, ch.0[2]);
        }
    } else {
        bus.write32_mask(regs::ENTXCCK, 1 << 18, 1);
        bus.write32_mask(regs::CCK_CHECK, 1 << 7, 1);
        bus.write32_mask(regs::RXPSEL, 1 << 28, 0);
        bus.write32_mask(regs::RXCCAMSK, 0x0000_fc00, 15);
        let scale = match channel {
            36..=64 => 1,
            100..=144 => 2,
            _ => 3,
        };
        bus.write32_mask(regs::TXSCALE_A, 0xf00, scale);
        let clk = match channel {
            36..=48 => 0x494,
            52..=64 => 0x453,
            100..=116 => 0x452,
            _ => 0x412,
        };
        bus.write32_mask(regs::CLKTRK, 0x1ffe_0000, clk);
    }
    let adc = (bus.read32(regs::ADCCLK) & 0xffcf_fc00) | 0x1001_0000;
    bus.write32(regs::ADCCLK, adc);
    bus.write32_mask(regs::ADC160, 1 << 30, 1);
    // --- potencia de la banda base (swing) según el efuse
    let swing = if channel <= 14 {
        efuse.tx_bb_swing_2g
    } else {
        efuse.tx_bb_swing_5g
    };
    let swing = if swing > 9 { 0 } else { swing };
    let table = [0x200, 0x16a, 0x101, 0x0b6];
    bus.write32_mask(regs::TXSCALE_A, 0xffe0_0000, table[usize::from(swing / 3)]);
    // --- MAC (`rtw_set_channel_mac`): 20 MHz, reloj de 80 MHz
    bus.write8(regs::DATA_SC, 0);
    let v = bus.read32(regs::WMAC_TRXPTCL_CTL) & !((1 << 7) | (1 << 8));
    bus.write32(regs::WMAC_TRXPTCL_CTL, v);
    let v = bus.read32(regs::AFE_CTRL1) & !((1 << 20) | (1 << 21));
    bus.write32(regs::AFE_CTRL1, v);
    bus.write8(regs::USTIME_TSF, 80);
    bus.write8(regs::USTIME_EDCA, 80);
    let v = bus.read8(regs::CCK_CHECK) & !(1 << 7);
    bus.write8(regs::CCK_CHECK, if channel > 14 { v | (1 << 7) } else { v });
    // --- radio
    let old = read_rf(bus, regs::RF_CHANNEL, regs::RF_MASK);
    let btg = rfe_btg(efuse.rfe_option);
    switch_rf(bus, channel > 14, btg);
    if channel <= 14 {
        write_rf(bus, regs::RF_LUTDBG, 1 << 6, 1);
        write_rf(bus, 0x64, 0xf, 0xf);
    } else {
        write_rf(bus, regs::RF_LUTDBG, 1 << 6, 0);
    }
    write_rf(bus, regs::RF_CHANNEL, regs::RF_MASK, rf18(old, channel));
    write_rf(bus, regs::RF_XTALX2, 1 << 19, 0);
    write_rf(bus, regs::RF_XTALX2, 1 << 19, 1);
    // --- filtro de recepción para 20 MHz
    bus.write32_mask(regs::ACBB0, (1 << 29) | (1 << 28), 2);
    bus.write32_mask(regs::ACBBRXFIR, (1 << 29) | (1 << 28), 2);
    bus.write32_mask(regs::TXDFIR, 1 << 31, 1);
    bus.write32_mask(regs::CHFIR, 1 << 31, 0);
    // --- potencia por velocidad
    set_tx_power(bus, channel, efuse);
}

/// El grupo de canales de la calibración (`rtw_get_channel_group`).
pub fn channel_group(channel: u8, cck: bool) -> usize {
    match channel {
        1 | 2 | 36..=42 => 0,
        3..=5 | 44..=50 => 1,
        6..=8 | 52..=58 => 2,
        9..=11 | 60..=64 => 3,
        12 | 13 | 100..=106 => 4,
        14 => {
            if cck {
                5
            } else {
                4
            }
        }
        108..=114 => 5,
        116..=122 => 6,
        124..=130 => 7,
        132..=138 => 8,
        140..=144 => 9,
        149..=155 => 10,
        157..=161 => 11,
        165..=171 => 12,
        _ => 13,
    }
}

/// El índice de potencia (0..=63) de una velocidad en un canal, de la calibración de fábrica:
/// la base del grupo de canales más la diferencia de OFDM o de 20 MHz. (Linux además aplica
/// las diferencias por velocidad y los límites por país; acá se usa la base, que es lo que
/// calibró la fábrica para la placa, sin pasarse del máximo.)
pub fn power_index(efuse: &Efuse, channel: u8, rate: u8) -> u8 {
    let cck = rate <= 3;
    let ofdm = (4..=11).contains(&rate);
    let group = channel_group(channel, cck);
    let v: i16 = if channel <= 14 {
        let p = &efuse.power_2g;
        if cck {
            i16::from(p.cck_base[group.min(5)])
        } else {
            let base = i16::from(p.bw40_base[group.min(4)]);
            base + if ofdm {
                i16::from(p.ofdm_diff)
            } else {
                i16::from(p.bw20_diff)
            }
        }
    } else {
        let p = &efuse.power_5g;
        let base = i16::from(p.bw40_base[group.min(13)]);
        base + if ofdm {
            i16::from(p.ofdm_diff)
        } else {
            i16::from(p.bw20_diff)
        }
    };
    // Sin calibrar (0xFF): un valor medio, el que usa Realtek por defecto.
    if (0..=0x3f).contains(&v) {
        v as u8
    } else {
        0x20
    }
}

/// Las velocidades de un flujo espacial: CCK (0–3), OFDM (4–11), HT MCS0–7 (12–19) y VHT
/// 1SS MCS0–9 (0x2C–0x35).
const RATES: [(u8, u8); 4] = [(0x00, 0x03), (0x04, 0x0b), (0x0c, 0x13), (0x2c, 0x35)];

/// Escribe la potencia de cada velocidad: un byte por velocidad, de a cuatro por registro
/// (`rtw8821c_set_tx_power_index`).
pub fn set_tx_power<B: Bus + ?Sized>(bus: &mut B, channel: u8, efuse: &Efuse) {
    for (first, last) in RATES {
        let mut word = 0u32;
        for rate in first..=last {
            if channel > 14 && rate <= 3 {
                continue; // no hay CCK en 5 GHz
            }
            let shift = u32::from(rate & 3) * 8;
            word |= u32::from(power_index(efuse, channel, rate)) << shift;
            if rate & 3 == 3 || rate == last {
                bus.write32(regs::TXAGC_A + u32::from(rate & 0xfc), word);
                word = 0;
            }
        }
    }
}
