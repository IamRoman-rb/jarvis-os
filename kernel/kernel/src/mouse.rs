//! Mouse PS/2 (puerto auxiliar del controlador 8042).
//!
//! El mismo chip que maneja el teclado tiene un segundo puerto para el mouse. Hay que habilitarlo,
//! activar su interrupción (IRQ12) y pedirle al mouse que empiece a mandar datos. Cada movimiento
//! llega como un paquete de 3 bytes; la interrupción solo encola los bytes y el escritorio arma
//! los paquetes (`jarvis_desktop::MouseDecoder`).
//! Referencia: <https://wiki.osdev.org/PS/2_Mouse> y <https://wiki.osdev.org/I8042_PS/2_Controller>.

use x86_64::instructions::port::Port;

use crate::queue::ByteQueue;

const DATA: u16 = 0x60;
const STATUS_COMMAND: u16 = 0x64;
const ACK: u8 = 0xFA;
/// Vueltas de espera por el controlador antes de rendirse.
const WAIT_LIMIT: u32 = 100_000;

static QUEUE: ByteQueue<512> = ByteQueue::new();

/// La llama el manejador de IRQ12.
pub fn push_byte(byte: u8) {
    QUEUE.push(byte);
}

pub fn pop_byte() -> Option<u8> {
    QUEUE.pop()
}

fn status() -> u8 {
    // SAFETY: 0x64 es el registro de estado del controlador 8042; leerlo no tiene efectos.
    unsafe { Port::new(STATUS_COMMAND).read() }
}

/// Espera a que se cumpla `ready` sobre el registro de estado.
fn wait(ready: impl Fn(u8) -> bool) -> bool {
    for _ in 0..WAIT_LIMIT {
        if ready(status()) {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn command(cmd: u8) {
    // Bit 1 del estado en 0: el controlador puede recibir un byte.
    if wait(|s| s & 0x02 == 0) {
        // SAFETY: comando al controlador 8042 por su puerto de comandos.
        unsafe { Port::new(STATUS_COMMAND).write(cmd) }
    }
}

fn write_data(byte: u8) {
    if wait(|s| s & 0x02 == 0) {
        // SAFETY: dato al controlador 8042 por su puerto de datos.
        unsafe { Port::new(DATA).write(byte) }
    }
}

fn read_data() -> Option<u8> {
    // Bit 0 del estado en 1: hay un byte para leer.
    // SAFETY: se lee el puerto de datos solo cuando el estado indica que hay un byte.
    wait(|s| s & 0x01 != 0).then(|| unsafe { Port::new(DATA).read() })
}

/// Manda un comando al mouse (0xD4 = "el próximo byte es para el puerto auxiliar").
fn mouse_command(byte: u8) -> bool {
    command(0xD4);
    write_data(byte);
    read_data() == Some(ACK)
}

/// Habilita el mouse. Se llama con las interrupciones deshabilitadas. `false` si no responde.
pub fn init() -> bool {
    // Descartar bytes viejos que hayan quedado en el controlador.
    while status() & 0x01 != 0 {
        // SAFETY: hay un byte pendiente en el puerto de datos (bit 0 del estado).
        let _: u8 = unsafe { Port::new(DATA).read() };
    }
    command(0xA8); // habilitar el puerto auxiliar
    command(0x20); // leer el byte de configuración
    let Some(config) = read_data() else {
        return false;
    };
    // Bit 1: interrupción del puerto auxiliar (IRQ12). Bit 5 en 0: reloj del mouse encendido.
    let config = (config | 0x02) & !0x20;
    command(0x60);
    write_data(config);
    mouse_command(0xF6) && mouse_command(0xF4) // valores por defecto + empezar a mandar datos
}
