//! Driver del micrófono: la placa virtio-sound (especificación virtio 1.2, §5.14), que QEMU
//! agrega con `-device virtio-sound-pci` y conecta al micrófono del anfitrión.
//!
//! La placa tiene 4 colas: control (0), eventos (1), reproducción (2) y **captura (3)**. Por la
//! de control se pregunta qué flujos de audio hay (`PCM_INFO`), se elige el de entrada y se lo
//! configura (`SET_PARAMS`: PCM de 16 bits, 16 kHz si puede), se prepara y se arranca. Después
//! se le dejan 4 buffers en la cola de captura: la placa los llena con 20 ms de audio cada uno
//! y los devuelve; el driver mide el nivel y los vuelve a ofrecer.
//!
//! Cada buffer es una cadena de 3 descriptores: cabecera (id del flujo, la lee la placa), datos
//! y estado (los escribe la placa).

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

const DIRECTION_INPUT: u8 = 1;
const FORMAT_S16: u8 = 5;
/// Las frecuencias de la especificación, por índice.
const RATES: [u32; 14] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    384000,
];
const PCM_INFO_SIZE: usize = 32;
const BUFFERS: usize = 4;

pub struct Mic {
    ctl: Queue,
    rx: Queue,
    req: *mut u8,
    resp: *mut u8,
    /// Cabeceras (id del flujo) y estados de los buffers de captura: 64 bytes cada uno.
    meta: *mut u8,
    bufs: [*mut u8; BUFFERS],
    stream: u32,
    period: u32,
    pub info: MicInfo,
    /// Pico que baja de a poco (para el medidor).
    peak_q: u32,
}

// SAFETY: memoria propia del driver; el kernel tiene un solo hilo.
unsafe impl Send for Mic {}

fn w32(p: *mut u8, off: usize, v: u32) {
    // SAFETY: los llamadores escriben dentro de páginas DMA propias (req, meta).
    unsafe { write_volatile(p.add(off) as *mut u32, v) }
}

fn r32(p: *const u8, off: usize) -> u32 {
    // SAFETY: los llamadores leen dentro de páginas DMA propias (resp, meta).
    unsafe { read_volatile(p.add(off) as *const u32) }
}

impl Mic {
    /// Busca la placa, configura el flujo de entrada y arranca la captura. `None` si no hay
    /// placa o no tiene entrada de 16 bits.
    pub fn init(offset: u64) -> Option<Mic> {
        let t = Transport::init(DEVICE_SOUND, offset)?;
        let ctl = t.queue(0)?;
        let _events = t.queue(1)?;
        let _tx = t.queue(2)?;
        let rx = t.queue(3)?;
        t.ready();
        let streams = t.cfg32(CFG_STREAMS).min(8);
        let mut mic = Mic {
            ctl,
            rx,
            req: dma_page()?,
            resp: dma_page()?,
            meta: dma_page()?,
            bufs: [dma_page()?, dma_page()?, dma_page()?, dma_page()?],
            stream: 0,
            period: 0,
            info: MicInfo::default(),
            peak_q: 0,
        };
        // ¿Qué flujos hay? Se elige el primero de entrada que acepte PCM de 16 bits.
        w32(mic.req, 0, R_PCM_INFO);
        w32(mic.req, 4, 0);
        w32(mic.req, 8, streams);
        w32(mic.req, 12, PCM_INFO_SIZE as u32);
        if mic.control(16, 4 + streams as usize * PCM_INFO_SIZE) != Some(S_OK) {
            serial_println!("microfono: la placa no contestó PCM_INFO");
            return None;
        }
        let mut chosen = None;
        for i in 0..streams as usize {
            let base = 4 + i * PCM_INFO_SIZE;
            // SAFETY: la respuesta tiene `streams` entradas de 32 bytes en la página `resp`.
            let (formats, rates, dir, ch_min) = unsafe {
                (
                    read_volatile(mic.resp.add(base + 8) as *const u64),
                    read_volatile(mic.resp.add(base + 16) as *const u64),
                    read_volatile(mic.resp.add(base + 24)),
                    read_volatile(mic.resp.add(base + 25)),
                )
            };
            if dir == DIRECTION_INPUT && formats & (1 << FORMAT_S16) != 0 {
                chosen = Some((i as u32, rates, ch_min.max(1)));
                break;
            }
        }
        let Some((stream, rates, channels)) = chosen else {
            serial_println!("microfono: la placa no tiene entrada de 16 bits");
            return None;
        };
        // 16 kHz (lo que usa el reconocimiento de voz) o, si no, la más baja disponible.
        let rate_idx = if rates & (1 << 3) != 0 {
            3
        } else {
            rates.trailing_zeros().min(RATES.len() as u32 - 1)
        };
        let rate = RATES[rate_idx as usize];
        let period = (rate * channels as u32 * 2 / 50).min(4096) & !1; // 20 ms
        w32(mic.req, 0, R_PCM_SET_PARAMS);
        w32(mic.req, 4, stream);
        w32(mic.req, 8, period * BUFFERS as u32);
        w32(mic.req, 12, period);
        w32(mic.req, 16, 0);
        w32(
            mic.req,
            20,
            channels as u32 | (FORMAT_S16 as u32) << 8 | rate_idx << 16,
        );
        let ok = mic.control(24, 4) == Some(S_OK)
            && mic.simple(R_PCM_PREPARE, stream)
            && mic.simple(R_PCM_START, stream);
        if !ok {
            serial_println!("microfono: la placa no aceptó el formato");
            return None;
        }
        mic.stream = stream;
        mic.period = period;
        mic.info = MicInfo {
            device: String::from("virtio-sound"),
            rate,
            channels,
            ..MicInfo::default()
        };
        for i in 0..BUFFERS {
            mic.post(i);
        }
        serial_println!(
            "MICROFONO virtio-sound {} Hz, {} canal(es), flujo {}",
            rate,
            channels,
            stream
        );
        Some(mic)
    }

    fn control(&mut self, req_len: usize, resp_len: usize) -> Option<u32> {
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
        self.control(8, 4) == Some(S_OK)
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
            if i >= BUFFERS {
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
