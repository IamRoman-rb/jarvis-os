//! Direcciones y bits de los registros de la RTL8821CE que usa el driver (de `reg.h`,
//! `rtw8821c.h`, `pci.h` y `bf.h` del driver rtw88 de Linux).

// --- sistema -----------------------------------------------------------------------------------
pub const SYS_FUNC_EN: u32 = 0x0002;
pub const FEN_BB_GLB_RST: u8 = 1 << 1;
pub const FEN_BB_RSTB: u8 = 1 << 0;
pub const FEN_PCIEA: u8 = 1 << 6;
/// En el byte `SYS_FUNC_EN + 1`.
pub const FEN_CPUEN: u8 = 1 << 2;
pub const SYS_PW_CTRL: u32 = 0x0004;
pub const PFM_WOWL: u8 = 1 << 3;
pub const SYS_CLK_CTRL: u32 = 0x0008;
pub const CPU_CLK_EN: u32 = 1 << 14;
pub const RSV_CTRL: u32 = 0x001c;
pub const WLMCU_IOIF: u8 = 1 << 0;
pub const RF_CTRL: u32 = 0x001f;
pub const RF_EN: u8 = 1 << 0;
pub const RF_RSTB: u8 = 1 << 1;
pub const RF_SDM_RSTB: u8 = 1 << 2;
pub const AFE_CTRL1: u32 = 0x0024;
pub const AFE_XTAL_CTRL: u32 = 0x0024;
pub const AFE_PLL_CTRL: u32 = 0x0028;
pub const EFUSE_CTRL: u32 = 0x0030;
pub const EF_FLAG: u32 = 1 << 31;
pub const LDO_EFUSE_CTRL: u32 = 0x0034;
pub const GPIO_MUXCFG: u32 = 0x0040;
pub const WLRFE_4_5_EN: u32 = 1 << 2;
pub const FSPI_EN: u32 = 1 << 19;
pub const BT_PTA_EN: u32 = 1 << 5;
pub const LED_CFG: u32 = 0x004c;
pub const PAPE_SEL_EN: u32 = 1 << 25;
pub const LNAON_SEL_EN: u32 = 1 << 26;
pub const DPDT_SEL_EN: u32 = 1 << 23;
pub const DPDT_WL_SEL: u32 = 1 << 24;
pub const PAD_CTRL1: u32 = 0x0064;
pub const PAPE_WLBT_SEL: u32 = 1 << 29;
pub const LNAON_WLBT_SEL: u32 = 1 << 28;
pub const BTGP_JTAG_EN: u32 = 1 << 24;
pub const BTGP_SPI_EN: u32 = 1 << 20;
pub const LED1DIS: u32 = 1 << 15;
pub const CTRL_TYPE: u32 = 0x0067;
pub const SYS_SDIO_CTRL: u32 = 0x0070;
pub const LTE_MUX_CTRL_PATH: u8 = 1 << 2; // bit 26, en el byte +3
pub const DBG_GNT_WL_BT: u32 = 1 << 27;
pub const SDIO_INT: u32 = 1 << 18;
pub const HCI_OPT_CTRL: u32 = 0x0074;
pub const USB_SUS_DIS: u32 = 1 << 8;
pub const MCUFW_CTRL: u32 = 0x0080;
pub const MCUFWDL_EN: u16 = 1 << 0;
pub const IMEM_DW_OK: u8 = 1 << 3;
pub const IMEM_CHKSUM_OK: u8 = 1 << 4;
pub const DMEM_DW_OK: u8 = 1 << 5;
pub const DMEM_CHKSUM_OK: u8 = 1 << 6;
pub const CHECK_SUM_OK: u16 = (1 << 4) | (1 << 6);
pub const FW_DW_RDY: u16 = 1 << 14;
pub const FW_INIT_RDY: u16 = 1 << 15;
pub const BOOT_FSPI_EN: u32 = 1 << 20;
/// Firmware listo: todo bajado, verificado e iniciado.
pub const FW_READY: u32 =
    (FW_INIT_RDY | FW_DW_RDY) as u32 | (IMEM_DW_OK | DMEM_DW_OK) as u32 | CHECK_SUM_OK as u32;
pub const FW_READY_MASK: u32 = 0xffff & !((1 << 12) | (1 << 13));
pub const WIFI_BT_INFO: u32 = 0x00aa;
pub const SYS_CFG1: u32 = 0x00f0;
pub const RF_TYPE_ID: u32 = 1 << 27;
pub const WLRF1: u32 = 0x00ec;
pub const WLRF1_BBRF_EN: u32 = (1 << 24) | (1 << 25) | (1 << 26);

// --- MAC ---------------------------------------------------------------------------------------
pub const CR: u32 = 0x0100;
/// Todo encendido: DMA de la PCIe y de la MAC, protocolo, planificador, TX y RX.
pub const MAC_TRX_ENABLE: u8 = 0xff;
pub const HCI_TXDMA_EN: u8 = 1 << 0;
pub const TXDMA_EN: u8 = 1 << 2;
/// En el byte `CR + 1`.
pub const ENSWBCN: u8 = 1 << 0;
pub const TXDMA_PQ_MAP: u32 = 0x010c;
pub const TRXFF_BNDY: u32 = 0x0114;
pub const RXFF_BNDY: u32 = 0x011c;
pub const C2HEVT: u32 = 0x01a0;
pub const HMETFR: u32 = 0x01cc;
pub const HMEBOX: [u32; 4] = [0x01d0, 0x01d4, 0x01d8, 0x01dc];
pub const HMEBOX_EX: [u32; 4] = [0x01f0, 0x01f4, 0x01f8, 0x01fc];
pub const FIFOPAGE_CTRL_2: u32 = 0x0204;
pub const BCN_VALID_V1: u16 = 1 << 15;
pub const AUTO_LLT_V1: u32 = 0x0208;
pub const TXDMA_OFFSET_CHK: u32 = 0x020c;
pub const TXDMA_STATUS: u32 = 0x0210;
pub const RQPN_CTRL_2: u32 = 0x022c;
pub const LD_RQPN: u32 = 1 << 31;
pub const FIFOPAGE_INFO: [u32; 5] = [0x0230, 0x0234, 0x0238, 0x023c, 0x0240];
pub const H2C_HEAD: u32 = 0x0244;
pub const H2C_TAIL: u32 = 0x0248;
pub const H2C_READ_ADDR: u32 = 0x024c;
pub const H2C_INFO: u32 = 0x0254;
pub const FWHW_TXQ_CTRL: u32 = 0x0420;
pub const EN_BCNQ_DL: u8 = 1 << 6; // bit 22, en el byte +2
pub const EN_WR_FREE_TAIL: u8 = 1 << 4; // bit 20, en el byte +2
pub const BCNQ_BDNY_V1: u32 = 0x0424;
pub const RRSR: u32 = 0x0440;
pub const CCK_CHECK: u32 = 0x0454;
pub const BCNQ1_BDNY_V1: u32 = 0x0456;
pub const AMPDU_MAX_TIME_V1: u32 = 0x0455;
pub const TX_HANG_CTRL: u32 = 0x045e;
pub const INIRTS_RATE_SEL: u32 = 0x0480;
pub const DATA_SC: u32 = 0x0483;
pub const PROT_MODE_CTRL: u32 = 0x04c8;
pub const BAR_MODE_CTRL: u32 = 0x04cc;
pub const PRECNT_CTRL: u32 = 0x04e5;
pub const EDCA_VO_PARAM: u32 = 0x0500;
pub const EDCA_VI_PARAM: u32 = 0x0504;
pub const PIFS: u32 = 0x0512;
pub const SIFS: u32 = 0x0514;
pub const SLOT: u32 = 0x051b;
pub const TX_PTCL_CTRL: u32 = 0x0520;
pub const TXPAUSE: u32 = 0x0522;
pub const TBTT_PROHIBIT: u32 = 0x0540;
pub const RD_NAV_NXT: u32 = 0x0544;
pub const BCN_CTRL: u32 = 0x0550;
pub const EN_BCN_FUNCTION: u8 = 1 << 3;
pub const DIS_TSF_UDT: u8 = 1 << 4;
pub const DRVERLYINT: u32 = 0x0558;
pub const BCNDMATIM: u32 = 0x0559;
pub const USTIME_TSF: u32 = 0x055c;
pub const RXTSF_OFFSET_CCK: u32 = 0x055e;
pub const TIMER0_SRC_SEL: u32 = 0x05b4;
pub const TCR: u32 = 0x0604;
pub const RCR: u32 = 0x0608;
pub const RX_PKT_LIMIT: u32 = 0x060c;
pub const RX_DRVINFO_SZ: u32 = 0x060f;
/// Dirección MAC del puerto 0 (la estación).
pub const MACID: u32 = 0x0610;
/// BSSID del puerto 0 (el punto de acceso al que está asociada).
pub const BSSID: u32 = 0x0618;
pub const USTIME_EDCA: u32 = 0x0638;
pub const ACKTO_CCK: u32 = 0x0639;
pub const WMAC_TRXPTCL_CTL: u32 = 0x0668;
pub const WMAC_TRXPTCL_CTL_H: u32 = 0x066c;
pub const RXFLTMAP0: u32 = 0x06a0;
pub const RXFLTMAP1: u32 = 0x06a2;
pub const RXFLTMAP2: u32 = 0x06a4;
pub const AID: u32 = 0x06a8;
pub const BT_COEX_TABLE0: u32 = 0x06c0;
pub const BT_COEX_TABLE1: u32 = 0x06c4;
pub const BT_COEX_BRK_TABLE: u32 = 0x06c8;
pub const BT_COEX_TABLE_H: u32 = 0x06cc;
pub const SND_PTCL_CTRL: u32 = 0x0718;
pub const WMAC_OPTION_FUNCTION: u32 = 0x07d0;
pub const H2C_PKT_READADDR: u32 = 0x10d0;
pub const H2C_PKT_WRITEADDR: u32 = 0x10d4;
pub const CPU_DMEM_CON: u32 = 0x1080;
pub const WL_PLATFORM_RST: u32 = 1 << 16;
pub const DDMA_EN: u32 = 1 << 8;
pub const FW_DBG7: u32 = 0x10fc;
pub const CR_EXT: u32 = 0x1100;
pub const DDMA_CH0SA: u32 = 0x1200;
pub const DDMA_CH0DA: u32 = 0x1204;
pub const DDMA_CH0CTRL: u32 = 0x1208;
pub const DDMACH0_OWN: u32 = 1 << 31;
pub const DDMACH0_CHKSUM_EN: u32 = 1 << 29;
pub const DDMACH0_CHKSUM_STS: u32 = 1 << 27;
pub const DDMACH0_RESET_CHKSUM_STS: u32 = 1 << 25;
pub const DDMACH0_CHKSUM_CONT: u32 = 1 << 24;
pub const DDMACH0_DLEN: u32 = 0x3ffff;
pub const H2CQ_CSR: u32 = 0x1330;
pub const H2CQ_FULL: u32 = 1 << 31;
pub const FAST_EDCA_VOVI: u32 = 0x1448;
pub const FAST_EDCA_BEBK: u32 = 0x144c;
pub const LTECOEX_CTRL: u32 = 0x1700;
pub const LTECOEX_WDATA: u32 = 0x1704;
pub const LTECOEX_RDATA: u32 = 0x1708;
pub const LTECOEX_READY: u32 = 1 << 29;

/// Direcciones internas del procesador de la placa (vistas desde su bus, no desde la PCI).
pub const OCP_TXBUF: u32 = 0x1878_0000;
pub const OCP_DMEM: u32 = 0x0020_0000;

// RCR: qué tramas acepta la placa.
pub const RCR_AAP: u32 = 1 << 0;
pub const RCR_CBSSID_DATA: u32 = 1 << 6;
pub const RCR_CBSSID_BCN: u32 = 1 << 7;
pub const RCR_APP_PHYSTS: u32 = 1 << 28;
pub const RCR_VHT_DACK: u32 = 1 << 26;
/// La configuración inicial de rtw8821c.h (WLAN_RCR_CFG).
pub const RCR_INIT: u32 = 0xe400_220e;

// --- PCI (pci.h) -------------------------------------------------------------------------------
pub const PCI_CTRL: u32 = 0x0300;
pub const RST_TRXDMA_INTF: u32 = 1 << 20;
pub const RX_TAG_EN: u32 = 1 << 15;
pub const PCI_HIMR0: u32 = 0x00b0;
pub const PCI_HISR0: u32 = 0x00b4;
pub const PCI_HIMR1: u32 = 0x00b8;
pub const PCI_HISR1: u32 = 0x00bc;
pub const PCI_HIMR3: u32 = 0x10b8;
pub const PCI_HISR3: u32 = 0x10bc;
pub const PCI_TXBD_DESA_BCNQ: u32 = 0x0308;
pub const PCI_TXBD_DESA_MGMTQ: u32 = 0x0310;
pub const PCI_TXBD_DESA_VOQ: u32 = 0x0318;
pub const PCI_TXBD_DESA_VIQ: u32 = 0x0320;
pub const PCI_TXBD_DESA_BEQ: u32 = 0x0328;
pub const PCI_TXBD_DESA_BKQ: u32 = 0x0330;
pub const PCI_RXBD_DESA_MPDUQ: u32 = 0x0338;
pub const PCI_TXBD_DESA_HI0Q: u32 = 0x0340;
pub const PCI_TXBD_DESA_H2CQ: u32 = 0x1320;
pub const PCI_TXBD_NUM_MGMTQ: u32 = 0x0380;
pub const PCI_RXBD_NUM_MPDUQ: u32 = 0x0382;
pub const PCI_TXBD_NUM_VOQ: u32 = 0x0384;
pub const PCI_TXBD_NUM_VIQ: u32 = 0x0386;
pub const PCI_TXBD_NUM_BEQ: u32 = 0x0388;
pub const PCI_TXBD_NUM_BKQ: u32 = 0x038a;
pub const PCI_TXBD_NUM_HI0Q: u32 = 0x038c;
pub const PCI_TXBD_NUM_H2CQ: u32 = 0x1328;
pub const PCI_TXBD_BCN_WORK: u32 = 0x0383;
pub const PCI_BCNQ_FLAG: u8 = 1 << 4;
pub const PCI_TXBD_RWPTR_CLR: u32 = 0x039c;
pub const PCI_TXBD_IDX_VOQ: u32 = 0x03a0;
pub const PCI_TXBD_IDX_VIQ: u32 = 0x03a4;
pub const PCI_TXBD_IDX_BEQ: u32 = 0x03a8;
pub const PCI_TXBD_IDX_BKQ: u32 = 0x03ac;
pub const PCI_TXBD_IDX_MGMTQ: u32 = 0x03b0;
pub const PCI_RXBD_IDX_MPDUQ: u32 = 0x03b4;
pub const PCI_TXBD_IDX_HI0Q: u32 = 0x03b8;
pub const PCI_TXBD_IDX_H2CQ: u32 = 0x132c;
pub const PCI_TXBD_H2CQ_CSR: u32 = 0x1330;
pub const CLR_H2CQ_HOST_IDX: u32 = 1 << 16;
pub const CLR_H2CQ_HW_IDX: u32 = 1 << 8;

/// Avisos que se piden (HIMR0): terminó de mandar (por cola), recibió, mensaje del firmware.
pub const IMR0: u32 =
    (1 << 7) | (1 << 6) | (1 << 5) | (1 << 4) | (1 << 3) | (1 << 2) | 1 | (1 << 10);
/// HIMR1: se llenó la FIFO de TX.
pub const IMR1: u32 = 1 << 9;
/// HIMR3: terminó un comando H2C.
pub const IMR3: u32 = 1 << 16;
pub const IMR_ROK: u32 = 1 << 0;

// --- banda base y radio ------------------------------------------------------------------------
pub const RXPSEL: u32 = 0x0808;
pub const RX_PSEL_RST: u32 = (1 << 28) | (1 << 29);
pub const RXCCAMSK: u32 = 0x0814;
pub const CLKTRK: u32 = 0x0860;
pub const ADCCLK: u32 = 0x08ac;
pub const ADC160: u32 = 0x08c4;
pub const CHFIR: u32 = 0x08f0;
pub const ACBB0: u32 = 0x0948;
pub const ACBBRXFIR: u32 = 0x094c;
pub const RXSB: u32 = 0x0a00;
pub const CCA_FLTR: u32 = 0x0a20;
pub const TXSF2: u32 = 0x0a24;
pub const TXSF6: u32 = 0x0a28;
pub const CCK0_FAREPORT: u32 = 0x0a2c;
pub const ENTXCCK: u32 = 0x0a80;
pub const ENRXCCA: u32 = 0x0a84;
pub const TXFILTER: u32 = 0x0aac;
pub const TXSCALE_A: u32 = 0x0c1c;
pub const TXDFIR: u32 = 0x0c20;
pub const RFE_CTRL8: u32 = 0x0cb4;
pub const RFECTL: u32 = 0x0cb8;
pub const BTG_SWITCH: u32 = 1 << 16;
pub const CTRL_SWITCH: u32 = 1 << 18;
pub const WL_SWITCH: u32 = (1 << 20) | (1 << 22);
pub const WLG_SWITCH: u32 = 1 << 21;
pub const WLA_SWITCH: u32 = 1 << 23;
pub const DMEM_CTRL: u32 = 0x1080;
pub const WL_RST: u32 = 1 << 16;
pub const SYS_CTRL: u32 = 0x0000;
pub const FEN_EN: u32 = 1 << 26;
/// Potencia de transmisión por velocidad del camino A (un byte por velocidad).
pub const TXAGC_A: u32 = 0x1d00;
pub const IQKFAILMSK: u32 = 0x1bf0;

/// Lectura directa de los registros de la radio (camino A): base + dirección × 4.
pub const RF_BASE_A: u32 = 0x2800;
/// Escritura serie (SIPI) de la radio, camino A.
pub const RF_SIPI_A: u32 = 0x0c90;
pub const RF_MASK: u32 = 0xfffff;
pub const RF_CHANNEL: u32 = 0x18;
pub const RF_LUTWA: u32 = 0x33;
pub const RF_LUTWD0: u32 = 0x3f;
pub const RF_LUTWE2: u32 = 0xee;
pub const RF_LUTDBG: u32 = 0xdf;
pub const RF_XTALX2: u32 = 0xb8;
pub const RF_DTXLOK: u32 = 0x08;
