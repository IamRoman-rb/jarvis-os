//! DEFLATE (RFC 1951) dentro de zlib (RFC 1950): lo que comprime los datos de un PNG.
//!
//! DEFLATE combina dos ideas:
//! - **LZ77**: en vez de repetir bytes, "copiá `largo` bytes de `distancia` atrás".
//! - **Huffman**: los símbolos (bytes sueltos, largos, distancias) se escriben con códigos de
//!   largo variable; los frecuentes, cortos. Un bloque trae sus propias tablas ("dinámicas"),
//!   usa unas fijas de la norma, o va sin comprimir ("stored").
//!
//! Los códigos se escriben desde el bit menos significativo de cada byte, pero cada código de
//! Huffman va con su bit más significativo primero: por eso la tabla rápida indexa con los bits
//! **invertidos**. Los códigos de hasta [`FAST_BITS`] bits se resuelven con una sola consulta;
//! los más largos (raros) se decodifican bit a bit como en `puff.c`.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;

/// Bits que resuelve la tabla rápida (512 entradas por tabla).
const FAST_BITS: u32 = 9;
const MAX_BITS: usize = 15;

/// Descomprime un flujo zlib (cabecera de 2 bytes, DEFLATE y Adler-32 al final). Falla con
/// [`Error::TooLarge`] si la salida pasaría de `limit` bytes (una "bomba" de compresión).
pub fn zlib_decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    let (&cmf, &flg) = (
        data.first().ok_or(Error::Truncated)?,
        data.get(1).ok_or(Error::Truncated)?,
    );
    if cmf & 0x0F != 8 || cmf >> 4 > 7 {
        return Err(Error::Invalid("zlib: método de compresión desconocido"));
    }
    if (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(Error::Invalid("zlib: cabecera dañada"));
    }
    if flg & 0x20 != 0 {
        return Err(Error::Unsupported("zlib: diccionario predefinido"));
    }
    let mut bits = Bits::new(&data[2..]);
    let out = inflate(&mut bits, limit)?;
    // Adler-32: justo después del último bloque, alineado a byte, en big endian.
    let pos = 2 + bits.byte_pos();
    let stored = data.get(pos..pos + 4).ok_or(Error::Truncated)?;
    if u32::from_be_bytes([stored[0], stored[1], stored[2], stored[3]]) != adler32(&out) {
        return Err(Error::Checksum);
    }
    Ok(out)
}

/// Descomprime DEFLATE "crudo" (sin la envoltura de zlib).
pub fn inflate_raw(data: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    inflate(&mut Bits::new(data), limit)
}

pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 es el máximo de bytes que se pueden sumar sin que `b` desborde 32 bits.
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    b << 16 | a
}

/// Lector de bits: de a bytes, empezando por el bit menos significativo.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    fn refill(&mut self) {
        while self.count <= 56 {
            let Some(&b) = self.data.get(self.pos) else {
                return;
            };
            self.buf |= u64::from(b) << self.count;
            self.count += 8;
            self.pos += 1;
        }
    }

    /// Mira `n` bits sin consumirlos (los que falten al final de los datos valen 0).
    fn peek(&mut self, n: u32) -> u32 {
        if self.count < n {
            self.refill();
        }
        (self.buf & ((1u64 << n) - 1)) as u32
    }

    fn consume(&mut self, n: u32) -> Result<(), Error> {
        if self.count < n {
            return Err(Error::Truncated);
        }
        self.buf >>= n;
        self.count -= n;
        Ok(())
    }

    fn get(&mut self, n: u32) -> Result<u32, Error> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.peek(n);
        self.consume(n)?;
        Ok(v)
    }

    /// Descarta los bits hasta el próximo byte (antes de un bloque sin comprimir).
    fn align(&mut self) {
        let drop = self.count % 8;
        self.buf >>= drop;
        self.count -= drop;
    }

    /// Cuántos bytes de la entrada se usaron de verdad (sin contar los leídos por adelantado).
    fn byte_pos(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }
}

/// Una tabla de Huffman canónica: cuántos códigos hay de cada largo y los símbolos en orden.
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
    /// Entrada = símbolo << 4 | largo; 0 = el código es más largo que `FAST_BITS`.
    fast: Vec<u16>,
}

impl Huffman {
    /// Arma la tabla desde el largo de código de cada símbolo (0 = el símbolo no aparece).
    fn new(lengths: &[u8]) -> Result<Huffman, Error> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // Un código sobre-suscripto (más códigos que los que entran en esos bits) es inválido.
        // Uno incompleto se acepta: DEFLATE lo permite cuando hay un solo código de distancia.
        let mut left: i32 = 1;
        for &c in &counts[1..] {
            left = left * 2 - i32::from(c);
            if left < 0 {
                return Err(Error::Invalid("deflate: código de Huffman sobre-suscripto"));
            }
        }
        let mut offs = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        // La tabla rápida: el código canónico de cada símbolo, invertido, repetido en todas las
        // entradas cuyos bits de más arriba no importan.
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut code: u32 = 0;
        let mut index = 0usize;
        for len in 1..=MAX_BITS as u32 {
            for _ in 0..counts[len as usize] {
                if len <= FAST_BITS {
                    let rev = code.reverse_bits() >> (32 - len);
                    let entry = symbols[index] << 4 | len as u16;
                    let mut i = rev;
                    while i < (1 << FAST_BITS) {
                        fast[i as usize] = entry;
                        i += 1 << len;
                    }
                }
                code += 1;
                index += 1;
            }
            code <<= 1;
        }
        Ok(Huffman {
            counts,
            symbols,
            fast,
        })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        let entry = self.fast[bits.peek(FAST_BITS) as usize];
        if entry != 0 {
            bits.consume(u32::from(entry & 15))?;
            return Ok(entry >> 4);
        }
        // Camino lento (puff.c): bit a bit, comparando con el primer código de cada largo.
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= bits.get(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Invalid("deflate: código de Huffman inexistente"))
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// En qué orden vienen los largos del código de los largos (sí: hay un Huffman para describir
/// el Huffman).
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn inflate(bits: &mut Bits<'_>, limit: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    loop {
        let last = bits.get(1)? == 1;
        match bits.get(2)? {
            0 => stored(bits, &mut out, limit)?,
            1 => {
                let (lit, dist) = fixed_tables()?;
                codes(bits, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(bits)?;
                codes(bits, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err(Error::Invalid("deflate: tipo de bloque 3")),
        }
        if last {
            bits.align();
            return Ok(out);
        }
    }
}

fn stored(bits: &mut Bits<'_>, out: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    bits.align();
    let len = bits.get(16)?;
    let nlen = bits.get(16)?;
    if len != !nlen & 0xFFFF {
        return Err(Error::Invalid(
            "deflate: largo de bloque sin comprimir dañado",
        ));
    }
    if out.len() + len as usize > limit {
        return Err(Error::TooLarge);
    }
    for _ in 0..len {
        out.push(bits.get(8)? as u8);
    }
    Ok(())
}

fn fixed_tables() -> Result<(Huffman, Huffman), Error> {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    Ok((Huffman::new(&l)?, Huffman::new(&[5u8; 30])?))
}

fn dynamic_tables(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), Error> {
    let nlen = bits.get(5)? as usize + 257;
    let ndist = bits.get(5)? as usize + 1;
    let ncode = bits.get(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(Error::Invalid("deflate: demasiados códigos"));
    }
    let mut cl = [0u8; 19];
    for &i in &CL_ORDER[..ncode] {
        cl[i] = bits.get(3)? as u8;
    }
    let clh = Huffman::new(&cl)?;
    let mut lengths = vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = clh.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                let prev = *lengths[..i]
                    .last()
                    .ok_or(Error::Invalid("deflate: repetición sin largo anterior"))?;
                (prev, 3 + bits.get(2)? as usize)
            }
            17 => (0, 3 + bits.get(3)? as usize),
            _ => (0, 11 + bits.get(7)? as usize),
        };
        if i + repeat > nlen + ndist {
            return Err(Error::Invalid("deflate: demasiados largos"));
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(Error::Invalid("deflate: falta el código de fin de bloque"));
    }
    Ok((
        Huffman::new(&lengths[..nlen])?,
        Huffman::new(&lengths[nlen..])?,
    ))
}

fn codes(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    limit: usize,
) -> Result<(), Error> {
    loop {
        let sym = lit.decode(bits)?;
        match sym {
            0..=255 => {
                if out.len() >= limit {
                    return Err(Error::TooLarge);
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            _ => {
                let s = usize::from(sym - 257);
                if s >= 29 {
                    return Err(Error::Invalid("deflate: largo inválido"));
                }
                let len = usize::from(LEN_BASE[s]) + bits.get(u32::from(LEN_EXTRA[s]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                if d >= 30 {
                    return Err(Error::Invalid("deflate: distancia inválida"));
                }
                let back = usize::from(DIST_BASE[d]) + bits.get(u32::from(DIST_EXTRA[d]))? as usize;
                if back > out.len() {
                    return Err(Error::Invalid("deflate: distancia antes del principio"));
                }
                if out.len() + len > limit {
                    return Err(Error::TooLarge);
                }
                // Byte a byte: la copia puede pisarse a sí misma (distancia < largo repite).
                let start = out.len() - back;
                for k in 0..len {
                    out.push(out[start + k]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloque_sin_comprimir() {
        // zlib: 78 01, bloque final stored con "hola", Adler-32.
        let mut z = alloc::vec![0x78, 0x01, 0x01, 4, 0, !4, 0xFF];
        z.extend_from_slice(b"hola");
        z.extend_from_slice(&adler32(b"hola").to_be_bytes());
        assert_eq!(zlib_decompress(&z, 100).unwrap(), b"hola");
        // Con el Adler cambiado, error.
        let n = z.len();
        z[n - 1] ^= 1;
        assert_eq!(zlib_decompress(&z, 100), Err(Error::Checksum));
    }

    #[test]
    fn ida_y_vuelta_contra_miniz() {
        // Datos con de todo: repeticiones largas y cortas, ruido, y un poco de texto.
        let mut data = Vec::new();
        let mut x: u32 = 12345;
        for i in 0..200_000u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            data.push(match i % 7000 {
                0..=2999 => (x >> 16) as u8,
                3000..=4999 => (i % 13) as u8,
                _ => b"la esfera pulsa "[(i % 16) as usize],
            });
        }
        // Nivel 0: bloques sin comprimir; 1: códigos fijos y dinámicos; 10: el máximo.
        for level in [0u8, 1, 6, 10] {
            let z = miniz_oxide::deflate::compress_to_vec_zlib(&data, level);
            assert_eq!(
                zlib_decompress(&z, data.len()).as_deref(),
                Ok(&data[..]),
                "nivel {level}"
            );
            assert_eq!(zlib_decompress(&z, data.len() - 1), Err(Error::TooLarge));
            assert!(zlib_decompress(&z[..z.len() / 2], data.len()).is_err());
        }
        // Algo corto sale con Huffman fijo.
        let z = miniz_oxide::deflate::compress_to_vec_zlib(b"abcabcabcabc", 9);
        assert_eq!(
            zlib_decompress(&z, 100).as_deref(),
            Ok(&b"abcabcabcabc"[..])
        );
    }

    #[test]
    fn adler_conocido() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
