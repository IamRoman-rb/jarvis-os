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

/// Cifra y descifra los mensajes de un grupo.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    machine: u32,
    counter: u64,
    seen: BTreeMap<u32, u64>,
}

impl Sealer {
    /// `counter` tiene que crecer entre reinicios (por ejemplo, la hora en microsegundos):
    /// la otra máquina descarta los contadores que ya vio.
    pub fn new(group: &Group, machine: u32, counter: u64) -> Sealer {
        Sealer {
            cipher: ChaCha20Poly1305::new(Key::from_slice(&group.key)),
            machine,
            counter,
            seen: BTreeMap::new(),
        }
    }

    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        self.counter += 1;
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.machine.to_le_bytes());
        nonce[4..].copy_from_slice(&self.counter.to_le_bytes());
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plain)
            .unwrap_or_default();
        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
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

    #[test]
    fn cifrado_autenticado_y_sin_repeticiones() {
        let mut a = Sealer::new(&group(), 1, 100);
        let mut b = Sealer::new(&group(), 2, 0);
        let m1 = a.seal(b"archivo");
        let m2 = a.seal(b"otro");
        assert!(!m1.windows(7).any(|w| w == b"archivo"), "va cifrado");
        assert_eq!(b.open(&m1).as_deref(), Some(&b"archivo"[..]));
        assert_eq!(b.open(&m1), None, "repetido");
        let mut bad = m2.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(b.open(&bad), None, "alterado");
        assert_eq!(b.open(&m2).as_deref(), Some(&b"otro"[..]));
        let own = a.seal(b"x");
        assert_eq!(a.open(&own), None, "los propios se ignoran");
        let mut stranger = Sealer::new(&Group::from_code("ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZ").unwrap(), 3, 0);
        assert_eq!(b.open(&stranger.seal(b"hola")), None, "otro grupo");
    }
}
