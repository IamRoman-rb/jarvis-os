//! Cola de bytes sin locks entre una interrupción (productora) y el bucle principal (consumidor).
//!
//! Un manejador de interrupción no puede esperar un lock que tenga tomado el bucle principal: la
//! CPU no vuelve al bucle hasta que el manejador termina, así que se trabaría para siempre. Con un
//! solo productor y un solo consumidor alcanza con dos índices atómicos: cada lado escribe solo el
//! suyo y lee el del otro.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

pub struct ByteQueue<const N: usize> {
    buf: [AtomicU8; N],
    /// Próxima posición a escribir (solo la toca la interrupción).
    head: AtomicUsize,
    /// Próxima posición a leer (solo la toca el bucle principal).
    tail: AtomicUsize,
}

impl<const N: usize> ByteQueue<N> {
    pub const fn new() -> Self {
        ByteQueue {
            buf: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Desde la interrupción. Si la cola está llena, el byte se descarta.
    pub fn push(&self, byte: u8) {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        if next == self.tail.load(Ordering::Acquire) {
            return;
        }
        self.buf[head].store(byte, Ordering::Relaxed);
        self.head.store(next, Ordering::Release); // publica el byte recién escrito
    }

    /// Desde el bucle principal.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let byte = self.buf[tail].load(Ordering::Relaxed);
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(byte)
    }
}
