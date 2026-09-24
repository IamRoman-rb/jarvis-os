//! Teclado PS/2: cola de scancodes entre la interrupción y el bucle principal.
//!
//! El manejador de IRQ1 no puede esperar ni tomar locks que el bucle principal tenga tomados (se
//! trabaría para siempre). Por eso se usa una **cola circular sin locks** de un solo productor (la
//! interrupción) y un solo consumidor (el bucle): cada lado solo escribe su propio índice atómico.
//! La traducción de scancode a tecla (`pc-keyboard`) se hace del lado del bucle.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use pc_keyboard::{DecodedKey, HandleControl, PS2Keyboard, ScancodeSet1, layouts};

const QUEUE_SIZE: usize = 128;

static QUEUE: [AtomicU8; QUEUE_SIZE] = [const { AtomicU8::new(0) }; QUEUE_SIZE];
/// Próxima posición a escribir (solo la toca la interrupción).
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Próxima posición a leer (solo la toca el bucle principal).
static TAIL: AtomicUsize = AtomicUsize::new(0);

/// La llama el manejador de IRQ1. Si la cola está llena, la tecla se descarta.
pub fn push_scancode(code: u8) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % QUEUE_SIZE;
    if next == TAIL.load(Ordering::Acquire) {
        return;
    }
    QUEUE[head].store(code, Ordering::Relaxed);
    HEAD.store(next, Ordering::Release); // publica el byte recién escrito
}

fn pop_scancode() -> Option<u8> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let code = QUEUE[tail].load(Ordering::Relaxed);
    TAIL.store((tail + 1) % QUEUE_SIZE, Ordering::Release);
    Some(code)
}

/// Decodificador (vive en el bucle principal). Layout US por ahora: `pc-keyboard` no trae el
/// latinoamericano.
pub struct Keyboard(PS2Keyboard<layouts::Us104Key, ScancodeSet1>);

impl Keyboard {
    pub fn new() -> Self {
        Keyboard(PS2Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,
        ))
    }

    /// Próxima tecla presionada, si hay.
    pub fn next_key(&mut self) -> Option<DecodedKey> {
        while let Some(code) = pop_scancode() {
            if let Ok(Some(event)) = self.0.add_byte(code)
                && let Some(key) = self.0.process_keyevent(event)
            {
                return Some(key);
            }
        }
        None
    }
}
