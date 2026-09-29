//! Información de la CPU con la instrucción `cpuid`: el nombre comercial del procesador y el
//! sensor térmico.
//! Referencias: <https://wiki.osdev.org/CPUID> y el manual de Intel (SDM vol. 3B, 15.8
//! "Platform Specific Power Management Support": sensor térmico digital).

use alloc::string::String;
use core::arch::x86_64::__cpuid;

use x86_64::registers::model_specific::Msr;

/// `IA32_THERM_STATUS`: bits 22:16 = cuántos grados faltan para TjMax; bit 31 = lectura válida.
const IA32_THERM_STATUS: u32 = 0x19C;
/// `MSR_TEMPERATURE_TARGET`: bits 23:16 = TjMax (la temperatura máxima de la CPU).
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

/// El sensor térmico digital (DTS) de las CPU Intel.
pub struct Thermal {
    tjmax: u8,
}

impl Thermal {
    /// `None` si no hay un sensor que se pueda leer sin riesgo. Leer un MSR que no existe causa
    /// una excepción (#GP), así que solo se lee si `cpuid` confirma que está:
    /// - fabricante Intel (AMD lo tiene en otro lado: registros SMN por PCI);
    /// - sin hipervisor (CPUID.1:ECX[31]): QEMU y las demás máquinas virtuales no emulan el
    ///   sensor, aunque a veces copien el bit de la CPU real;
    /// - CPUID.6:EAX[0] (DTS presente).
    pub fn detect() -> Option<Thermal> {
        let v = __cpuid(0);
        let intel = (v.ebx, v.edx, v.ecx) == (0x756e_6547, 0x4965_6e69, 0x6c65_746e);
        if !intel || v.eax < 6 {
            return None;
        }
        let l1 = __cpuid(1);
        if l1.ecx & (1 << 31) != 0 || __cpuid(6).eax & 1 == 0 {
            return None;
        }
        // TjMax está en un MSR desde Nehalem (familia 6, modelo 0x1A); antes era 100 °C.
        let family = (l1.eax >> 8) & 0xF;
        let model = ((l1.eax >> 4) & 0xF) | ((l1.eax >> 12) & 0xF0);
        let tjmax = if family == 6 && model >= 0x1A {
            // SAFETY: el MSR existe en esta CPU (Intel, familia 6, modelo ≥ Nehalem); leerlo no
            // tiene efectos.
            let t = ((unsafe { Msr::new(MSR_TEMPERATURE_TARGET).read() } >> 16) & 0xFF) as u8;
            if t == 0 { 100 } else { t }
        } else {
            100
        };
        Some(Thermal { tjmax })
    }

    /// Grados Celsius, si la lectura es válida.
    pub fn read(&self) -> Option<u8> {
        // SAFETY: `detect` confirmó con cpuid que el sensor (y su MSR) existe; leerlo no tiene
        // efectos.
        let v = unsafe { Msr::new(IA32_THERM_STATUS).read() };
        if v & (1 << 31) == 0 {
            return None;
        }
        let below = ((v >> 16) & 0x7F) as u8;
        Some(self.tjmax.saturating_sub(below))
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
