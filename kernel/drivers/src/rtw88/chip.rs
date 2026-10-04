//! La RTL8821CE entera: arrancarla, cambiar de canal, conectarse a una red y mandar tramas.
//!
//! El orden del arranque es el de `rtw_core_start` + `rtw_power_on` de Linux:
//!
//! 1. Datos del chip (versión, cantidad de caminos de radio).
//! 2. Encender ([`pwrseq`](super::pwrseq)) y leer el efuse.
//! 3. Cargar el firmware ([`fw`](super::fw)).
//! 4. La MAC: qué cola de la PCI va a qué prioridad, cuántas páginas de la memoria de TX tiene
//!    cada una, dónde viven los H2C, y los tiempos del protocolo (`rtw8821c_mac_init`).
//! 5. La banda base y la radio: las tablas y la calibración del cristal (`phy_set_param`).
//! 6. Arrancar las interrupciones, avisarle al firmware cómo es la placa y repartir la antena
//!    con el Bluetooth.
//!
//! **El Bluetooth.** La 8821CE es un combo Wi-Fi + Bluetooth con una antena compartida, y
//! Linux corre un algoritmo entero (coex.c) para repartirla. JARVIS-OS no tiene Bluetooth: la
//! antena queda siempre del Wi-Fi (la fase "solo WLAN" de coex.c).
//!
//! **La seguridad.** La placa sabe cifrar con CCMP, pero acá no se le dan las claves: las
//! tramas salen ya cifradas (jarvis-wifi) y llegan cifradas. Así el cifrado que corre es el que
//! se probó contra otra implementación.

use super::desc::{self, Rx};
use super::efuse::{self, Efuse};
use super::fw::{self, H2c};
use super::phy::{self, ChParams, Cond};
use super::{Bus, BusExt, Error, Queue, pwrseq, regs};

/// Páginas de 128 bytes de la memoria de TX (64 KiB).
const TXFF_PAGES: u16 = 512;
/// Páginas reservadas: las del driver (8) y las del firmware (info extra 24, estática 8, cola
/// H2C 8, buffer de TX 4).
const RSVD_DRV: u16 = 8;
const RSVD_PAGES: u16 = RSVD_DRV + 24 + 8 + 8 + 4;
const RXFF_SIZE: u32 = 16384;

/// La configuración del filtro de recepción (`hal.rcr` de Linux): agregar el CRC, el MIC y el
/// ICV, el estado de la PHY, y aceptar difusión, multicast y lo dirigido a nosotros.
const RCR_BASE: u32 = (1 << 31)
    | (1 << 30)
    | (1 << 29)
    | (1 << 20)
    | (1 << 14)
    | regs::RCR_APP_PHYSTS
    | regs::RCR_VHT_DACK
    | (1 << 3)
    | (1 << 2)
    | (1 << 1);

/// El mac_id de la estación en el firmware.
pub const MAC_ID: u8 = 0;

pub struct Rtw8821c {
    pub efuse: Efuse,
    pub mac: [u8; 6],
    /// Versión del chip (*cut*: A = 0, B = 1…).
    pub cut: u8,
    pub fw_version: (u16, u8, u8),
    h2c: H2c,
    ch: ChParams,
    cond: Cond,
    channel: u8,
    rate_id: u8,
    connected: bool,
}

/// Lo que se sabe de la red a la que se conectó (para elegir velocidades).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// Velocidades básicas que anuncia (bits de CCK y OFDM, `supp_rates` de Linux: 1, 2, 5,5,
    /// 11, 6, 9…54 Mb/s).
    pub legacy: u16,
    /// La red tiene 802.11n (HT) o 802.11ac (VHT).
    pub ht: bool,
    pub vht: bool,
}

impl Rtw8821c {
    /// Arranca la placa con el firmware `fw` (el archivo rtw8821c_fw.bin).
    pub fn start<B: Bus + ?Sized>(bus: &mut B, fw_file: &[u8]) -> Result<Rtw8821c, Error> {
        fw::parse(fw_file)?;
        let cfg1 = bus.read32(regs::SYS_CFG1);
        let cut = ((cfg1 >> 12) & 0xf) as u8;
        bus.hci_setup();
        pwrseq::power_on(bus)?;
        let phy_map = efuse::read_physical(bus)?;
        let log = efuse::logical_map(&phy_map)?;
        let efuse = efuse::parse(&log);
        let mut mac = efuse.mac;
        if !efuse.mac_valid() {
            // Sin MAC de fábrica: una local (bit 1) derivada del efuse, siempre la misma.
            let h = phy_map.iter().fold(0x811c_9dc5u32, |h, &b| {
                (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
            });
            mac = [0x02, 0x21, 0xc8, (h >> 16) as u8, (h >> 8) as u8, h as u8];
        }
        let header = fw::download(bus, fw_file)?;
        let mut chip = Rtw8821c {
            cond: Cond::new(&efuse, cut, u8::from(efuse.rfe_option & (1 << 5) != 0)),
            efuse,
            mac,
            cut,
            fw_version: (header.version, header.sub_version, header.sub_index),
            h2c: H2c::default(),
            ch: ChParams::default(),
            channel: 0,
            rate_id: 8,
            connected: false,
        };
        // La 8821CE por PCIe: que el firmware recupere el Bluetooth del combo.
        chip.h2c.command(bus, fw::recover_bt());
        chip.mac_init(bus)?;
        chip.phy_init(bus);
        // Arrancar: las interrupciones (las pide el kernel), lo que el firmware tiene que saber
        // y la antena para el Wi-Fi.
        let txbuf = (TXFF_PAGES - 4) - (TXFF_PAGES - RSVD_PAGES);
        chip.h2c.packet(bus, fw::general_info(txbuf as u8));
        chip.h2c
            .packet(bus, fw::phydm_info(chip.efuse.rfe_option, chip.cut));
        chip.coex_wifi_only(bus);
        bus.write32(regs::RCR, RCR_BASE | regs::RCR_CBSSID_BCN);
        bus.write32(0x0620, u32::MAX); // MAR: todo el multicast
        bus.write32(0x0624, u32::MAX);
        chip.port_init(bus);
        chip.set_channel(bus, 1);
        Ok(chip)
    }

    // --- MAC ------------------------------------------------------------------------------------

    fn mac_init<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<(), Error> {
        // Qué prioridad tiene cada cola en la PCI (rqpn_table_8821c[1]): VO y VI normal, BE y
        // BK baja, gestión extra, alta la alta.
        let (normal, low, extra, high) = (2u16, 1u16, 0u16, 3u16);
        let map = normal << 4 | normal << 6 | low << 8 | low << 10 | extra << 12 | high << 14;
        bus.write16(regs::TXDMA_PQ_MAP, map);
        bus.write8(regs::CR, 0);
        bus.write8(regs::CR, regs::MAC_TRX_ENABLE);
        bus.write32(regs::H2CQ_CSR, regs::H2CQ_FULL);
        // Páginas de cada cola (page_table_8821c[1]: alta 16, normal 16, baja 16, extra 14) y
        // la pública, lo que sobra.
        let boundary = TXFF_PAGES - RSVD_PAGES;
        let (hq, nq, lq, exq, gap) = (16u16, 16u16, 16u16, 14u16, 1u16);
        let pubq = boundary - hq - lq - nq - exq - gap;
        bus.write16(regs::FIFOPAGE_INFO[0], hq);
        bus.write16(regs::FIFOPAGE_INFO[1], lq);
        bus.write16(regs::FIFOPAGE_INFO[2], nq);
        bus.write16(regs::FIFOPAGE_INFO[3], exq);
        bus.write16(regs::FIFOPAGE_INFO[4], pubq);
        bus.set32(regs::RQPN_CTRL_2, regs::LD_RQPN);
        bus.write16(regs::FIFOPAGE_CTRL_2, boundary);
        bus.set8(regs::FWHW_TXQ_CTRL + 2, regs::EN_WR_FREE_TAIL);
        bus.write16(regs::BCNQ_BDNY_V1, boundary);
        bus.write16(regs::FIFOPAGE_CTRL_2 + 2, boundary);
        bus.write16(regs::BCNQ1_BDNY_V1, boundary);
        bus.write32(regs::RXFF_BNDY, RXFF_SIZE - 256 - 1);
        bus.set8(regs::AUTO_LLT_V1, 1);
        if !bus.wait32(regs::AUTO_LLT_V1, 1, 0) {
            return Err(Error::Mac("no se armó la lista de páginas (LLT)"));
        }
        bus.write8(regs::CR + 3, 0);
        // La cola de los H2C, en sus 8 páginas (`init_h2c`).
        let h2cq_addr = u32::from(TXFF_PAGES - 4 - 8) << 7;
        let h2cq_size = 8u32 << 7;
        let v = bus.read32(regs::H2C_HEAD) & 0xfffc_0000;
        bus.write32(regs::H2C_HEAD, v | h2cq_addr);
        let v = bus.read32(regs::H2C_READ_ADDR) & 0xfffc_0000;
        bus.write32(regs::H2C_READ_ADDR, v | h2cq_addr);
        let v = bus.read32(regs::H2C_TAIL) & 0xfffc_0000;
        bus.write32(regs::H2C_TAIL, v | (h2cq_addr + h2cq_size));
        let v = bus.read8(regs::H2C_INFO);
        bus.write8(regs::H2C_INFO, (v & 0xfc) | 0x01);
        let v = bus.read8(regs::H2C_INFO);
        bus.write8(regs::H2C_INFO, (v & 0xfb) | 0x04);
        let v = bus.read8(regs::TXDMA_OFFSET_CHK + 1);
        bus.write8(regs::TXDMA_OFFSET_CHK + 1, (v & 0x7f) | 0x80);
        let wp = bus.read32(regs::H2C_PKT_WRITEADDR) & 0x3ffff;
        let rp = bus.read32(regs::H2C_PKT_READADDR) & 0x3ffff;
        let free = if wp >= rp {
            h2cq_size - (wp - rp)
        } else {
            rp - wp
        };
        if free != h2cq_size {
            return Err(Error::Mac("la cola H2C no está vacía"));
        }
        // Protocolo (`rtw8821c_mac_init`): agregación, RTS, EDCA, beacons, filtros de RX.
        bus.write8(regs::AMPDU_MAX_TIME_V1, 0x70);
        bus.set8(regs::TX_HANG_CTRL, 1 << 2);
        let pre: u16 = 0x1e4 | (1 << 11);
        bus.write8(regs::PRECNT_CTRL, pre as u8);
        bus.write8(regs::PRECNT_CTRL + 1, (pre >> 8) as u8);
        bus.write32(
            regs::PROT_MODE_CTRL,
            0xff | (0x08 << 8) | (0x20 << 16) | (0x20 << 24),
        );
        bus.write16(regs::BAR_MODE_CTRL + 2, 0x01 | (0x08 << 8));
        bus.write8(regs::FAST_EDCA_VOVI, 6);
        bus.write8(regs::FAST_EDCA_VOVI + 2, 6);
        bus.write8(regs::FAST_EDCA_BEBK, 6);
        bus.write8(regs::FAST_EDCA_BEBK + 2, 6);
        bus.set8(regs::INIRTS_RATE_SEL, 1 << 5);
        bus.clr8(regs::TIMER0_SRC_SEL, (1 << 4) | (1 << 5) | (1 << 6));
        bus.write16(regs::TXPAUSE, 0);
        bus.write8(regs::SLOT, 0x09);
        bus.write8(regs::PIFS, 0x19);
        bus.write32(regs::SIFS, 0x0a | (0x0e << 8) | (0x10 << 16) | (0x10 << 24));
        bus.write16(regs::EDCA_VO_PARAM + 2, 0x186);
        bus.write16(regs::EDCA_VI_PARAM + 2, 0x3bc);
        bus.write32(regs::RD_NAV_NXT, 0x05 | (0x1b << 16));
        bus.write16(regs::RXTSF_OFFSET_CCK, 0x30 | (0x30 << 8));
        bus.set8(regs::BCN_CTRL, regs::EN_BCN_FUNCTION);
        bus.write32(regs::TBTT_PROHIBIT, 0x04 | (0x064 << 8));
        bus.write8(regs::DRVERLYINT, 0x04);
        bus.write8(regs::BCNDMATIM, 0x02);
        bus.clr8(regs::TX_PTCL_CTRL + 1, 1 << 4);
        bus.write16(regs::RXFLTMAP0, 0xffff);
        bus.write16(regs::RXFLTMAP1, 0x0fff);
        bus.write16(regs::RXFLTMAP2, 0xffff);
        bus.write32(regs::RCR, regs::RCR_INIT);
        bus.write8(regs::RX_PKT_LIMIT, (12288 >> 9) as u8);
        bus.write8(regs::TCR + 2, 0x30);
        bus.write8(regs::TCR + 1, 0x30);
        bus.write8(regs::ACKTO_CCK, 0x40);
        bus.set8(regs::WMAC_TRXPTCL_CTL_H, 1 << 1);
        bus.set8(regs::SND_PTCL_CTRL, 1 << 6);
        bus.write32(regs::WMAC_OPTION_FUNCTION + 8, 0xb081_0041);
        bus.write8(regs::WMAC_OPTION_FUNCTION + 4, 0x98);
        // Que cada trama recibida traiga el estado de la PHY (4 × 8 bytes).
        bus.write8(regs::RX_DRVINFO_SZ, 4);
        let v = (bus.read8(regs::TRXFF_BNDY + 1) & 0xf0) | 0x0f;
        bus.write8(regs::TRXFF_BNDY + 1, v);
        bus.set32(regs::RCR, regs::RCR_APP_PHYSTS);
        bus.clr32(regs::WMAC_OPTION_FUNCTION + 4, (1 << 8) | (1 << 9));
        Ok(())
    }

    // --- banda base y radio ----------------------------------------------------------------------

    fn phy_init<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        let mut v = bus.read8(regs::SYS_FUNC_EN) | regs::FEN_PCIEA;
        bus.write8(regs::SYS_FUNC_EN, v);
        // Reiniciar la banda base: 1, 0, 1.
        v |= regs::FEN_BB_RSTB | regs::FEN_BB_GLB_RST;
        bus.write8(regs::SYS_FUNC_EN, v);
        v &= !(regs::FEN_BB_RSTB | regs::FEN_BB_GLB_RST);
        bus.write8(regs::SYS_FUNC_EN, v);
        v |= regs::FEN_BB_RSTB | regs::FEN_BB_GLB_RST;
        bus.write8(regs::SYS_FUNC_EN, v);
        let rf = regs::RF_EN | regs::RF_RSTB | regs::RF_SDM_RSTB;
        bus.write8(regs::RF_CTRL, rf);
        bus.delay_us(10);
        bus.write8(regs::WLRF1 + 3, rf);
        bus.delay_us(10);
        bus.clr32(regs::RXPSEL, regs::RX_PSEL_RST);
        phy::load_tables(bus, &self.cond, phy::rfe_btg(self.efuse.rfe_option));
        let xtal = u32::from(self.efuse.crystal_cap & 0x3f);
        bus.write32_mask(regs::AFE_XTAL_CTRL, 0x7e00_0000, xtal);
        bus.write32_mask(regs::AFE_PLL_CTRL, 0x7e, xtal);
        bus.write32_mask(regs::CCK0_FAREPORT, (1 << 18) | (1 << 22), 0);
        bus.set32(regs::RXPSEL, regs::RX_PSEL_RST);
        self.ch = ChParams([
            bus.read32(regs::TXSF2),
            bus.read32(regs::TXSF6),
            bus.read32(regs::TXFILTER),
        ]);
        bus.write32(0x1c94, 0xafff_afff);
    }

    /// Cambia de canal (2,4 GHz: 1–14; 5 GHz: 36–165).
    pub fn set_channel<B: Bus + ?Sized>(&mut self, bus: &mut B, channel: u8) {
        if channel == self.channel {
            return;
        }
        let band_changed = (channel > 14) != (self.channel > 14) || self.channel == 0;
        phy::set_channel(bus, channel, &self.efuse, &self.ch);
        self.channel = channel;
        if band_changed {
            self.antenna(bus, channel > 14);
        }
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// Calibra la radio (el firmware hace el IQK). Linux la corre antes de conectarse a una red
    /// (`mgd_prepare_tx`); tarda hasta unos segundos.
    pub fn calibrate<B: Bus + ?Sized>(&mut self, bus: &mut B) -> bool {
        if !self.h2c.packet(bus, fw::iqk(self.connected)) {
            return false;
        }
        let mut done = false;
        for _ in 0..300 {
            if phy::read_rf(bus, regs::RF_DTXLOK, regs::RF_MASK) == 0xabcde {
                done = true;
                break;
            }
            bus.delay_us(20_000);
        }
        phy::write_rf(bus, regs::RF_DTXLOK, regs::RF_MASK, 0);
        done
    }

    // --- antena (Bluetooth fuera) -----------------------------------------------------------------

    /// Escribe un campo del coexistidor (registro indirecto 0x38).
    fn lte_field<B: Bus + ?Sized>(&self, bus: &mut B, mask: u32, value: u32) {
        let old = fw::ltecoex_read(bus, 0x38).unwrap_or(0);
        let v = (old & !mask) | ((value << mask.trailing_zeros()) & mask);
        fw::ltecoex_write(bus, 0x38, v);
    }

    /// La antena siempre para el Wi-Fi: GNT_BT bajo y GNT_WL alto por software, el camino de la
    /// antena en manos del Wi-Fi (`COEX_SET_ANT_WONLY`).
    fn coex_wifi_only<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        bus.set8(regs::SYS_FUNC_EN, regs::FEN_BB_GLB_RST | regs::FEN_BB_RSTB);
        bus.write8(0xff1a, 0);
        bus.clr32(
            regs::PAD_CTRL1,
            regs::BTGP_SPI_EN | regs::BTGP_JTAG_EN | regs::LED1DIS,
        );
        bus.clr32(regs::GPIO_MUXCFG, regs::FSPI_EN);
        bus.clr32(regs::SYS_SDIO_CTRL, regs::SDIO_INT | regs::DBG_GNT_WL_BT);
        bus.set8(regs::BCN_CTRL, regs::EN_BCN_FUNCTION);
        // Las respuestas (ACK, CTS) y los beacons, con prioridad.
        bus.set8(regs::BT_COEX_TABLE_H, (1 << 3) | (1 << 4));
        bus.set8(regs::BT_COEX_TABLE_H + 3, 1 << 3);
        self.lte_field(bus, 0xc000, 1); // GNT_BT: bajo por software
        self.lte_field(bus, 0x0c00, 1);
        self.lte_field(bus, 0x3000, 3); // GNT_WL: alto por software
        self.lte_field(bus, 0x0300, 3);
        bus.set8(regs::SYS_SDIO_CTRL + 3, regs::LTE_MUX_CTRL_PATH);
        // Pizarra compartida con el Bluetooth: "Wi-Fi activo y encendido".
        bus.write16(regs::WIFI_BT_INFO, 0x2 | 0x1 | (1 << 15));
        bus.write32(regs::BT_COEX_TABLE0, 0x5555_5555);
        bus.write32(regs::BT_COEX_TABLE1, 0x5555_5555);
        bus.write32(regs::BT_COEX_BRK_TABLE, 0xf0ff_ffff);
    }

    /// El conmutador de la antena según la banda (`rtw8821c_coex_cfg_ant_switch`, controlado por
    /// la banda base).
    fn antenna<B: Bus + ?Sized>(&mut self, bus: &mut B, five_ghz: bool) {
        let rfe = self.efuse.rfe_option;
        if matches!(rfe, 5 | 13 | 6 | 14) {
            return; // dos antenas, sin conmutador
        }
        let inverse = matches!(rfe, 3 | 11 | 4 | 12);
        let wlg_at_btg = matches!(rfe, 2 | 10 | 7 | 15 | 4 | 12);
        let value = if wlg_at_btg {
            // El Wi-Fi de 2,4 GHz pasa por la rama del Bluetooth: el conmutador queda fijo (en
            // "A" si la polaridad está invertida, si no en "G + Bluetooth").
            if inverse || rfe == 2 { 2 } else { 3 }
        } else if five_ghz == inverse {
            2
        } else {
            1
        };
        bus.clr32(regs::LED_CFG, regs::DPDT_SEL_EN);
        bus.set32(regs::LED_CFG, regs::DPDT_WL_SEL);
        bus.write8_mask(regs::RFE_CTRL8, 0xff, 0x77);
        bus.write32_mask(regs::RFE_CTRL8, 0xf000_0000, value);
        bus.set8(regs::CTRL_TYPE, (1 << 5) | (1 << 4));
    }

    // --- la estación ------------------------------------------------------------------------------

    fn write_addr<B: Bus + ?Sized>(bus: &mut B, at: u32, addr: &[u8; 6]) {
        for (i, b) in addr.iter().enumerate() {
            bus.write8(at + i as u32, *b);
        }
    }

    /// El puerto 0 como estación sin conectar.
    fn port_init<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        Self::write_addr(bus, regs::MACID, &self.mac);
        bus.write32_mask(regs::CR, 0x30000, 0);
        bus.write8(regs::BCN_CTRL, regs::EN_BCN_FUNCTION);
    }

    /// Buscando redes: aceptar los beacons de todas (`FIF_BCN_PRBRESP_PROMISC`).
    pub fn set_scanning<B: Bus + ?Sized>(&mut self, bus: &mut B, on: bool) {
        let rcr = if on {
            RCR_BASE
        } else {
            RCR_BASE | regs::RCR_CBSSID_BCN
        };
        bus.write32(regs::RCR, rcr);
    }

    /// Anota el punto de acceso (antes de autenticarse): la placa filtra por su BSSID.
    pub fn set_bssid<B: Bus + ?Sized>(&mut self, bus: &mut B, bssid: &[u8; 6]) {
        Self::write_addr(bus, regs::BSSID, bssid);
    }

    /// Asociada (`aid` del punto de acceso) o no. Al asociarse le dice al firmware que la
    /// estación existe y entre qué velocidades elegir.
    pub fn set_associated<B: Bus + ?Sized>(&mut self, bus: &mut B, aid: Option<u16>, link: &Link) {
        match aid {
            Some(aid) => {
                bus.write32_mask(regs::CR, 0x30000, 2); // RTW_NET_MGD_LINKED
                bus.write32_mask(regs::AID, 0x7ff, u32::from(aid));
                self.h2c.command(bus, fw::media_status(MAC_ID, true));
                let (rate_id, mask) = rate_mask(link, self.channel > 14);
                self.rate_id = rate_id;
                self.h2c
                    .command(bus, fw::ra_info(MAC_ID, rate_id, mask, link.vht, false));
                self.connected = true;
            }
            None => {
                if self.connected {
                    self.h2c.command(bus, fw::media_status(MAC_ID, false));
                }
                bus.write32_mask(regs::CR, 0x30000, 0);
                bus.write32_mask(regs::AID, 0x7ff, 0);
                Self::write_addr(bus, regs::BSSID, &[0; 6]);
                self.connected = false;
            }
        }
    }

    // --- tramas -----------------------------------------------------------------------------------

    /// Manda una trama 802.11 (gestión o datos, ya cifrada si hace falta).
    pub fn send<B: Bus + ?Sized>(&mut self, bus: &mut B, frame: &[u8]) -> bool {
        let five = self.channel > 14;
        let kind = (frame.first().copied().unwrap_or(0) >> 2) & 3;
        let (queue, d) = if kind == 0 {
            (Queue::Mgmt, desc::mgmt_desc(frame, five))
        } else {
            let rate_id = if self.connected {
                self.rate_id
            } else if five {
                7
            } else {
                8
            };
            (
                Queue::Be,
                desc::data_desc(frame, rate_id, self.connected, five),
            )
        };
        let mut packet = alloc::vec::Vec::with_capacity(desc::TX_DESC_SIZE + frame.len());
        packet.extend_from_slice(&d);
        packet.extend_from_slice(frame);
        bus.tx(queue, &packet)
    }

    /// Interpreta un buffer de recepción: la trama (sin CRC) y la señal.
    pub fn receive<'a>(&self, buf: &'a [u8]) -> Option<(&'a [u8], Option<i8>)> {
        let rx: Rx = desc::parse_rx(buf, self.efuse.rfe_option)?;
        if rx.c2h {
            return None;
        }
        Some((&buf[rx.offset..rx.offset + rx.len], rx.rssi))
    }

    /// Apaga la placa (al apagar la PC o desconectar el Wi-Fi).
    pub fn stop<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        bus.write32(regs::PCI_HIMR0, 0);
        bus.write32(regs::PCI_HIMR1, 0);
        bus.write32(regs::PCI_HIMR3, 0);
        let _ = pwrseq::run(bus, pwrseq::POWER_OFF);
    }
}

/// Qué velocidades puede usar el firmware con esta red y con qué tabla (`rtw_update_sta_info`,
/// para un flujo espacial y 20 MHz). Bits de la máscara: 0–3 CCK, 4–11 OFDM, 12–19 HT MCS0–7,
/// 12–21 VHT 1SS MCS0–9.
pub fn rate_mask(link: &Link, five_ghz: bool) -> (u8, u32) {
    let legacy = if five_ghz {
        u32::from(link.legacy & 0xff0)
    } else {
        u32::from(link.legacy & 0xfff)
    };
    if link.vht {
        let mask = 0x3ff000 | legacy & 0x10 | if five_ghz { 0 } else { legacy & 0x5 };
        // ARFR1_AC_1SS (5 GHz) o ARFR2_AC_2G_1SS
        (if five_ghz { 10 } else { 11 }, mask)
    } else if link.ht {
        if five_ghz {
            (5, 0xff000 | legacy & 0x30) // GN_N1SS
        } else {
            (3, 0xff000 | legacy & 0x10 | legacy & 0x5) // BGN_20M_1SS
        }
    } else if five_ghz {
        (7, legacy) // G
    } else if legacy <= 0xf {
        (8, legacy) // B
    } else {
        (6, legacy & 0xff5) // BG
    }
}
