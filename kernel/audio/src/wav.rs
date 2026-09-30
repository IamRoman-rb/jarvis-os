//! WAV: el contenedor RIFF/WAVE de Microsoft e IBM.
//!
//! Un RIFF es una lista de *chunks* (4 letras, largo de 32 bits, datos, relleno a par). Un WAV
//! tiene al menos `fmt ` (cómo están las muestras: formato, canales, frecuencia, bits) y `data`
//! (las muestras). Lo demás (`LIST` con el título, `fact`…) se saltea. El mismo formato de
//! `fmt ` (un WAVEFORMATEX) aparece adentro de los AVI para describir su audio.

use alloc::vec::Vec;

use crate::resample::Decoder;
use crate::{Error, adpcm, u16_at, u32_at};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// PCM con 8 (sin signo), 16 o 24 bits por muestra.
    Pcm {
        bits: u16,
    },
    ImaAdpcm {
        block_align: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub codec: Codec,
    pub channels: usize,
    pub rate: u32,
}

impl Format {
    /// Lee un WAVEFORMATEX (el cuerpo de un chunk `fmt `).
    pub fn parse(fmt: &[u8]) -> Result<Format, Error> {
        let mut tag = u16_at(fmt, 0)?;
        let channels = u16_at(fmt, 2)? as usize;
        let rate = u32_at(fmt, 4)?;
        let block_align = u16_at(fmt, 12)? as usize;
        let bits = u16_at(fmt, 14)?;
        if tag == 0xFFFE {
            // WAVE_FORMAT_EXTENSIBLE: el formato de verdad son los primeros 2 bytes del GUID.
            tag = u16_at(fmt, 24)?;
        }
        if channels == 0 || channels > 8 || !(1000..=192_000).contains(&rate) {
            return Err(Error::Invalid("canales o frecuencia imposibles"));
        }
        let codec = match (tag, bits) {
            (1, 8 | 16 | 24) => Codec::Pcm { bits },
            (1, _) => return Err(Error::Unsupported("PCM de esa cantidad de bits")),
            (0x11, 4) if channels <= 2 && block_align >= 4 * channels => {
                Codec::ImaAdpcm { block_align }
            }
            (3, _) => return Err(Error::Unsupported("muestras de punto flotante")),
            (0x55, _) => return Err(Error::Unsupported("MP3 (usa punto flotante)")),
            _ => return Err(Error::Unsupported("códec de audio desconocido")),
        };
        Ok(Format {
            codec,
            channels,
            rate,
        })
    }

    /// Bytes por cuadro de PCM (todos los canales).
    fn pcm_frame(&self) -> usize {
        match self.codec {
            Codec::Pcm { bits } => bits as usize / 8 * self.channels,
            Codec::ImaAdpcm { .. } => 0,
        }
    }

    /// Cuadros (muestras por canal) que ocupan `bytes` de datos.
    pub fn frames_in(&self, bytes: usize) -> u64 {
        match self.codec {
            Codec::Pcm { .. } => (bytes / self.pcm_frame().max(1)) as u64,
            Codec::ImaAdpcm { block_align } => {
                let per = adpcm::samples_per_block(block_align, self.channels);
                (bytes / block_align * per) as u64
            }
        }
    }

    /// Decodifica `data` (entero) y agrega las muestras a `out`.
    pub fn decode(&self, data: &[u8], out: &mut Vec<i16>) -> Result<(), Error> {
        match self.codec {
            Codec::Pcm { bits: 8 } => out.extend(data.iter().map(|&b| ((b as i16) - 128) << 8)),
            Codec::Pcm { bits: 16 } => out.extend(
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|s| i16::from_le_bytes(*s)),
            ),
            Codec::Pcm { .. } => out.extend(
                // 24 bits: se quedan los 16 de arriba.
                data.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|s| i16::from_le_bytes([s[1], s[2]])),
            ),
            Codec::ImaAdpcm { block_align } => {
                for block in data.chunks(block_align) {
                    // Un último bloque cortado no se decodifica.
                    if block.len() == block_align {
                        adpcm::decode_block(block, self.channels, out)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// De a cuántos bytes conviene decodificar (un bloque de ADPCM o ~4096 cuadros de PCM).
    fn chunk_bytes(&self) -> usize {
        match self.codec {
            Codec::Pcm { .. } => self.pcm_frame() * 4096,
            Codec::ImaAdpcm { block_align } => block_align,
        }
    }
}

/// Un WAV leído: su formato y dónde están los datos.
#[derive(Clone, Debug)]
pub struct Wav {
    pub format: Format,
    data: (usize, usize),
    bytes: Vec<u8>,
    pos: usize,
}

impl Wav {
    /// Lee un archivo WAV entero (se queda con los bytes: se decodifica de a pedazos).
    pub fn parse(bytes: Vec<u8>) -> Result<Wav, Error> {
        if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
            return Err(Error::Invalid("no es un WAV"));
        }
        let mut at = 12;
        let mut format = None;
        let mut data = None;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = u32_at(&bytes, at + 4)? as usize;
            let body = at + 8;
            // Un "data" más largo que el archivo (grabaciones cortadas): hasta donde haya.
            let end = body.saturating_add(len).min(bytes.len());
            match id {
                b"fmt " => format = Some(Format::parse(&bytes[body..end])?),
                b"data" => {
                    data = Some((body, end));
                    break;
                }
                _ => {}
            }
            at = body + len + (len & 1);
        }
        let format = format.ok_or(Error::Invalid("falta el chunk fmt"))?;
        let data = data.ok_or(Error::Invalid("falta el chunk data"))?;
        Ok(Wav {
            format,
            data,
            bytes,
            pos: data.0,
        })
    }

    /// Cuánto dura, en cuadros.
    pub fn frames(&self) -> u64 {
        self.format.frames_in(self.data.1 - self.data.0)
    }
}

impl Decoder for Wav {
    fn rate(&self) -> u32 {
        self.format.rate
    }
    fn channels(&self) -> usize {
        self.format.channels
    }
    fn next_chunk(&mut self, out: &mut Vec<i16>) {
        let end = (self.pos + self.format.chunk_bytes()).min(self.data.1);
        if self.pos < end {
            // Un bloque dañado se saltea (sale silencio en su lugar: nada).
            let _ = self.format.decode(&self.bytes[self.pos..end], out);
            self.pos = end;
        }
    }
}

/// Arma un WAV (lo usan los tests y las herramientas que generan los paquetes).
pub fn write(format: Format, data: &[u8]) -> Vec<u8> {
    let (tag, bits, block_align, extra): (u16, u16, usize, &[u8]) = match format.codec {
        Codec::Pcm { bits } => (1, bits, bits as usize / 8 * format.channels, &[]),
        Codec::ImaAdpcm { block_align } => (0x11, 4, block_align, &[2, 0, 0, 0]),
    };
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&(format.channels as u16).to_le_bytes());
    fmt.extend_from_slice(&format.rate.to_le_bytes());
    let per = match format.codec {
        Codec::Pcm { .. } => 1,
        Codec::ImaAdpcm { block_align } => {
            adpcm::samples_per_block(block_align, format.channels) as u32
        }
    };
    let byte_rate = format.rate / per * block_align as u32;
    fmt.extend_from_slice(&byte_rate.to_le_bytes());
    fmt.extend_from_slice(&(block_align as u16).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if !extra.is_empty() {
        // cbSize = 2 y las muestras por bloque.
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&(per as u16).to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn all(mut w: Wav) -> Vec<i16> {
        let mut out = Vec::new();
        loop {
            let before = out.len();
            w.next_chunk(&mut out);
            if out.len() == before {
                return out;
            }
        }
    }

    #[test]
    fn pcm_de_8_16_y_24_bits() {
        let f16 = Format {
            codec: Codec::Pcm { bits: 16 },
            channels: 2,
            rate: 44_100,
        };
        let samples: Vec<i16> = (0..10_000)
            .map(|i| (i * 7 % 30_000) as i16 - 15_000)
            .collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let w = Wav::parse(write(f16, &bytes)).unwrap();
        assert_eq!(w.format, f16);
        assert_eq!(w.frames(), 5000);
        assert_eq!(all(w), samples);

        let f8 = Format {
            codec: Codec::Pcm { bits: 8 },
            channels: 1,
            rate: 8000,
        };
        let w = Wav::parse(write(f8, &[0, 128, 255])).unwrap();
        assert_eq!(all(w), [-32768, 0, 32512]);

        let f24 = Format {
            codec: Codec::Pcm { bits: 24 },
            channels: 1,
            rate: 48_000,
        };
        let w = Wav::parse(write(f24, &[0xAA, 0x34, 0x12, 0x00, 0x00, 0x80])).unwrap();
        assert_eq!(all(w), [0x1234, -32768]);
    }

    #[test]
    fn adpcm_en_wav() {
        let samples: Vec<i16> = (0..3000i32)
            .map(|i| (((i % 200) - 100).abs() * 300 - 15000) as i16)
            .collect();
        let enc = adpcm::encode(&samples, 1, 256);
        let f = Format {
            codec: Codec::ImaAdpcm { block_align: 256 },
            channels: 1,
            rate: 22_050,
        };
        let w = Wav::parse(write(f, &enc)).unwrap();
        assert_eq!(w.format, f);
        assert_eq!(w.frames(), 505 * 6);
        let out = all(w);
        let err: i64 = samples
            .iter()
            .zip(&out)
            .map(|(a, b)| (*a as i64 - *b as i64).abs())
            .sum::<i64>()
            / 3000;
        assert!(err < 300, "error medio {err}");
    }

    #[test]
    fn archivos_raros() {
        assert!(Wav::parse(b"RIFF\0\0\0\0AVI ".to_vec()).is_err());
        assert!(Wav::parse(vec![0; 3]).is_err());
        // Punto flotante y MP3: se dice por qué no.
        let mut fmt = vec![0u8; 16];
        fmt[0] = 3;
        fmt[2] = 1;
        fmt[4..8].copy_from_slice(&44_100u32.to_le_bytes());
        fmt[14] = 32;
        assert_eq!(
            Format::parse(&fmt),
            Err(Error::Unsupported("muestras de punto flotante"))
        );
        fmt[0] = 0x55;
        assert!(matches!(Format::parse(&fmt), Err(Error::Unsupported(_))));
        // Un chunk desconocido antes de fmt se saltea; un data cortado se lee hasta donde hay.
        let f = Format {
            codec: Codec::Pcm { bits: 16 },
            channels: 1,
            rate: 8000,
        };
        let base = write(f, &[1, 0, 2, 0, 3, 0]);
        let mut file = base[..12].to_vec();
        file.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        file.extend_from_slice(&base[12..]);
        file.truncate(file.len() - 2);
        let w = Wav::parse(file).unwrap();
        assert_eq!(all(w), [1, 2]);
    }
}
