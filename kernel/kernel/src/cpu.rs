//! Información de la CPU con la instrucción `cpuid`: el nombre comercial del procesador.
//! Referencia: <https://wiki.osdev.org/CPUID>.

use alloc::string::String;
use core::arch::x86_64::__cpuid;

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
