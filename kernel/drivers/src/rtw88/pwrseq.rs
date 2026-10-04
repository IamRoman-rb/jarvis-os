//! Encender y apagar la placa: secuencias de escrituras y esperas que publica Realtek
//! (`card_enable_flow_8821c` y `card_disable_flow_8821c` en rtw8821c.c).
//!
//! La placa pasa por tres estados: apagada (*card disable*), emulación (*card emu*: responde a la
//! PCI pero la MAC duerme) y activa. Cada transición es una lista de pasos sobre registros de un
//! byte. En Linux las tablas sirven para PCIe, USB y SDIO; acá están solo los pasos de la PCIe.

use super::{Bus, BusExt, Error, regs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Cambia los bits `mask` del byte `addr` a `value`.
    Write { addr: u16, mask: u8, value: u8 },
    /// Espera a que los bits `mask` del byte `addr` valgan `value`.
    Poll { addr: u16, mask: u8, value: u8 },
}

const fn w(addr: u16, mask: u8, value: u8) -> Step {
    Step::Write { addr, mask, value }
}

const fn p(addr: u16, mask: u8, value: u8) -> Step {
    Step::Poll { addr, mask, value }
}

/// Apagada → emulación → activa.
pub const POWER_ON: &[Step] = &[
    // trans_carddis_to_cardemu_8821c
    w(0x0005, 0x98, 0x00),
    w(0x0300, 0xff, 0x00),
    w(0x0301, 0xff, 0x00),
    // trans_cardemu_to_act_8821c
    w(0x0005, 0x1c, 0x00),
    w(0x0075, 0x01, 0x01),
    p(0x0006, 0x02, 0x02),
    w(0x0075, 0x01, 0x00),
    w(0x0006, 0x01, 0x01),
    w(0x0005, 0x80, 0x00),
    w(0x0005, 0x18, 0x00),
    w(0x0005, 0x01, 0x01),
    p(0x0005, 0x01, 0x00),
    w(0x0020, 0x08, 0x08),
    w(0x0074, 0x20, 0x20),
    w(0x0022, 0x02, 0x00),
    w(0x0062, 0xe0, 0xe0),
    w(0x0061, 0xe0, 0x00),
    w(0x007c, 0x02, 0x00),
];

/// Activa → emulación → apagada.
pub const POWER_OFF: &[Step] = &[
    // trans_act_to_cardemu_8821c
    w(0x0093, 0x08, 0x00),
    w(0x001f, 0xff, 0x00),
    w(0x0049, 0x02, 0x00),
    w(0x0006, 0x01, 0x01),
    w(0x0002, 0x02, 0x00),
    w(0x0005, 0x02, 0x02),
    p(0x0005, 0x02, 0x00),
    w(0x0020, 0x08, 0x00),
    // trans_cardemu_to_carddis_8821c
    w(0x0067, 0x20, 0x00),
    w(0x0005, 0x04, 0x04),
    w(0x0081, 0xc0, 0x00),
    w(0x0090, 0x02, 0x00),
];

/// Espera hasta 1 s (20 000 × 50 µs, RTW_PWR_POLLING_CNT).
fn poll<B: Bus + ?Sized>(bus: &mut B, addr: u32, mask: u8, value: u8) -> bool {
    for _ in 0..20_000 {
        if bus.read8(addr) & mask == value & mask {
            return true;
        }
        bus.delay_us(50);
    }
    false
}

/// Corre una secuencia. Si una espera no responde, en la PCIe se prueba una vez más después de
/// mover el bit PFM_WOWL (así lo hace Linux: despierta a la placa de un estado intermedio).
pub fn run<B: Bus + ?Sized>(bus: &mut B, steps: &[Step]) -> Result<(), Error> {
    for step in steps {
        match *step {
            Step::Write { addr, mask, value } => {
                let a = u32::from(addr);
                let old = bus.read8(a);
                bus.write8(a, (old & !mask) | (value & mask));
            }
            Step::Poll { addr, mask, value } => {
                let a = u32::from(addr);
                if poll(bus, a, mask, value) {
                    continue;
                }
                let v = bus.read8(regs::SYS_PW_CTRL);
                bus.write8(regs::SYS_PW_CTRL, v | regs::PFM_WOWL);
                bus.write8(regs::SYS_PW_CTRL, v & !regs::PFM_WOWL);
                if !poll(bus, a, mask, value) {
                    return Err(Error::Power(addr));
                }
            }
        }
    }
    Ok(())
}

/// ¿Está encendida? Apagada, el registro CR lee 0xEA.
pub fn powered<B: Bus + ?Sized>(bus: &mut B) -> bool {
    bus.read8(regs::CR) != 0xea
}

/// Lo de antes de la secuencia (`rtw_mac_pre_system_cfg`): sin suspensión de la PCI, los pines
/// de la antena para el Wi-Fi y la banda base y la radio apagadas.
pub fn pre_power_on<B: Bus + ?Sized>(bus: &mut B) {
    bus.write8(regs::RSV_CTRL, 0);
    bus.set32(regs::HCI_OPT_CTRL, regs::USB_SUS_DIS);
    bus.set32(regs::PAD_CTRL1, regs::PAPE_WLBT_SEL | regs::LNAON_WLBT_SEL);
    bus.clr32(regs::LED_CFG, regs::PAPE_SEL_EN | regs::LNAON_SEL_EN);
    bus.set32(regs::GPIO_MUXCFG, regs::WLRFE_4_5_EN);
    bus.clr8(regs::SYS_FUNC_EN, regs::FEN_BB_RSTB | regs::FEN_BB_GLB_RST);
    bus.clr8(
        regs::RF_CTRL,
        regs::RF_SDM_RSTB | regs::RF_RSTB | regs::RF_EN,
    );
    bus.clr32(regs::WLRF1, regs::WLRF1_BBRF_EN);
}

/// Lo de después (`__rtw_mac_init_system_cfg`): el DMA interno, las funciones del sistema y que
/// no arranque desde una flash propia (el firmware lo da el driver).
pub fn post_power_on<B: Bus + ?Sized>(bus: &mut B) {
    bus.set32(regs::CPU_DMEM_CON, regs::WL_PLATFORM_RST | regs::DDMA_EN);
    bus.set8(regs::SYS_FUNC_EN + 1, 0xd8);
    let v = (bus.read8(regs::CR_EXT + 3) & 0xf0) | 0x0c;
    bus.write8(regs::CR_EXT + 3, v);
    let fw = bus.read32(regs::MCUFW_CTRL);
    if fw & regs::BOOT_FSPI_EN != 0 {
        bus.write32(regs::MCUFW_CTRL, fw & !regs::BOOT_FSPI_EN);
        bus.clr32(regs::GPIO_MUXCFG, regs::FSPI_EN);
    }
}

/// Enciende la placa (`rtw_mac_power_on`). Si ya estaba encendida (un arranque anterior la dejó
/// así), primero la apaga: así se empieza siempre del mismo estado.
pub fn power_on<B: Bus + ?Sized>(bus: &mut B) -> Result<(), Error> {
    pre_power_on(bus);
    if powered(bus) {
        run(bus, POWER_OFF)?;
        pre_power_on(bus);
    }
    run(bus, POWER_ON)?;
    post_power_on(bus);
    Ok(())
}
