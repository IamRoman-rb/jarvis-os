//! Sensores de temperatura (K13): cómo se interpretan los números que dan el procesador y el
//! firmware.
//!
//! - **AMD (Zen, familias 17h y 19h)**: la temperatura de control (Tctl) está en el registro
//!   `THM_TCON_CUR_TMP` (dirección SMN 0x59800), que se lee por dos registros de configuración
//!   del complejo raíz PCI (00:00.0, índice en 0x60 y dato en 0x64). Los bits 31:21 son grados en
//!   octavos; si el bit 19 (`CUR_TEMP_RANGE_SEL`) está prendido, la escala empieza en −49 °C.
//!   Referencia: el driver `k10temp` de Linux (drivers/hwmon/k10temp.c).
//! - **ACPI**: las zonas térmicas (`ThermalZone` en el AML) tienen un método `_TMP` que devuelve
//!   décimas de kelvin (ACPI 6.5 §11.4.21). Muchas placas de escritorio no las declaran, y algunas
//!   devuelven un valor fijo o absurdo: se descarta lo que no parezca una temperatura.

/// Dirección SMN del registro de temperatura de los Zen.
pub const AMD_SMN_TEMP: u32 = 0x0005_9800;
/// Registros de configuración del complejo raíz para llegar al SMN (índice y dato).
pub const AMD_SMN_INDEX: u8 = 0x60;
pub const AMD_SMN_DATA: u8 = 0x64;

/// Tctl en milésimas de grado, a partir del valor crudo del registro.
pub fn amd_tctl_millideg(raw: u32) -> i32 {
    let temp = ((raw >> 21) & 0x7FF) as i32 * 125;
    if raw & (1 << 19) != 0 {
        temp - 49_000
    } else {
        temp
    }
}

/// Grados Celsius enteros (redondeados) si la lectura es creíble (de 1 a 125 °C).
pub fn plausible_celsius(millideg: i32) -> Option<u8> {
    let c = (millideg + 500).div_euclid(1000);
    (1..=125).contains(&c).then_some(c as u8)
}

/// Grados Celsius a partir de lo que devuelve `_TMP` (décimas de kelvin).
pub fn acpi_tmp_celsius(tenths_kelvin: u64) -> Option<u8> {
    let tenths = i32::try_from(tenths_kelvin).ok()?;
    plausible_celsius((tenths - 2732) * 100)
}

/// ¿Es un Zen (familia 17h o 19h, u otra más nueva con el mismo registro)? `family` es la
/// familia efectiva de `cpuid` (base + extendida).
pub fn amd_family_has_smn_temp(family: u32) -> bool {
    matches!(family, 0x17 | 0x19 | 0x1A)
}

/// La familia efectiva de `cpuid` hoja 1 (EAX): si la base es 0xF, se le suma la extendida.
pub fn cpuid_family(eax: u32) -> u32 {
    let base = (eax >> 8) & 0xF;
    if base == 0xF {
        base + ((eax >> 20) & 0xFF)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tctl_de_amd() {
        // 45 °C: 45 * 8 = 360 octavos en los bits 31:21.
        assert_eq!(amd_tctl_millideg(360 << 21), 45_000);
        // Con la escala desplazada (bit 19): 94 °C crudos son 45 °C.
        assert_eq!(amd_tctl_millideg((94 * 8) << 21 | 1 << 19), 45_000);
        // Octavos: 45,375 °C.
        assert_eq!(amd_tctl_millideg(363 << 21), 45_375);
        // Los bits bajos no cuentan.
        assert_eq!(amd_tctl_millideg(360 << 21 | 0x7FFFF & !(1 << 19)), 45_000);
    }

    #[test]
    fn temperaturas_creibles() {
        assert_eq!(plausible_celsius(45_375), Some(45));
        assert_eq!(plausible_celsius(45_500), Some(46));
        assert_eq!(plausible_celsius(0), None);
        assert_eq!(plausible_celsius(-5_000), None);
        assert_eq!(plausible_celsius(200_000), None);
    }

    #[test]
    fn tmp_de_acpi() {
        // 3032 décimas de kelvin = 30 °C.
        assert_eq!(acpi_tmp_celsius(3032), Some(30));
        assert_eq!(acpi_tmp_celsius(3182), Some(45));
        // Cero kelvin o valores sin sentido: se descartan.
        assert_eq!(acpi_tmp_celsius(0), None);
        assert_eq!(acpi_tmp_celsius(2732), None);
        assert_eq!(acpi_tmp_celsius(u64::MAX), None);
    }

    #[test]
    fn familias() {
        // Ryzen 5 5600GT (Cezanne): familia 0xF + 0xA = 0x19.
        assert_eq!(cpuid_family(0x00A5_0F00), 0x19);
        assert!(amd_family_has_smn_temp(0x19));
        // Ryzen 1000/2000/3000: 0x17.
        assert_eq!(cpuid_family(0x0080_0F11), 0x17);
        // Intel (familia 6) y los AMD viejos (0x10) no.
        assert_eq!(cpuid_family(0x0009_06EA), 6);
        assert!(!amd_family_has_smn_temp(6));
        assert!(!amd_family_has_smn_temp(0x10));
    }
}
