//! Información de la CPU con la instrucción `cpuid`: el nombre comercial del procesador y el
//! sensor térmico.
//! Referencias: <https://wiki.osdev.org/CPUID>, el manual de Intel (SDM vol. 3B, 15.8
//! "Platform Specific Power Management Support": sensor térmico digital) y, para AMD, el driver
//! `k10temp` de Linux (ver `jarvis_drivers::sensors`).

use alloc::string::String;
use alloc::vec::Vec;
use core::arch::x86_64::__cpuid;

use jarvis_drivers::sensors;
use x86_64::registers::model_specific::Msr;

use crate::{acpi, pci, time};

/// `IA32_THERM_STATUS`: bits 22:16 = cuántos grados faltan para TjMax; bit 31 = lectura válida.
const IA32_THERM_STATUS: u32 = 0x19C;
/// `MSR_TEMPERATURE_TARGET`: bits 23:16 = TjMax (la temperatura máxima de la CPU).
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;
/// Cada cuánto se evalúa `_TMP` (es código del firmware: puede hablar con el controlador
/// embebido, que es lento).
const ACPI_EVERY_MS: u64 = 5000;

/// De dónde sale la temperatura.
pub enum Thermal {
    /// El sensor térmico digital (DTS) de las CPU Intel.
    Intel { tjmax: u8 },
    /// El registro Tctl de los Ryzen (por SMN, a través del complejo raíz PCI).
    Amd,
    /// Las zonas térmicas del AML (las rutas de sus `_TMP`), con la última lectura.
    Acpi {
        zones: Vec<String>,
        last: Option<u8>,
        at: Option<u64>,
    },
}

/// ¿Corre en una máquina virtual (CPUID.1:ECX[31])? QEMU y las demás no emulan los sensores,
/// aunque a veces copien los bits de la CPU real.
fn hypervisor() -> bool {
    __cpuid(1).ecx & (1 << 31) != 0
}

impl Thermal {
    /// `None` si no hay un sensor que se pueda leer sin riesgo. Leer un MSR que no existe causa
    /// una excepción (#GP), así que solo se lee si `cpuid` confirma que está. Llamarla después
    /// de cargar el AML (para las zonas térmicas).
    pub fn detect() -> Option<Thermal> {
        let v = __cpuid(0);
        let intel = (v.ebx, v.edx, v.ecx) == (0x756e_6547, 0x4965_6e69, 0x6c65_746e);
        let amd = (v.ebx, v.edx, v.ecx) == (0x6874_7541, 0x6974_6e65, 0x444d_4163);
        if !hypervisor() {
            if intel && v.eax >= 6 && __cpuid(6).eax & 1 != 0 {
                return Some(Thermal::Intel {
                    tjmax: intel_tjmax(),
                });
            }
            // El complejo raíz (00:00.0) tiene que ser de AMD: por ahí se llega al SMN.
            if amd
                && sensors::amd_family_has_smn_temp(sensors::cpuid_family(__cpuid(1).eax))
                && pci::read_config(0, 0, 0, 0) & 0xFFFF == 0x1022
            {
                return Some(Thermal::Amd);
            }
        }
        let zones = acpi::get().map(|a| a.thermal_zones()).unwrap_or_default();
        (!zones.is_empty()).then_some(Thermal::Acpi {
            zones,
            last: None,
            at: None,
        })
    }

    /// Para Configuración → Hardware.
    pub fn describe(&self) -> String {
        match self {
            Thermal::Intel { tjmax } => alloc::format!("Intel DTS (TjMax {tjmax} °C)"),
            Thermal::Amd => String::from("AMD Tctl (SMN)"),
            Thermal::Acpi { zones, .. } => alloc::format!("ACPI: {}", zones.join(", ")),
        }
    }

    /// Grados Celsius, si la lectura es válida.
    pub fn read(&mut self) -> Option<u8> {
        match self {
            Thermal::Intel { tjmax } => {
                // SAFETY: `detect` confirmó con cpuid que el sensor (y su MSR) existe; leerlo no
                // tiene efectos.
                let v = unsafe { Msr::new(IA32_THERM_STATUS).read() };
                if v & (1 << 31) == 0 {
                    return None;
                }
                let below = ((v >> 16) & 0x7F) as u8;
                Some(tjmax.saturating_sub(below))
            }
            Thermal::Amd => {
                pci::write_config(0, 0, 0, sensors::AMD_SMN_INDEX, sensors::AMD_SMN_TEMP);
                let raw = pci::read_config(0, 0, 0, sensors::AMD_SMN_DATA);
                sensors::plausible_celsius(sensors::amd_tctl_millideg(raw))
            }
            Thermal::Acpi { zones, last, at } => {
                let now = time::millis();
                if at.is_none_or(|t| now - t >= ACPI_EVERY_MS) {
                    let acpi = acpi::get()?;
                    // La más caliente de las zonas.
                    *last = zones
                        .iter()
                        .filter_map(|z| acpi.integer(&alloc::format!("{z}._TMP")))
                        .filter_map(sensors::acpi_tmp_celsius)
                        .max();
                    *at = Some(now);
                }
                *last
            }
        }
    }
}

/// TjMax está en un MSR desde Nehalem (familia 6, modelo 0x1A); antes era 100 °C.
fn intel_tjmax() -> u8 {
    let l1 = __cpuid(1);
    let family = (l1.eax >> 8) & 0xF;
    let model = ((l1.eax >> 4) & 0xF) | ((l1.eax >> 12) & 0xF0);
    if family == 6 && model >= 0x1A {
        // SAFETY: el MSR existe en esta CPU (Intel, familia 6, modelo ≥ Nehalem); leerlo no tiene
        // efectos.
        let t = ((unsafe { Msr::new(MSR_TEMPERATURE_TARGET).read() } >> 16) & 0xFF) as u8;
        if t == 0 { 100 } else { t }
    } else {
        100
    }
}

/// "Intel(R) Core(TM) i5-…" o "QEMU Virtual CPU…". Vacío si la CPU no lo informa.
pub fn brand() -> String {
    // `cpuid` existe en toda CPU x86_64 y solo lee información (en Rust es una función segura).
    let max = __cpuid(0x8000_0000).eax;
    if max < 0x8000_0004 {
        return String::new();
    }
    let mut bytes = [0u8; 48];
    for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
        let r = __cpuid(leaf); // las hojas 0x80000002..4 existen: se verificó el máximo arriba

        for (j, reg) in [r.eax, r.ebx, r.ecx, r.edx].into_iter().enumerate() {
            let o = i * 16 + j * 4;
            bytes[o..o + 4].copy_from_slice(&reg.to_le_bytes());
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    String::from(text.trim_matches(|c: char| c == '\0' || c.is_whitespace()))
}
