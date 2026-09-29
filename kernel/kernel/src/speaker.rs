//! Parlante de la PC ("PC speaker"): la salida del canal 2 del PIT conectada a un parlantito.
//!
//! Se programa el canal 2 como generador de onda cuadrada (modo 3) a la frecuencia de la nota y
//! se habilita la salida al parlante con los bits 0 y 1 del puerto 0x61. En QEMU suena si se lo
//! arranca con `-audiodev ... -machine pcspk-audiodev=...` (lo hace `cargo xtask run`).
//! El canal 2 también se usa al arrancar para calibrar el TSC (`pit.rs`), pero eso termina antes.
//! Referencia: <https://wiki.osdev.org/PC_Speaker>.

use x86_64::instructions::port::Port;

use crate::pit::PIT_FREQUENCY_HZ;

/// Hace sonar `hz` (0 = silencio).
pub fn tone(hz: u32) {
    let mut control = Port::<u8>::new(0x61);
    if hz < 20 {
        // SAFETY: 0x61 controla la compuerta del canal 2 y el parlante; apagar los bits 0 y 1
        // solo silencia el parlante.
        unsafe {
            let v = control.read();
            control.write(v & !0x03);
        }
        return;
    }
    let divisor = (PIT_FREQUENCY_HZ / hz).clamp(1, 0xFFFF) as u16;
    // SAFETY: 0x43 es el comando del PIT y 0x42 el dato del canal 2, que no usa nadie más
    // después del arranque; 0x61 conecta el canal 2 al parlante.
    unsafe {
        Port::<u8>::new(0x43).write(0b1011_0110); // canal 2, byte bajo y alto, modo 3 (cuadrada)
        Port::<u8>::new(0x42).write((divisor & 0xFF) as u8);
        Port::<u8>::new(0x42).write((divisor >> 8) as u8);
        let v = control.read();
        control.write(v | 0x03);
    }
}
