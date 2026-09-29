//! Llevar cualquier audio a lo que pide la placa: una frecuencia fija y dos canales.
//!
//! Remuestrear es preguntarse "¿cuánto vale la onda en este instante?" para instantes que no
//! coinciden con las muestras que hay. Acá se interpola en línea recta entre las dos muestras
//! vecinas, con la posición en punto fijo (16 bits de fracción). Es el método más simple: suena
//! bien para voz y música de 22–48 kHz; subir mucho la frecuencia (8 kHz → 48 kHz) deja un poco
//! de "brillo" metálico que un filtro mejor sacaría.

use alloc::vec::Vec;

/// Algo que produce audio en su formato original (un archivo, la voz que llega por la red).
pub trait Decoder {
    fn rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Agrega a `out` el próximo pedazo, intercalado por canal. Si no agrega nada, terminó
    /// (o, si es una transmisión, no llegó más todavía: ver [`Decoder::finished`]).
    fn next_chunk(&mut self, out: &mut Vec<i16>);
    /// ¿Ya no va a llegar nada más? Un archivo termina cuando se acaba; una transmisión, cuando
    /// lo avisan.
    fn finished(&self) -> bool {
        true
    }
    /// Para las transmisiones: agregar muestras que llegaron (`end`: no llega más).
    fn feed(&mut self, _pcm: &[i16], _end: bool) {}
}

/// Lo que suena en el mezclador: produce cuadros estéreo a la frecuencia de salida.
pub trait Source {
    /// Escribe hasta `out.len() / 2` cuadros (izquierdo, derecho). Devuelve cuántos escribió:
    /// menos que eso quiere decir que por ahora no hay más.
    fn render(&mut self, out: &mut [i16]) -> usize;
    /// ¿Terminó para siempre?
    fn done(&self) -> bool;
    /// Para las transmisiones (la voz): agregar muestras que llegaron.
    fn feed(&mut self, _pcm: &[i16], _end: bool) {}
}

/// Un [`Decoder`] llevado a `rate` Hz estéreo.
pub struct Resampled<D: Decoder> {
    dec: D,
    /// Muestras del decodificador que todavía no se usaron (intercaladas, sus canales).
    pending: Vec<i16>,
    /// Cuadro de `pending` en el que estamos y cuánto de camino al siguiente (Q16).
    pos: usize,
    frac: u32,
    step: u32,
    ended: bool,
}

impl<D: Decoder> Resampled<D> {
    pub fn new(dec: D, rate: u32) -> Self {
        let step = ((dec.rate() as u64) << 16) / rate.max(1) as u64;
        Resampled {
            dec,
            pending: Vec::new(),
            pos: 0,
            frac: 0,
            step: step.max(1) as u32,
            ended: false,
        }
    }

    pub fn decoder_mut(&mut self) -> &mut D {
        &mut self.dec
    }

    /// Cuadro `i` de lo pendiente, como (izquierdo, derecho).
    fn frame(&self, i: usize) -> (i32, i32) {
        let ch = self.dec.channels().max(1);
        let l = self.pending[i * ch] as i32;
        let r = if ch >= 2 {
            self.pending[i * ch + 1] as i32
        } else {
            l
        };
        (l, r)
    }

    /// Asegura que haya al menos dos cuadros desde `pos` (o que se terminó).
    fn refill(&mut self) -> bool {
        let ch = self.dec.channels().max(1);
        loop {
            let have = self.pending.len() / ch;
            if self.pos + 1 < have {
                return true;
            }
            // Se descarta lo ya usado (queda el cuadro actual, para interpolar).
            if self.pos > 0 {
                let keep = self.pos.min(have);
                self.pending.drain(..keep * ch);
                self.pos -= keep;
            }
            let before = self.pending.len();
            self.dec.next_chunk(&mut self.pending);
            if self.pending.len() == before {
                if self.dec.finished() {
                    self.ended = true;
                }
                return false;
            }
        }
    }
}

impl<D: Decoder> Source for Resampled<D> {
    fn render(&mut self, out: &mut [i16]) -> usize {
        let frames = out.len() / 2;
        for n in 0..frames {
            if !self.refill() {
                return n;
            }
            let (l0, r0) = self.frame(self.pos);
            let (l1, r1) = self.frame(self.pos + 1);
            let f = self.frac as i32;
            out[2 * n] = (l0 + (((l1 - l0) * f) >> 16)) as i16;
            out[2 * n + 1] = (r0 + (((r1 - r0) * f) >> 16)) as i16;
            let next = self.frac + self.step;
            self.pos += (next >> 16) as usize;
            self.frac = next & 0xFFFF;
        }
        frames
    }

    fn done(&self) -> bool {
        self.ended
    }

    fn feed(&mut self, pcm: &[i16], end: bool) {
        self.dec.feed(pcm, end);
    }
}

/// Una transmisión: muestras que llegan de a pedazos (la voz de JARVIS desde el cerebro).
pub struct Stream {
    rate: u32,
    channels: usize,
    queued: Vec<i16>,
    end: bool,
}

impl Stream {
    pub fn new(rate: u32, channels: usize) -> Self {
        Stream {
            rate,
            channels: channels.max(1),
            queued: Vec::new(),
            end: false,
        }
    }
}

impl Decoder for Stream {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn next_chunk(&mut self, out: &mut Vec<i16>) {
        out.append(&mut self.queued);
    }
    fn finished(&self) -> bool {
        self.end && self.queued.is_empty()
    }
    fn feed(&mut self, pcm: &[i16], end: bool) {
        self.queued.extend_from_slice(pcm);
        self.end |= end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Un decodificador de prueba: devuelve lo que tiene de a pedazos de `chunk` cuadros.
    struct Fixed {
        rate: u32,
        channels: usize,
        data: Vec<i16>,
        at: usize,
        chunk: usize,
    }

    impl Decoder for Fixed {
        fn rate(&self) -> u32 {
            self.rate
        }
        fn channels(&self) -> usize {
            self.channels
        }
        fn next_chunk(&mut self, out: &mut Vec<i16>) {
            let end = (self.at + self.chunk * self.channels).min(self.data.len());
            out.extend_from_slice(&self.data[self.at..end]);
            self.at = end;
        }
    }

    fn render_all(src: &mut dyn Source) -> Vec<i16> {
        let mut out = Vec::new();
        let mut buf = vec![0i16; 2 * 333];
        loop {
            let n = src.render(&mut buf);
            out.extend_from_slice(&buf[..2 * n]);
            if n < 333 {
                return out;
            }
        }
    }

    #[test]
    fn misma_frecuencia_copia_y_mono_va_a_los_dos_lados() {
        let data: Vec<i16> = (0..1000).collect();
        let mut r = Resampled::new(
            Fixed {
                rate: 48_000,
                channels: 1,
                data: data.clone(),
                at: 0,
                chunk: 77,
            },
            48_000,
        );
        let out = render_all(&mut r);
        // Todos los cuadros menos el último (no hay con quién interpolarlo).
        assert_eq!(out.len(), 999 * 2);
        for (i, f) in out.chunks(2).enumerate() {
            assert_eq!((f[0], f[1]), (i as i16, i as i16));
        }
        assert!(r.done());
    }

    #[test]
    fn subir_y_bajar_la_frecuencia() {
        // Una rampa: interpolada sigue siendo una rampa.
        let data: Vec<i16> = (0..2000)
            .flat_map(|i| [i as i16 * 10, -(i as i16)])
            .collect();
        let mut up = Resampled::new(
            Fixed {
                rate: 24_000,
                channels: 2,
                data: data.clone(),
                at: 0,
                chunk: 100,
            },
            48_000,
        );
        let out = render_all(&mut up);
        assert!((out.len() / 2).abs_diff(3998) <= 2, "{}", out.len() / 2);
        assert_eq!((out[2], out[3]), (5, -1)); // a mitad de camino entre 0 y 10 (y 0 y -1: redondea para abajo)
        assert_eq!((out[4], out[5]), (10, -1));
        let mut down = Resampled::new(
            Fixed {
                rate: 48_000,
                channels: 2,
                data,
                at: 0,
                chunk: 64,
            },
            16_000,
        );
        let out = render_all(&mut down);
        assert!((out.len() / 2).abs_diff(667) <= 1, "{}", out.len() / 2);
        assert_eq!((out[2], out[3]), (30, -3));
    }

    #[test]
    fn una_transmision_espera_lo_que_falta() {
        let mut s = Resampled::new(Stream::new(8000, 1), 8000);
        let mut buf = [0i16; 20];
        assert_eq!(s.render(&mut buf), 0);
        assert!(!s.done(), "no terminó: todavía no llegó nada");
        s.feed(&[1, 2, 3, 4, 5], false);
        assert_eq!(s.render(&mut buf), 4);
        assert_eq!(&buf[..8], &[1, 1, 2, 2, 3, 3, 4, 4]);
        s.feed(&[6], true);
        assert_eq!(s.render(&mut buf), 1);
        assert_eq!(s.render(&mut buf), 0);
        assert!(s.done());
    }
}
