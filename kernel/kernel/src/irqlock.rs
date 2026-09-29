//! Lock para datos que comparten varias tareas (K9).
//!
//! Con un solo núcleo, un `spin::Mutex` común tiene un problema cuando hay desalojo: si la tarea
//! A lo toma y el timer le saca la CPU, la tarea B que lo pida da vueltas todo su turno sin
//! poder avanzar; y si lo pide un manejador de interrupción, se traba para siempre (A no vuelve
//! a correr hasta que el manejador termine). Por eso este lock **deshabilita las interrupciones**
//! mientras está tomado: nadie puede desalojar a quien lo tiene, y la sección crítica tiene que
//! ser corta (copiar una cola, no procesarla).

use spin::Mutex;
use x86_64::instructions::interrupts;

pub struct IrqMutex<T>(Mutex<T>);

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        IrqMutex(Mutex::new(value))
    }

    /// Ejecuta `f` con el dato, sin interrupciones.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        interrupts::without_interrupts(|| f(&mut self.0.lock()))
    }
}
