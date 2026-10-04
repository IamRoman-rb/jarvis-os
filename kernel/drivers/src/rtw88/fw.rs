//! El firmware de la placa y cómo se le habla.
//!
//! **El archivo** (`rtw8821c_fw.bin`, de linux-firmware) es una cabecera de 64 bytes y tres
//! secciones para la memoria del procesador de la placa: DMEM (datos), IMEM (código) y,
//! opcional, EMEM. Cada sección termina en 8 bytes de suma de control.
//!
//! **La copia** (`__rtw_download_firmware`): el procesador se apaga, cada sección se manda en
//! pedazos de 4 KiB por la cola de beacons a la "página reservada" de la memoria de TX de la
//! placa, y un DMA interno (DDMA) la copia de ahí a su lugar, verificando la suma. Al final se
//! enciende el procesador y se espera a que diga "listo".
//!
//! **Los mensajes** del sistema al firmware (H2C) son de 8 bytes por cuatro buzones de registros
//! (`HMEBOX0..3`), o de 32 bytes por la cola H2C (los "paquetes", con encabezado y número de
//! secuencia).

use alloc::vec::Vec;

use super::desc::{self, TX_DESC_SIZE};
use super::{Bus, BusExt, Error, Queue, regs};

pub const HEADER: usize = 64;
const CHECKSUM: usize = 8;
/// Pedazos de la copia por la página reservada.
const CHUNK: usize = 0x1000;

/// Lo que dice la cabecera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub sub_version: u8,
    pub sub_index: u8,
    pub dmem_addr: u32,
    pub dmem_size: usize,
    pub imem_addr: u32,
    pub imem_size: usize,
    pub emem_addr: u32,
    pub emem_size: usize,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Lee y verifica la cabecera: los tamaños tienen que sumar el largo del archivo. Los tamaños
/// incluyen la suma de control de cada sección.
pub fn parse(fw: &[u8]) -> Result<Header, Error> {
    if fw.len() < HEADER {
        return Err(Error::Firmware("archivo demasiado corto"));
    }
    if u16::from_le_bytes([fw[0], fw[1]]) != 0x8821 {
        return Err(Error::Firmware("no es de la RTL8821C"));
    }
    let dmem = le32(fw, 0x24) as usize + CHECKSUM;
    let imem = le32(fw, 0x30) as usize + CHECKSUM;
    let emem = if fw[0x18] & (1 << 4) != 0 {
        le32(fw, 0x34) as usize + CHECKSUM
    } else {
        0
    };
    if HEADER + dmem + imem + emem != fw.len() {
        return Err(Error::Firmware("los tamaños no coinciden con el archivo"));
    }
    Ok(Header {
        version: u16::from_le_bytes([fw[4], fw[5]]),
        sub_version: fw[6],
        sub_index: fw[7],
        dmem_addr: le32(fw, 0x20) & !(1 << 31),
        dmem_size: dmem,
        imem_addr: le32(fw, 0x3c) & !(1 << 31),
        imem_size: imem,
        emem_addr: le32(fw, 0x38) & !(1 << 31),
        emem_size: emem,
    })
}

/// Escribe `data` en la página reservada `page` de la memoria de TX
/// (`rtw_fw_write_data_rsvd_page`): por la cola de beacons, con el beacon "válido" como aviso
/// de que llegó.
pub fn write_rsvd_page<B: Bus + ?Sized>(
    bus: &mut B,
    page: u16,
    data: &[u8],
    boundary: u16,
) -> Result<(), Error> {
    let bcn_ctrl = bus.read8(regs::BCN_CTRL);
    bus.write16(regs::FIFOPAGE_CTRL_2, (page & 0xfff) | regs::BCN_VALID_V1);
    let cr1 = bus.read8(regs::CR + 1);
    bus.write8(regs::CR + 1, cr1 | regs::ENSWBCN);
    bus.write8(
        regs::BCN_CTRL,
        (bcn_ctrl & !regs::EN_BCN_FUNCTION) | regs::DIS_TSF_UDT,
    );
    let txq = bus.read8(regs::FWHW_TXQ_CTRL + 2);
    bus.write8(regs::FWHW_TXQ_CTRL + 2, txq & !regs::EN_BCNQ_DL);
    let mut packet = Vec::with_capacity(TX_DESC_SIZE + data.len());
    packet.extend_from_slice(&desc::rsvd_page_desc(data.len()));
    packet.extend_from_slice(data);
    let sent = bus.tx(Queue::Bcn, &packet);
    if sent {
        // Las páginas reservadas van por la cola de beacons: avisar que hay una.
        bus.set8(regs::PCI_TXBD_BCN_WORK, regs::PCI_BCNQ_FLAG);
    }
    let ok = sent && bus.wait32(regs::FIFOPAGE_CTRL_2, u32::from(regs::BCN_VALID_V1), 1);
    bus.write16(regs::FIFOPAGE_CTRL_2, boundary | regs::BCN_VALID_V1);
    bus.write8(regs::BCN_CTRL, bcn_ctrl);
    bus.write8(regs::FWHW_TXQ_CTRL + 2, txq);
    bus.write8(regs::CR + 1, cr1);
    if ok {
        Ok(())
    } else {
        Err(Error::Firmware("la página reservada no llegó"))
    }
}

/// Copia una sección del firmware a su lugar en la memoria de la placa.
fn download_section<B: Bus + ?Sized>(bus: &mut B, data: &[u8], dst: u32) -> Result<(), Error> {
    bus.set32(regs::DDMA_CH0CTRL, regs::DDMACH0_RESET_CHKSUM_STS);
    for (n, chunk) in data.chunks(CHUNK).enumerate() {
        // La página 0 de la memoria de TX (la cola de beacons se reubica ahí durante la copia).
        write_rsvd_page(bus, 0, chunk, 0)?;
        if !bus.wait32(regs::DDMA_CH0CTRL, regs::DDMACH0_OWN, 0) {
            return Err(Error::Firmware("el DMA interno está ocupado"));
        }
        let mut ctrl = regs::DDMACH0_CHKSUM_EN | regs::DDMACH0_OWN;
        ctrl |= chunk.len() as u32 & regs::DDMACH0_DLEN;
        if n > 0 {
            ctrl |= regs::DDMACH0_CHKSUM_CONT;
        }
        // Origen: la página 0 del buffer de TX, salteando el descriptor.
        bus.write32(regs::DDMA_CH0SA, regs::OCP_TXBUF + TX_DESC_SIZE as u32);
        bus.write32(regs::DDMA_CH0DA, dst + (n * CHUNK) as u32);
        bus.write32(regs::DDMA_CH0CTRL, ctrl);
        if !bus.wait32(regs::DDMA_CH0CTRL, regs::DDMACH0_OWN, 0) {
            return Err(Error::Firmware("el DMA interno no terminó"));
        }
    }
    // ¿La suma de control dio? (Y se anota en MCUFW_CTRL qué memoria quedó bien.)
    let mut ctl = bus.read8(regs::MCUFW_CTRL);
    let bad = bus.read32(regs::DDMA_CH0CTRL) & regs::DDMACH0_CHKSUM_STS != 0;
    let (dw, ok) = if dst < regs::OCP_DMEM {
        (regs::IMEM_DW_OK, regs::IMEM_CHKSUM_OK)
    } else {
        (regs::DMEM_DW_OK, regs::DMEM_CHKSUM_OK)
    };
    ctl |= dw;
    if bad {
        bus.write8(regs::MCUFW_CTRL, ctl & !ok);
        return Err(Error::Firmware("suma de control incorrecta"));
    }
    bus.write8(regs::MCUFW_CTRL, ctl | ok);
    Ok(())
}

fn cpu_enable<B: Bus + ?Sized>(bus: &mut B, on: bool) {
    if on {
        bus.set8(regs::RSV_CTRL + 1, regs::WLMCU_IOIF);
        bus.set8(regs::SYS_FUNC_EN + 1, regs::FEN_CPUEN);
    } else {
        bus.clr8(regs::SYS_FUNC_EN + 1, regs::FEN_CPUEN);
        bus.clr8(regs::RSV_CTRL + 1, regs::WLMCU_IOIF);
    }
}

/// Lee un registro del coexistidor LTE/Bluetooth (indirecto, por 0x1700).
pub fn ltecoex_read<B: Bus + ?Sized>(bus: &mut B, offset: u16) -> Option<u32> {
    if !bus.wait32(regs::LTECOEX_CTRL, regs::LTECOEX_READY, 1) {
        return None;
    }
    bus.write32(regs::LTECOEX_CTRL, 0x800f_0000 | u32::from(offset));
    Some(bus.read32(regs::LTECOEX_RDATA))
}

pub fn ltecoex_write<B: Bus + ?Sized>(bus: &mut B, offset: u16, value: u32) -> bool {
    if !bus.wait32(regs::LTECOEX_CTRL, regs::LTECOEX_READY, 1) {
        return false;
    }
    bus.write32(regs::LTECOEX_WDATA, value);
    bus.write32(regs::LTECOEX_CTRL, 0xc00f_0000 | u32::from(offset));
    true
}

/// Carga el firmware (`__rtw_download_firmware`). Al volver, el procesador de la placa corre.
pub fn download<B: Bus + ?Sized>(bus: &mut B, fw: &[u8]) -> Result<Header, Error> {
    let h = parse(fw)?;
    let lte = ltecoex_read(bus, 0x38).ok_or(Error::Firmware("el coexistidor no responde"))?;
    cpu_enable(bus, false);
    // Copia de los registros que se tocan (`download_firmware_reg_backup`).
    let pq_map = bus.read8(regs::TXDMA_PQ_MAP + 1);
    let cr = bus.read8(regs::CR);
    let fifo1 = bus.read16(regs::FIFOPAGE_INFO[0]);
    let rqpn = bus.read32(regs::RQPN_CTRL_2) | regs::LD_RQPN;
    let bcn = bus.read8(regs::BCN_CTRL);
    // Solo la cola alta (HIQ), con prioridad alta y 0x200 páginas.
    bus.write8(regs::TXDMA_PQ_MAP + 1, 3 << 6);
    bus.write8(regs::CR, regs::HCI_TXDMA_EN | regs::TXDMA_EN);
    bus.write32(regs::H2CQ_CSR, regs::H2CQ_FULL);
    bus.write16(regs::FIFOPAGE_INFO[0], 0x200);
    bus.write32(regs::RQPN_CTRL_2, rqpn);
    bus.write8(
        regs::BCN_CTRL,
        (bcn & !regs::EN_BCN_FUNCTION) | regs::DIS_TSF_UDT,
    );
    // Reiniciar la plataforma del procesador (`download_firmware_reset_platform`).
    bus.clr8(regs::CPU_DMEM_CON + 2, (regs::WL_PLATFORM_RST >> 16) as u8);
    bus.clr8(regs::SYS_CLK_CTRL + 1, (regs::CPU_CLK_EN >> 8) as u8);
    bus.set8(regs::CPU_DMEM_CON + 2, (regs::WL_PLATFORM_RST >> 16) as u8);
    bus.set8(regs::SYS_CLK_CTRL + 1, (regs::CPU_CLK_EN >> 8) as u8);

    let ctl = (bus.read16(regs::MCUFW_CTRL) & 0x3800) | regs::MCUFWDL_EN;
    bus.write16(regs::MCUFW_CTRL, ctl);
    let mut at = HEADER;
    let mut result = Ok(());
    for (size, addr) in [
        (h.dmem_size, h.dmem_addr),
        (h.imem_size, h.imem_addr),
        (h.emem_size, h.emem_addr),
    ] {
        if size == 0 {
            continue;
        }
        result = download_section(bus, &fw[at..at + size], addr);
        if result.is_err() {
            break;
        }
        at += size;
    }
    if let Err(e) = result {
        bus.clr8(regs::MCUFW_CTRL, regs::MCUFWDL_EN as u8);
        bus.set8(regs::SYS_FUNC_EN + 1, regs::FEN_CPUEN);
        return Err(e);
    }
    // Devolver los registros.
    bus.write8(regs::TXDMA_PQ_MAP + 1, pq_map);
    bus.write8(regs::CR, cr);
    bus.write32(regs::H2CQ_CSR, regs::H2CQ_FULL);
    bus.write16(regs::FIFOPAGE_INFO[0], fifo1);
    bus.write32(regs::RQPN_CTRL_2, rqpn);
    bus.write8(regs::BCN_CTRL, bcn);
    // Fin (`download_firmware_end_flow`): si las dos memorias quedaron bien, "copiado".
    bus.write32(regs::TXDMA_STATUS, 1 << 2);
    let ctl = bus.read16(regs::MCUFW_CTRL);
    if ctl & regs::CHECK_SUM_OK == regs::CHECK_SUM_OK {
        bus.write16(
            regs::MCUFW_CTRL,
            (ctl | regs::FW_DW_RDY) & !regs::MCUFWDL_EN,
        );
    }
    cpu_enable(bus, true);
    if !ltecoex_write(bus, 0x38, lte) {
        return Err(Error::Firmware("el coexistidor no responde"));
    }
    // Esperar a que el firmware arranque (hasta ~1 s).
    let mut ready = false;
    for _ in 0..100 {
        if bus.wait32(regs::MCUFW_CTRL, regs::FW_READY_MASK, regs::FW_READY) {
            ready = true;
            break;
        }
    }
    if !ready {
        let key = bus.read32(regs::FW_DBG7) & 0xffff_ff00;
        return Err(Error::Firmware(if key == 0xfaaa_aa00 {
            "la placa rechazó la clave del firmware"
        } else {
            "no arrancó"
        }));
    }
    bus.hci_setup();
    Ok(h)
}

// --- mensajes al firmware (H2C) ------------------------------------------------------------------

/// Los buzones de 8 bytes: se usan de a uno, en ronda.
#[derive(Default)]
pub struct H2c {
    next_box: usize,
    seq: u16,
}

impl H2c {
    /// Manda un comando de 8 bytes por el próximo buzón libre (`rtw_fw_send_h2c_command`).
    pub fn command<B: Bus + ?Sized>(&mut self, bus: &mut B, cmd: [u8; 8]) -> bool {
        let b = self.next_box;
        let mut free = false;
        for _ in 0..30 {
            if bus.read8(regs::HMETFR) >> b & 1 == 0 {
                free = true;
                break;
            }
            bus.delay_us(100);
        }
        if !free {
            return false;
        }
        bus.write32(
            regs::HMEBOX_EX[b],
            u32::from_le_bytes([cmd[4], cmd[5], cmd[6], cmd[7]]),
        );
        bus.write32(
            regs::HMEBOX[b],
            u32::from_le_bytes([cmd[0], cmd[1], cmd[2], cmd[3]]),
        );
        self.next_box = (b + 1) % 4;
        true
    }

    /// Manda un paquete H2C de 32 bytes por la cola H2C (`rtw_fw_send_h2c_packet`).
    pub fn packet<B: Bus + ?Sized>(&mut self, bus: &mut B, mut pkt: [u8; 32]) -> bool {
        pkt[6..8].copy_from_slice(&self.seq.to_le_bytes());
        self.seq = self.seq.wrapping_add(1);
        let mut buf = Vec::with_capacity(TX_DESC_SIZE + 32);
        buf.extend_from_slice(&desc::h2c_desc(32));
        buf.extend_from_slice(&pkt);
        bus.tx(Queue::H2c, &buf)
    }

    /// El firmware se reinició: los buzones y la secuencia vuelven a cero.
    pub fn reset(&mut self) {
        *self = H2c::default();
    }
}

/// Encabezado de un paquete H2C: categoría 1, comando 0xFF, subcomando y largo total.
fn packet_header(sub: u16, payload: u16) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = 0x01;
    p[1] = 0xff;
    p[2..4].copy_from_slice(&sub.to_le_bytes());
    p[4..6].copy_from_slice(&(8 + payload).to_le_bytes());
    p
}

/// "Información general": dónde empieza el buffer de TX del firmware, en páginas desde el borde
/// de las páginas reservadas.
pub fn general_info(fw_txbuf_offset: u8) -> [u8; 32] {
    let mut p = packet_header(0x0d, 4);
    p[10] = fw_txbuf_offset;
    p
}

/// "Información de la PHY": tipo de antena, 1T1R, versión del chip, camino A.
pub fn phydm_info(rfe: u8, cut: u8) -> [u8; 32] {
    let mut p = packet_header(0x11, 8);
    p[8] = rfe;
    p[9] = 0; // FW_RF_1T1R
    p[10] = cut;
    p[11] = 0x11; // RX y TX por el camino A
    p
}

/// Que el firmware calibre la radio (IQK).
pub fn iqk(segment: bool) -> [u8; 32] {
    let mut p = packet_header(0x0e, 1);
    p[8] = if segment { 2 } else { 0 };
    p
}

/// "Conectado" o "desconectado" (`rtw_fw_media_status_report`), para la estación `mac_id`.
pub fn media_status(mac_id: u8, connected: bool) -> [u8; 8] {
    [0x01, u8::from(connected), mac_id, 0, 0, 0, 0, 0]
}

/// Que el firmware elija la velocidad de transmisión (*rate adaptation*) entre las de `mask`.
pub fn ra_info(mac_id: u8, rate_id: u8, mask: u32, vht: bool, sgi: bool) -> [u8; 8] {
    let mut w0: u32 = 0x40;
    w0 |= u32::from(mac_id) << 8;
    w0 |= u32::from(rate_id & 0x1f) << 16;
    w0 |= u32::from(sgi) << 23;
    // 20 MHz, sin LDPC; VHT si la red lo tiene.
    w0 |= u32::from(vht) << 28;
    w0 |= 1 << 30; // sin seguimiento de potencia
    let mut c = [0u8; 8];
    c[..4].copy_from_slice(&w0.to_le_bytes());
    c[4..].copy_from_slice(&mask.to_le_bytes());
    c
}

/// En la 8821CE por PCIe, recuperar el Bluetooth del combo (`rtw_fw_set_recover_bt_device`).
pub fn recover_bt() -> [u8; 8] {
    [0xd1, 0x01, 0, 0, 0, 0, 0, 0]
}
