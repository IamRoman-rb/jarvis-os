//! Lo que viaja por el relé.
//!
//! - **Marco**: 4 bytes de largo (big endian) + contenido. El primero que manda cada máquina es
//!   `JSR1` + el id de grupo (16 bytes); el relé reenvía los siguientes a las otras máquinas del
//!   mismo grupo, sin entenderlos.
//! - **Sobre**: nonce (id de máquina, 4 bytes + contador, 8 bytes) + el mensaje cifrado con
//!   ChaCha20-Poly1305. Un contador que no crece (una repetición) se descarta.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::pair::Group;

pub const HELLO_MAGIC: &[u8; 4] = b"JSR1";
/// Un archivo de hasta 8 MiB más la cabecera.
pub const MAX_FRAME: usize = 9 * 1024 * 1024;

/// El marco de un contenido.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + payload.len());
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// El saludo al relé.
pub fn hello(group: &Group) -> Vec<u8> {
    let mut p = Vec::from(&HELLO_MAGIC[..]);
    p.extend_from_slice(&group.id);
    frame(&p)
}

/// Un marco más largo que [`MAX_FRAME`]: la conexión está rota o no es un relé.
#[derive(Debug, PartialEq, Eq)]
pub struct TooLong;

/// Junta los bytes que llegan en marcos completos.
#[derive(Default)]
pub struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// El próximo marco, `Ok(None)` si falta, o `Err` si el largo no es razonable.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, TooLong> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME {
            return Err(TooLong);
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Cuántos mensajes se reservan de una vez (ver [`reserve`]).
pub const BLOCK: u64 = 1 << 32;

/// Los contadores que puede usar una sesión nueva: `(start, limit)`, para
/// [`Sealer::new`]. Los usados van de `start + 1` a `limit`.
///
/// Por qué así: con la misma clave, repetir un nonce de ChaCha20-Poly1305 deja ver el XOR de
/// los dos mensajes y permite falsificar otros. Toda máquina de un grupo comparte la clave, así
/// que cada una tiene que usar contadores que nunca repitió, *también después de reiniciarse*.
/// La hora (`hint`) no alcanza: puede faltar o atrasar. Por eso el llamador guarda `limit` en
/// el disco **antes** de cifrar nada y, en el próximo arranque, lo pasa como `floor`: aunque
/// la hora sea 0, la sesión nueva empieza después de todo lo que pudo usar la anterior.
pub fn reserve(hint: u64, floor: u64) -> (u64, u64) {
    let start = hint.max(floor);
    (start, start.saturating_add(BLOCK))
}

/// Cifra y descifra los mensajes de un grupo.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    machine: u32,
    counter: u64,
    limit: u64,
    seen: BTreeMap<u32, u64>,
}

impl Sealer {
    /// `(start, limit)` sale de [`reserve`]: cifra con los contadores `start + 1 ..= limit` y,
    /// después, se niega. La otra máquina descarta los contadores que ya vio.
    pub fn new(group: &Group, machine: u32, start: u64, limit: u64) -> Sealer {
        Sealer {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&group.key)),
            machine,
            counter: start,
            limit,
            seen: BTreeMap::new(),
        }
    }

    /// El mensaje cifrado, o `None` si se acabaron los contadores reservados (hay que
    /// reservar otro bloque: nunca se cifra con un contador fuera de la reserva).
    pub fn seal(&mut self, plain: &[u8]) -> Option<Vec<u8>> {
        if self.counter >= self.limit {
            return None;
        }
        self.counter += 1;
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.machine.to_le_bytes());
        nonce[4..].copy_from_slice(&self.counter.to_le_bytes());
        let ct = self.cipher.encrypt(Nonce::from_slice(&nonce), plain).ok()?;
        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Some(out)
    }

    /// El mensaje en claro, o `None` si no es de este grupo, fue alterado o es una repetición.
    pub fn open(&mut self, msg: &[u8]) -> Option<Vec<u8>> {
        if msg.len() < 12 + 16 {
            return None;
        }
        let machine = u32::from_le_bytes(msg[..4].try_into().ok()?);
        let counter = u64::from_le_bytes(msg[4..12].try_into().ok()?);
        if machine == self.machine || self.seen.get(&machine).is_some_and(|&c| counter <= c) {
            return None;
        }
        let plain = self
            .cipher
            .decrypt(Nonce::from_slice(&msg[..12]), &msg[12..])
            .ok()?;
        self.seen.insert(machine, counter);
        Some(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> Group {
        Group::from_code("ABCDE-FGHJK-LMNPQ-RSTUV").unwrap()
    }

    #[test]
    fn marcos() {
        let mut d = Deframer::default();
        let a = frame(b"hola");
        let b = frame(b"");
        d.push(&a[..3]);
        assert_eq!(d.next_frame(), Ok(None));
        d.push(&a[3..]);
        d.push(&b);
        assert_eq!(d.next_frame(), Ok(Some(b"hola".to_vec())));
        assert_eq!(d.next_frame(), Ok(Some(Vec::new())));
        assert_eq!(d.next_frame(), Ok(None));
        d.push(&[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(d.next_frame(), Err(TooLong));
    }

    fn sealer(machine: u32, start: u64) -> Sealer {
        Sealer::new(&group(), machine, start, start + BLOCK)
    }

    #[test]
    fn cifrado_autenticado_y_sin_repeticiones() {
        let mut a = sealer(1, 100);
        let mut b = sealer(2, 0);
        let m1 = a.seal(b"archivo").unwrap();
        let m2 = a.seal(b"otro").unwrap();
        assert!(!m1.windows(7).any(|w| w == b"archivo"), "va cifrado");
        assert_eq!(b.open(&m1).as_deref(), Some(&b"archivo"[..]));
        assert_eq!(b.open(&m1), None, "repetido");
        let mut bad = m2.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(b.open(&bad), None, "alterado");
        assert_eq!(b.open(&m2).as_deref(), Some(&b"otro"[..]));
        let own = a.seal(b"x").unwrap();
        assert_eq!(a.open(&own), None, "los propios se ignoran");
        let other = Group::from_code("ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZ").unwrap();
        let mut stranger = Sealer::new(&other, 3, 0, BLOCK);
        assert_eq!(b.open(&stranger.seal(b"hola").unwrap()), None, "otro grupo");
    }

    fn counter(msg: &[u8]) -> u64 {
        u64::from_le_bytes(msg[4..12].try_into().unwrap())
    }

    #[test]
    fn no_cifra_fuera_de_la_reserva() {
        let mut a = Sealer::new(&group(), 1, 10, 12);
        assert_eq!(counter(&a.seal(b"a").unwrap()), 11);
        assert_eq!(counter(&a.seal(b"b").unwrap()), 12);
        assert_eq!(a.seal(b"c"), None, "el 13 no estaba reservado");
        assert_eq!(a.seal(b"c"), None);
    }

    #[test]
    fn despues_de_reiniciar_sin_hora_los_contadores_siguen_creciendo() {
        // Primera sesión, con hora: reserva y gasta algunos.
        let (start, limit) = reserve(5_000_000, 0);
        let mut before = Sealer::new(&group(), 1, start, limit);
        let last = (0..3)
            .map(|_| counter(&before.seal(b"x").unwrap()))
            .last()
            .unwrap();
        // Se reinicia sin reloj (hint = 0): manda la reserva guardada en el disco.
        let (start2, limit2) = reserve(0, limit);
        let mut after = Sealer::new(&group(), 1, start2, limit2);
        let first = counter(&after.seal(b"y").unwrap());
        assert!(
            first > limit && first > last,
            "{first} tiene que pasar {limit}"
        );
        // Y la otra máquina, que vio la sesión anterior, lo acepta.
        let mut peer = sealer(2, 0);
        assert!(peer.open(&before.seal(b"z").unwrap()).is_some());
        assert!(peer.open(&after.seal(b"w").unwrap()).is_some());
        // Una hora más nueva que la reserva también sirve, sin volver atrás.
        assert_eq!(reserve(limit + 7, limit).0, limit + 7);
        assert_eq!(reserve(u64::MAX - 1, 0).1, u64::MAX, "no desborda");
    }
}
