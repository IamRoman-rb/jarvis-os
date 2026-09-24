//! Lectura del reloj de tiempo real (RTC) del chip CMOS.
//!
//! Es el reloj con pila de la placa madre: sigue andando con la máquina apagada. Se accede por
//! dos puertos de E/S: en 0x70 se elige el registro y en 0x71 se lee su valor.

use jarvis_gfx::clock::DateTime;
use x86_64::instructions::port::Port;

const CMOS_ADDRESS: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

fn read_register(reg: u8) -> u8 {
    // SAFETY: 0x70/0x71 son los puertos estándar del CMOS. El bit 7 en 1 deja desactivadas las
    // NMI mientras se lee, como recomienda la documentación del chip.
    unsafe {
        Port::<u8>::new(CMOS_ADDRESS).write(0x80 | reg);
        Port::<u8>::new(CMOS_DATA).read()
    }
}

fn update_in_progress() -> bool {
    read_register(0x0A) & 0x80 != 0
}

fn read_raw() -> [u8; 6] {
    while update_in_progress() {
        core::hint::spin_loop();
    }
    [
        read_register(0x00), // segundos
        read_register(0x02), // minutos
        read_register(0x04), // horas
        read_register(0x07), // día del mes
        read_register(0x08), // mes
        read_register(0x09), // año (dos dígitos)
    ]
}

/// Convierte BCD (cada nibble es un dígito decimal: 0x59 = 59) a binario.
const fn from_bcd(v: u8) -> u8 {
    (v & 0x0F) + (v >> 4) * 10
}

/// Lee fecha y hora en UTC. Devuelve `None` si el reloj da valores imposibles.
pub fn read_utc() -> Option<DateTime> {
    // El reloj puede actualizarse a mitad de la lectura: se lee hasta obtener dos iguales.
    let mut raw = read_raw();
    loop {
        let again = read_raw();
        if again == raw {
            break;
        }
        raw = again;
    }
    let status_b = read_register(0x0B);
    let binary = status_b & 0x04 != 0;
    let hour_24 = status_b & 0x02 != 0;

    let conv = |v: u8| if binary { v } else { from_bcd(v) };
    let pm = raw[2] & 0x80 != 0;
    let mut hour = conv(raw[2] & 0x7F);
    if !hour_24 {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let dt = DateTime {
        year: 2000 + conv(raw[5]) as u16,
        month: conv(raw[4]),
        day: conv(raw[3]),
        hour,
        minute: conv(raw[1]),
        second: conv(raw[0]),
    };
    dt.is_valid().then_some(dt)
}
