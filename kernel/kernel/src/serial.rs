//! Driver mínimo del puerto serie COM1 (UART 16550).
//!
//! Es la "consola de depuración" del kernel: QEMU redirige COM1 a la terminal del host
//! (`-serial stdio`), así que todo lo que se escribe acá aparece en tu consola. También es la
//! vía por la que la CI sabe que el kernel arrancó (`JARVIS_BOOT_OK`).

use core::fmt::{self, Write};

use spin::Mutex;
use x86_64::instructions::port::Port;

const COM1: u16 = 0x3F8;

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    /// # Safety
    /// `base` tiene que ser la dirección de E/S de un UART real, y nadie más debe usarlo.
    const unsafe fn new(base: u16) -> Self {
        SerialPort { base }
    }

    fn out(&self, offset: u16, value: u8) {
        // SAFETY: `new` garantiza que `base` es un UART; escribir sus registros no afecta memoria.
        unsafe { Port::new(self.base + offset).write(value) }
    }

    fn inp(&self, offset: u16) -> u8 {
        // SAFETY: ídem `out`; leer el registro de estado no tiene efectos secundarios.
        unsafe { Port::new(self.base + offset).read() }
    }

    /// Configura 38400 baudios, 8 bits, sin paridad, 1 bit de parada (8N1).
    fn init(&self) {
        self.out(1, 0x00); // sin interrupciones (todavía no hay IDT: llega en K1)
        self.out(3, 0x80); // DLAB = 1: los próximos dos registros son el divisor de baudios
        self.out(0, 0x03); // divisor 3 → 115200 / 3 = 38400 baudios
        self.out(1, 0x00);
        self.out(3, 0x03); // DLAB = 0, 8N1
        self.out(2, 0xC7); // FIFO activado y vaciado
        self.out(4, 0x0B); // DTR + RTS + OUT2
    }

    fn send(&self, byte: u8) {
        // Esperar a que el registro de transmisión esté vacío (bit 5 del estado de línea).
        while self.inp(5) & 0x20 == 0 {
            core::hint::spin_loop();
        }
        self.out(0, byte);
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.send(b'\r');
            }
            self.send(byte);
        }
        Ok(())
    }
}

// SAFETY: 0x3F8 es COM1 en todo PC compatible y este es el único lugar que lo usa.
pub static SERIAL: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1) });

pub fn init() {
    SERIAL.lock().init();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    // Si el puerto está tomado (un panic mientras se escribía), se descarta el mensaje
    // antes que colgar el kernel esperando un lock que nunca se libera.
    if let Some(mut port) = SERIAL.try_lock() {
        let _ = port.write_fmt(args);
    }
}

#[macro_export]
macro_rules! serial_println {
    () => { $crate::serial::_print(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::serial::_print(format_args!("{}\n", format_args!($($arg)*))) };
}
