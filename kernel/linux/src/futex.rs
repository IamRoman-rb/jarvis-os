//! `futex`: la llamada con la que los hilos de un programa se esperan y se despiertan.
//!
//! Un mutex o una condición de la biblioteca de C (musl, glibc) o de Rust es un entero en la
//! memoria del programa. Mientras nadie compite, se toma y se suelta en el espacio de usuario,
//! sin el kernel. Cuando un hilo tiene que esperar, pide `futex(FUTEX_WAIT, &entero, valor)`:
//! "dormime **si** el entero todavía vale `valor`". Ese "si" se verifica en el kernel sin que
//! nadie pueda cambiar el entero en el medio, y evita perder un aviso que llegó justo antes de
//! dormir. El que suelta pide `futex(FUTEX_WAKE, &entero, n)`: "despertá a `n`".
//!
//! Acá está la tabla de los que esperan, sin hardware: quién espera en qué dirección (de qué
//! espacio de direcciones) y a quién le tocó despertarse. El kernel (kernel/process.rs) pone las
//! esperas de verdad. Referencia: `man 2 futex` y "Futexes Are Tricky" (Ulrich Drepper).

use alloc::vec::Vec;

pub mod op {
    pub const WAIT: u32 = 0;
    pub const WAKE: u32 = 1;
    pub const REQUEUE: u32 = 3;
    pub const CMP_REQUEUE: u32 = 4;
    pub const WAKE_OP: u32 = 5;
    pub const WAIT_BITSET: u32 = 9;
    pub const WAKE_BITSET: u32 = 10;
    /// Bits de opciones que no cambian la operación.
    pub const PRIVATE: u32 = 128;
    pub const CLOCK_REALTIME: u32 = 256;
}

/// Dónde se espera: el espacio de direcciones (la raíz de sus tablas de páginas) y la dirección.
pub type Key = (u64, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiter {
    key: Key,
    /// Quién espera (el número de su tarea).
    who: usize,
    woken: bool,
}

#[derive(Default, Debug)]
pub struct Futexes {
    waiters: Vec<Waiter>,
}

impl Futexes {
    pub const fn new() -> Futexes {
        Futexes {
            waiters: Vec::new(),
        }
    }

    /// `who` empieza a esperar en `key` (ya se comprobó el valor).
    pub fn wait(&mut self, key: Key, who: usize) {
        self.waiters.push(Waiter {
            key,
            who,
            woken: false,
        });
    }

    /// ¿Ya lo despertaron? Si sí, sale de la tabla.
    pub fn take_woken(&mut self, who: usize) -> bool {
        match self.waiters.iter().position(|w| w.who == who && w.woken) {
            Some(i) => {
                self.waiters.remove(i);
                true
            }
            None => false,
        }
    }

    /// `who` deja de esperar (venció el plazo o lo terminaron). Si justo lo habían despertado,
    /// ese aviso se pasa al siguiente que espera lo mismo (si no, se perdería).
    pub fn cancel(&mut self, who: usize) {
        if let Some(i) = self.waiters.iter().position(|w| w.who == who) {
            let w = self.waiters.remove(i);
            if w.woken {
                self.wake(w.key, 1);
            }
        }
    }

    /// Despierta hasta `n` de los que esperan en `key` (los más viejos primero). Devuelve cuántos.
    pub fn wake(&mut self, key: Key, n: usize) -> usize {
        let mut done = 0;
        for w in self.waiters.iter_mut() {
            if done == n {
                break;
            }
            if w.key == key && !w.woken {
                w.woken = true;
                done += 1;
            }
        }
        done
    }

    /// Despierta `n` de `from` y pasa hasta `move_n` de los demás a esperar en `to` (lo usan las
    /// condiciones: despertar a todos de golpe sería que todos peleen por el mutex).
    pub fn requeue(&mut self, from: Key, to: Key, n: usize, move_n: usize) -> usize {
        let woken = self.wake(from, n);
        let mut moved = 0;
        for w in self.waiters.iter_mut() {
            if moved == move_n {
                break;
            }
            if w.key == from && !w.woken {
                w.key = to;
                moved += 1;
            }
        }
        woken + moved
    }

    /// Los que esperan en un espacio que se va (el proceso terminó).
    pub fn forget_space(&mut self, root: u64) {
        self.waiters.retain(|w| w.key.0 != root);
    }

    pub fn len(&self) -> usize {
        self.waiters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

/// `FUTEX_WAKE_OP`: la operación sobre el segundo entero (`val3` codificado como en Linux) y si
/// hay que despertar también a los que esperan en él. Devuelve (valor nuevo, ¿despertar?).
pub fn wake_op(encoded: u32, old: u32) -> (u32, bool) {
    let op = (encoded >> 28) & 7;
    let cmp = (encoded >> 24) & 15;
    let mut oparg = (encoded >> 12) & 0xfff;
    let cmparg = encoded & 0xfff;
    if op & 8 != 0 || (encoded >> 31) & 1 != 0 {
        // FUTEX_OP_OPARG_SHIFT: el argumento es un corrimiento.
        oparg = 1u32.checked_shl(oparg).unwrap_or(0);
    }
    let new = match op & 7 {
        0 => oparg,
        1 => old.wrapping_add(oparg),
        2 => old | oparg,
        3 => old & !oparg,
        4 => old ^ oparg,
        _ => old,
    };
    let wake = match cmp {
        0 => old == cmparg,
        1 => old != cmparg,
        2 => old < cmparg,
        3 => old <= cmparg,
        4 => old > cmparg,
        5 => old >= cmparg,
        _ => false,
    };
    (new, wake)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esperar_y_despertar() {
        let mut f = Futexes::new();
        let k = (0x1000, 0x40_0000);
        f.wait(k, 3);
        f.wait(k, 4);
        f.wait((0x1000, 0x40_0004), 5);
        assert!(!f.take_woken(3));
        assert_eq!(f.wake(k, 1), 1);
        assert!(f.take_woken(3), "el más viejo primero");
        assert!(!f.take_woken(4));
        assert_eq!(f.wake(k, usize::MAX), 1);
        assert!(f.take_woken(4));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn un_aviso_no_se_pierde_si_el_despertado_se_va() {
        let mut f = Futexes::new();
        let k = (1, 8);
        f.wait(k, 1);
        f.wait(k, 2);
        f.wake(k, 1);
        // El 1 venció su plazo justo cuando lo despertaban: el aviso pasa al 2.
        f.cancel(1);
        assert!(f.take_woken(2));
        assert!(f.is_empty());
    }

    #[test]
    fn requeue_mueve_a_los_demas() {
        let mut f = Futexes::new();
        let (cond, mutex) = ((1, 0x10), (1, 0x20));
        for who in 0..4 {
            f.wait(cond, who);
        }
        assert_eq!(f.requeue(cond, mutex, 1, usize::MAX), 4);
        assert!(f.take_woken(0));
        assert_eq!(f.wake(mutex, 1), 1);
        assert!(f.take_woken(1));
        f.forget_space(1);
        assert!(f.is_empty());
    }

    #[test]
    fn wake_op_como_linux() {
        // FUTEX_OP(FUTEX_OP_ADD, 1, FUTEX_OP_CMP_GT, 0): suma 1 y despierta si valía > 0.
        let encoded = (1 << 28) | (4 << 24) | (1 << 12);
        assert_eq!(wake_op(encoded, 5), (6, true));
        assert_eq!(wake_op(encoded, 0), (1, false));
    }
}
