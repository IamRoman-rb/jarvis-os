//! PIT (Programmable Interval Timer, chip 8253/8254).
//!
//! El PIT tiene un oscilador de 1.193.182 Hz y tres canales:
//! - **Canal 0** → IRQ0: interrupción periódica. Acá solo sirve para **despertar** al bucle
//!   principal (que duerme con `hlt`). No se usa para medir el tiempo: bajo carga, un emulador
//!   (o un kernel con interrupciones deshabilitadas mucho rato) puede **perder** interrupciones, y
//!   un reloj que las cuenta atrasa. Medido en QEMU sin aceleración, contar 1000 IRQ por segundo
//!   daba un reloj al 68 % de la velocidad real.
//! - **Canal 2**: se usa una sola vez, sin interrupciones, para calibrar el TSC (ver `time.rs`).
//!
//! Referencia: <https://wiki.osdev.org/Programmable_Interval_Timer>.

use x86_64::instructions::port::Port;

pub const PIT_FREQUENCY_HZ: u32 = 1_193_182;
/// Frecuencia de la IRQ0: cada 4 ms se despierta el bucle (alcanza para 60 fps).
pub const WAKEUPS_PER_SECOND: u32 = 250;

pub fn init() {
    let divisor = (PIT_FREQUENCY_HZ / WAKEUPS_PER_SECOND) as u16;
    // SAFETY: 0x43 es el registro de comando del PIT y 0x40 el canal 0, conectado a la IRQ0.
    unsafe {
        // Canal 0, byte bajo y después alto, modo 2 (generador de pulsos periódicos), binario.
        Port::<u8>::new(0x43).write(0b0011_0100);
        Port::<u8>::new(0x40).write((divisor & 0xFF) as u8);
        Port::<u8>::new(0x40).write((divisor >> 8) as u8);
    }
}

/// Espera exactamente `ms` milisegundos usando el canal 2 del PIT por *polling* (sin
/// interrupciones), llamando a `before` justo al arrancar la cuenta. Devuelve lo que devuelve
/// `before` (así el llamador puede leer el TSC en el instante exacto del inicio).
pub fn wait_ms_polling<T>(ms: u32, before: impl FnOnce() -> T) -> T {
    let count = (PIT_FREQUENCY_HZ as u64 * ms as u64 / 1000).min(0xFFFF) as u16;
    let mut control = Port::<u8>::new(0x61);
    let mut command = Port::<u8>::new(0x43);
    let mut channel2 = Port::<u8>::new(0x42);
    // SAFETY: 0x61 controla la compuerta del canal 2 (bit 0) y el parlante (bit 1, que se deja
    // apagado); 0x43/0x42 son el comando y el dato del canal 2. Nada más usa el canal 2.
    unsafe {
        let gate = (control.read() & !0x02) | 0x01;
        control.write(gate);
        // Canal 2, byte bajo y alto, modo 0 (la salida sube al llegar a cero), binario.
        command.write(0b1011_0000);
        channel2.write((count & 0xFF) as u8);
        channel2.write((count >> 8) as u8); // escribir el byte alto arranca la cuenta
        let value = before();
        // El bit 5 de 0x61 refleja la salida del canal 2: sube cuando la cuenta llega a cero.
        while control.read() & 0x20 == 0 {
            core::hint::spin_loop();
        }
        value
    }
}
