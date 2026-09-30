//! Driver de sonido: la placa virtio-sound (especificación virtio 1.2, §5.14), que QEMU agrega
//! con `-device virtio-sound-pci` y conecta al micrófono y a los parlantes del anfitrión.
//!
//! La placa tiene 4 colas: control (0), eventos (1), **reproducción (2)** y **captura (3)**. Por
//! la de control se pregunta qué flujos de audio hay (`PCM_INFO`), se elige uno de entrada y uno
//! de salida y se los configura (`SET_PARAMS`: PCM de 16 bits), se preparan y se arrancan.
//!
//! - **Micrófono** (K7): 4 buffers de 20 ms en la cola de captura; la placa los llena y los
//!   devuelve; el driver mide el nivel y los vuelve a ofrecer.
//! - **Parlantes** (K12): 5 buffers de 20 ms en la cola de reproducción. La placa devuelve cada
//!   uno cuando lo terminó de *consumir*: ese es el reloj del audio. El driver lo vuelve a llenar
//!   con lo que dejó el mezclador (audio.rs) y lo manda de nuevo. Si no hay nada, va silencio.
//!
//! Cada buffer es una cadena de 3 descriptores: cabecera (el número de flujo), datos y estado
//! (este último lo escribe la placa).

use alloc::string::String;
use core::ptr::{read_volatile, write_volatile};

use jarvis_desktop::audio::{MicInfo, level};

use crate::serial_println;
use crate::virtio_modern::{Queue, Transport, dma_page};

const DEVICE_SOUND: u16 = 25;
const CFG_STREAMS: usize = 4;

const R_PCM_INFO: u32 = 0x0100;
const R_PCM_SET_PARAMS: u32 = 0x0101;
const R_PCM_PREPARE: u32 = 0x0102;
const R_PCM_START: u32 = 0x0104;
const S_OK: u32 = 0x8000;

const DIRECTION_OUTPUT: u8 = 0;
const DIRECTION_INPUT: u8 = 1;
const FORMAT_S16: u8 = 5;
/// Las frecuencias de la especificación, por índice.
const RATES: [u32; 14] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    384000,
];
const PCM_INFO_SIZE: usize = 32;
const MIC_BUFFERS: usize = 4;
const OUT_BUFFERS: usize = 5;

pub struct Mic {
    rx: Queue,
    /// Cabeceras (id del flujo) y estados de los buffers de captura: 64 bytes cada uno.
    meta: *mut u8,
    bufs: [*mut u8; MIC_BUFFERS],
    stream: u32,
    period: u32,
    pub info: MicInfo,
    /// Pico que baja de a poco (para el medidor).
    peak_q: u32,
}

// SAFETY: memoria propia del driver, usada por una sola tarea a la vez.
unsafe impl Send for Mic {}

/// La salida de audio (K12).
pub struct Speaker {
    tx: Queue,
    meta: *mut u8,
    bufs: [*mut u8; OUT_BUFFERS],
    /// Cuadros de audio de verdad (no el silencio de relleno) que lleva cada buffer.
    real: [u32; OUT_BUFFERS],
    stream: u32,
    /// Bytes por buffer (20 ms).
    period: u32,
    pub rate: u32,
    /// Cuadros de audio de verdad que la placa ya consumió.
    pub played: u64,
}

// SAFETY: ídem `Mic`: la usa solo la tarea del audio.
unsafe impl Send for Speaker {}

fn w32(p: *mut u8, off: usize, v: u32) {
    // SAFETY: los llamadores escriben dentro de páginas DMA propias (req, meta).
    unsafe { write_volatile(p.add(off) as *mut u32, v) }
}

fn r32(p: *const u8, off: usize) -> u32 {
    // SAFETY: los llamadores leen dentro de páginas DMA propias (resp, meta).
    unsafe { read_volatile(p.add(off) as *const u32) }
}

/// La cola de control y sus dos páginas (pedido y respuesta).
struct Control {
    ctl: Queue,
    req: *mut u8,
    resp: *mut u8,
}

impl Control {
    fn call(&mut self, req_len: usize, resp_len: usize) -> Option<u32> {
        self.ctl.submit(
            0,
            &[
                (self.req, req_len as u32, false),
                (self.resp, resp_len as u32, true),
            ],
        );
        self.ctl.wait_used()?;
        Some(r32(self.resp, 0))
    }

    fn simple(&mut self, code: u32, stream: u32) -> bool {
        w32(self.req, 0, code);
        w32(self.req, 4, stream);
        self.call(8, 4) == Some(S_OK)
    }

    /// Configura el flujo `stream`: `period` bytes por buffer, `buffers` buffers.
    fn set_params(
        &mut self,
        stream: u32,
        period: u32,
        buffers: u32,
        channels: u8,
        rate_idx: u32,
    ) -> bool {
        w32(self.req, 0, R_PCM_SET_PARAMS);
        w32(self.req, 4, stream);
        w32(self.req, 8, period * buffers);
        w32(self.req, 12, period);
        w32(self.req, 16, 0);
        w32(
            self.req,
            20,
            channels as u32 | (FORMAT_S16 as u32) << 8 | rate_idx << 16,
        );
        self.call(24, 4) == Some(S_OK) && self.simple(R_PCM_PREPARE, stream)
    }
}

/// Un flujo de la placa: número, frecuencias que acepta y canales (mínimo, máximo).
struct StreamInfo {
    id: u32,
    rates: u64,
    channels: (u8, u8),
}

/// Busca la placa y arranca la entrada (micrófono) y la salida (parlantes) que tenga.
pub fn init(offset: u64) -> (Option<Mic>, Option<Speaker>) {
    let Some(t) = Transport::init(DEVICE_SOUND, offset) else {
        return (None, None);
    };
    let (Some(ctl), Some(_events), Some(tx), Some(rx)) =
        (t.queue(0), t.queue(1), t.queue(2), t.queue(3))
    else {
        return (None, None);
    };
    t.ready();
    let streams = t.cfg32(CFG_STREAMS).min(8);
    let (Some(req), Some(resp)) = (dma_page(), dma_page()) else {
        return (None, None);
    };
    let mut c = Control { ctl, req, resp };
    // ¿Qué flujos hay? Se elige el primero de entrada y el primero de salida con PCM de 16 bits.
    w32(c.req, 0, R_PCM_INFO);
    w32(c.req, 4, 0);
    w32(c.req, 8, streams);
    w32(c.req, 12, PCM_INFO_SIZE as u32);
    if c.call(16, 4 + streams as usize * PCM_INFO_SIZE) != Some(S_OK) {
        serial_println!("sonido: la placa no contestó PCM_INFO");
        return (None, None);
    }
    let (mut input, mut output) = (None, None);
    for i in 0..streams as usize {
        let base = 4 + i * PCM_INFO_SIZE;
        // SAFETY: la respuesta tiene `streams` entradas de 32 bytes en la página `resp`.
        let (formats, rates, dir, ch_min, ch_max) = unsafe {
            (
                read_volatile(c.resp.add(base + 8) as *const u64),
                read_volatile(c.resp.add(base + 16) as *const u64),
                read_volatile(c.resp.add(base + 24)),
                read_volatile(c.resp.add(base + 25)),
                read_volatile(c.resp.add(base + 26)),
            )
        };
        if formats & (1 << FORMAT_S16) == 0 {
            continue;
        }
        let info = StreamInfo {
            id: i as u32,
            rates,
            channels: (ch_min.max(1), ch_max.max(ch_min).max(1)),
        };
        match dir {
            DIRECTION_INPUT if input.is_none() => input = Some(info),
            DIRECTION_OUTPUT if output.is_none() => output = Some(info),
            _ => {}
        }
    }
    let mic = input.and_then(|s| Mic::start(&mut c, rx, s));
    let speaker = output.and_then(|s| Speaker::start(&mut c, tx, s));
    (mic, speaker)
}

impl Mic {
    fn start(c: &mut Control, rx: Queue, s: StreamInfo) -> Option<Mic> {
        // 16 kHz (lo que usa el reconocimiento de voz) o, si no, la más baja disponible.
        let rate_idx = if s.rates & (1 << 3) != 0 {
            3
        } else {
            s.rates.trailing_zeros().min(RATES.len() as u32 - 1)
        };
        let rate = RATES[rate_idx as usize];
        let channels = s.channels.0;
        let period = (rate * channels as u32 * 2 / 50).min(4096) & !1; // 20 ms
        if !c.set_params(s.id, period, MIC_BUFFERS as u32, channels, rate_idx)
            || !c.simple(R_PCM_START, s.id)
        {
            serial_println!("microfono: la placa no aceptó el formato");
            return None;
        }
        let mut mic = Mic {
            rx,
            meta: dma_page()?,
            bufs: [dma_page()?, dma_page()?, dma_page()?, dma_page()?],
            stream: s.id,
            period,
            info: MicInfo {
                device: String::from("virtio-sound"),
                rate,
                channels,
                ..MicInfo::default()
            },
            peak_q: 0,
        };
        for i in 0..MIC_BUFFERS {
            mic.post(i);
        }
        serial_println!(
            "MICROFONO virtio-sound {} Hz, {} canal(es), flujo {}",
            rate,
            channels,
            s.id
        );
        Some(mic)
    }

    /// Ofrece el buffer `i` a la placa (descriptores 3i, 3i+1, 3i+2).
    fn post(&mut self, i: usize) {
        let hdr = self.meta.wrapping_add(i * 64);
        let status = hdr.wrapping_add(16);
        w32(hdr, 0, self.stream);
        self.rx.submit(
            (i * 3) as u16,
            &[
                (hdr, 4, false),
                (self.bufs[i], self.period, true),
                (status, 8, true),
            ],
        );
    }

    /// Atiende los buffers que la placa devolvió (llamar seguido, en el bucle principal).
    pub fn poll(&mut self) {
        while let Some((head, len)) = self.rx.take_used() {
            let i = (head / 3) as usize;
            if i >= MIC_BUFFERS {
                continue;
            }
            // `len` incluye los 8 bytes del estado, al final.
            let data = (len.saturating_sub(8)).min(self.period) as usize;
            // SAFETY: la placa escribió `data` bytes en el buffer `i` (una página propia).
            let pcm = unsafe { core::slice::from_raw_parts(self.bufs[i], data) };
            let l = level(pcm);
            if !self.info.receiving {
                serial_println!("MICROFONO_DATOS {} bytes", data);
            }
            self.info.receiving = true;
            self.info.level = l;
            // El pico baja ~2 puntos por buffer (100 en ~1 s).
            self.peak_q = self.peak_q.saturating_sub(2).max(l as u32);
            self.info.peak = self.peak_q as u8;
            self.post(i);
        }
    }
}

impl Speaker {
    fn start(c: &mut Control, tx: Queue, s: StreamInfo) -> Option<Speaker> {
        // 48 kHz (lo de las placas de hoy), si no 44,1 kHz, si no la más alta que haya.
        let rate_idx = if s.rates & (1 << 7) != 0 {
            7
        } else if s.rates & (1 << 6) != 0 {
            6
        } else {
            63 - s.rates.leading_zeros().min(63)
        };
        let rate = *RATES.get(rate_idx as usize)?;
        // El mezclador produce estéreo: si la placa no acepta dos canales, no se usa.
        if !(s.channels.0..=s.channels.1).contains(&2) {
            serial_println!("parlantes: la placa no acepta estéreo");
            return None;
        }
        let period = (rate * 2 * 2 / 50).min(4096) & !3; // 20 ms estéreo de 16 bits
        if !c.set_params(s.id, period, OUT_BUFFERS as u32, 2, rate_idx) {
            serial_println!("parlantes: la placa no aceptó el formato");
            return None;
        }
        let mut sp = Speaker {
            tx,
            meta: dma_page()?,
            bufs: [
                dma_page()?,
                dma_page()?,
                dma_page()?,
                dma_page()?,
                dma_page()?,
            ],
            real: [0; OUT_BUFFERS],
            stream: s.id,
            period,
            rate,
            played: 0,
        };
        // Primero se llenan los buffers (con silencio) y después se arranca.
        for i in 0..OUT_BUFFERS {
            sp.post(i, &mut |_| 0);
        }
        if !c.simple(R_PCM_START, s.id) {
            serial_println!("parlantes: la placa no arrancó la salida");
            return None;
        }
        serial_println!(
            "AUDIO_SALIDA virtio-sound {} Hz estéreo, flujo {}",
            rate,
            s.id
        );
        Some(sp)
    }

    /// Llena el buffer `i` con `fill` (devuelve cuántos cuadros de verdad puso; el resto va en
    /// silencio) y lo manda a la placa.
    fn post(&mut self, i: usize, fill: &mut dyn FnMut(&mut [i16]) -> usize) {
        let frames = self.period as usize / 4;
        // SAFETY: el buffer `i` es una página propia de 4096 bytes y la placa no lo tiene
        // (volvió por la cola de usados o todavía no se mandó); `period` ≤ 4096.
        let pcm = unsafe { core::slice::from_raw_parts_mut(self.bufs[i] as *mut i16, frames * 2) };
        let n = fill(pcm).min(frames);
        pcm[n * 2..].fill(0);
        self.real[i] = n as u32;
        let hdr = self.meta.wrapping_add(i * 64);
        let status = hdr.wrapping_add(16);
        w32(hdr, 0, self.stream);
        self.tx.submit(
            (i * 3) as u16,
            &[
                (hdr, 4, false),
                (self.bufs[i], self.period, false),
                (status, 8, true),
            ],
        );
    }

    /// Los buffers que la placa ya consumió se vuelven a llenar con `fill` y se mandan. Devuelve
    /// cuántos se atendieron.
    pub fn refill(&mut self, fill: &mut dyn FnMut(&mut [i16]) -> usize) -> usize {
        let mut n = 0;
        while let Some((head, _)) = self.tx.take_used() {
            let i = (head / 3) as usize;
            if i >= OUT_BUFFERS {
                continue;
            }
            self.played += self.real[i] as u64;
            self.post(i, fill);
            n += 1;
        }
        n
    }
}

impl crate::audio::Output for Speaker {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn poll(&mut self, fill: &mut dyn FnMut(&mut [i16]) -> usize) {
        self.refill(fill);
    }
    fn played(&self) -> u64 {
        self.played
    }
}
