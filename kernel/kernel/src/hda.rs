//! Driver de sonido Intel High Definition Audio (K13; la salida de audio que K12 dejó
//! pendiente para el hardware real).
//!
//! Hardware: controladores PCI de clase 04/03/00: el `intel-hda` de QEMU (con `hda-output`) y el
//! de la PC de Roman (AMD 1022:15E3, con un codec Realtek). El HDMI de la placa de video también
//! es un controlador HDA, pero solo tiene salidas digitales: no se elige.
//!
//! 1. Reset del controlador; los codecs presentes aparecen en STATESTS.
//! 2. CORB y RIRB: los anillos por los que van los verbos y vuelven las respuestas.
//! 3. Se recorre el grafo de widgets del codec y se busca un camino de un pin de salida a un DAC
//!    (`jarvis_drivers::hda::output_path`); se lo configura (selectores, amplificadores, pin).
//! 4. Un flujo de salida con una lista de 4 buffers de 20 ms (BDL) que la placa recorre en
//!    círculo. La tarea "audio" mira por dónde va (LPIB) y rellena los que ya sonaron.
//!
//! Sin interrupciones: la tarea del audio revisa cada 5 ms, y con 4 buffers de 20 ms hay 60 ms de
//! margen.
//! Referencias: "High Definition Audio Specification" 1.0a §3.3 (registros), §4 (programación)
//! y §7.3 (verbos); <https://wiki.osdev.org/Intel_High_Definition_Audio>.

use alloc::vec::Vec;

use jarvis_drivers::hda::{
    self, FORMAT_48K_16_STEREO, GET_CONFIG_DEFAULT, GET_CONNECTION_LIST, GET_PARAMETER,
    PARAM_CONN_LEN, PARAM_FUNCTION_TYPE, PARAM_NODE_COUNT, PARAM_OUT_AMP_CAPS, PARAM_PIN_CAPS,
    PARAM_VENDOR, PARAM_WIDGET_CAPS, SET_AMP, SET_CONNECTION_SELECT, SET_EAPD, SET_FORMAT,
    SET_PIN_CONTROL, SET_POWER_STATE, SET_STREAM_CHANNEL, Widget, WidgetType,
};

use crate::audio::Output;
use crate::mmio::Mmio;
use crate::{dma, pci, serial_println, time};

const GCAP: usize = 0x00;
const GCTL: usize = 0x08;
const STATESTS: usize = 0x0E;
const CORBLBASE: usize = 0x40;
const CORBWP: usize = 0x48;
const CORBRP: usize = 0x4A;
const CORBCTL: usize = 0x4C;
const CORBSIZE: usize = 0x4E;
const RIRBLBASE: usize = 0x50;
const RIRBWP: usize = 0x58;
const RINTCNT: usize = 0x5A;
const RIRBCTL: usize = 0x5C;
const RIRBSTS: usize = 0x5D;
const RIRBSIZE: usize = 0x5E;
// Registros de un flujo (desde su base).
const SD_CTL: usize = 0x00;
const SD_LPIB: usize = 0x04;
const SD_CBL: usize = 0x08;
const SD_LVI: usize = 0x0C;
const SD_FMT: usize = 0x12;
const SD_BDPL: usize = 0x18;

const RATE: u32 = 48_000;
const BUFFERS: usize = 4;
/// 20 ms de estéreo de 16 bits (múltiplo de 128 bytes, como pide la BDL).
const PERIOD: usize = (RATE as usize / 50) * 4;
/// El número de flujo que se le asigna a la salida (1–15; 0 no se usa).
const STREAM_TAG: u8 = 1;

pub struct Hda {
    sd: Mmio,
    bufs: *mut u8,
    real: [u32; BUFFERS],
    /// El buffer que estaba sonando la última vez que se miró.
    current: usize,
    played: u64,
}

// SAFETY: memoria DMA propia; la usa solo la tarea del audio.
unsafe impl Send for Hda {}

/// Los anillos de verbos.
struct Commands {
    regs: Mmio,
    corb: *mut u32,
    rirb: *mut u64,
    wp: u16,
    rp: u16,
}

impl Commands {
    fn start(regs: Mmio) -> Option<Commands> {
        let corb = dma::alloc(256 * 4, 128)? as *mut u32;
        let rirb = dma::alloc(256 * 8, 128)? as *mut u64;
        regs.w8(CORBCTL, 0);
        regs.w8(RIRBCTL, 0);
        let t = time::millis();
        while (regs.r8(CORBCTL) | regs.r8(RIRBCTL)) & 2 != 0 && time::millis() - t < 50 {}
        // 256 entradas (bits 0–1 = 2), si el controlador las admite (bit 6 de la capacidad).
        if regs.r8(CORBSIZE) & 0x40 != 0 {
            regs.w8(CORBSIZE, 2);
        }
        if regs.r8(RIRBSIZE) & 0x40 != 0 {
            regs.w8(RIRBSIZE, 2);
        }
        regs.w64_split(CORBLBASE, dma::phys(corb as *const u8));
        regs.w64_split(RIRBLBASE, dma::phys(rirb as *const u8));
        // Reset del puntero de lectura del CORB: escribir el bit 15, esperar que se vea, volver a 0.
        regs.w16(CORBRP, 1 << 15);
        let t = time::millis();
        while regs.r16(CORBRP) & 1 << 15 == 0 && time::millis() - t < 10 {}
        regs.w16(CORBRP, 0);
        while regs.r16(CORBRP) & 1 << 15 != 0 && time::millis() - t < 20 {}
        regs.w16(CORBWP, 0);
        regs.w16(RIRBWP, 1 << 15);
        // Cada RINTCNT respuestas el controlador avisa y se detiene hasta que se borre RIRBSTS:
        // el máximo, y se borra después de cada respuesta.
        regs.w16(RINTCNT, 0xFF);
        regs.w8(CORBCTL, 2); // RUN
        regs.w8(RIRBCTL, 2); // DMA
        Some(Commands {
            regs,
            corb,
            rirb,
            wp: 0,
            rp: 0,
        })
    }

    /// Manda un verbo y espera su respuesta.
    fn send(&mut self, command: u32) -> Option<u32> {
        self.wp = (self.wp + 1) % 256;
        // SAFETY: el CORB tiene 256 entradas y `wp < 256`.
        unsafe { core::ptr::write_volatile(self.corb.add(self.wp as usize), command) };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.regs.w16(CORBWP, self.wp);
        let t = time::millis();
        while self.regs.r16(RIRBWP) & 0xFF == self.rp {
            if time::millis() - t > 50 {
                return None;
            }
        }
        self.rp = (self.rp + 1) % 256;
        // SAFETY: el RIRB tiene 256 entradas y `rp < 256`; lo escribe el controlador.
        let r = unsafe { core::ptr::read_volatile(self.rirb.add(self.rp as usize)) };
        self.regs.w8(RIRBSTS, 0b101);
        Some(r as u32)
    }

    fn get(&mut self, codec: u8, node: u8, param: u8) -> u32 {
        self.send(hda::verb(codec, node, GET_PARAMETER, param))
            .unwrap_or(0)
    }
}

pub fn probe() -> Option<Hda> {
    for dev in pci::find_class(0x04, 0x03, 0x00) {
        if let Some(h) = Hda::start(dev) {
            return Some(h);
        }
    }
    None
}

impl Hda {
    fn start(dev: pci::Device) -> Option<Hda> {
        let bar = dev.bar_address(0)?;
        dev.enable_memory_and_dma();
        let regs = Mmio::map(bar, 0x1000)?;
        // Reset: CRST en 0, esperar, en 1, esperar; los codecs se anuncian en ~0,5 ms.
        regs.w32(GCTL, regs.r32(GCTL) & !1);
        let t = time::millis();
        while regs.r32(GCTL) & 1 != 0 && time::millis() - t < 100 {}
        regs.w32(GCTL, regs.r32(GCTL) | 1);
        while regs.r32(GCTL) & 1 == 0 && time::millis() - t < 200 {}
        let t = time::millis();
        while time::millis() - t < 2 {}
        let codecs = regs.r16(STATESTS);
        let gcap = regs.r16(GCAP);
        let (inputs, outputs) = (((gcap >> 8) & 0xF) as usize, ((gcap >> 12) & 0xF) as usize);
        if codecs == 0 || outputs == 0 {
            return None;
        }
        let mut cmd = Commands::start(regs)?;
        for codec in (0..15u8).filter(|c| codecs & 1 << c != 0) {
            let Some(dac) = configure_codec(&mut cmd, codec) else {
                continue;
            };
            // El DAC toma el flujo 1, canal 0, en el formato del flujo (antes de arrancarlo).
            let _ = cmd.send(hda::verb16(codec, dac, SET_FORMAT, FORMAT_48K_16_STEREO));
            let _ = cmd.send(hda::verb(codec, dac, SET_STREAM_CHANNEL, STREAM_TAG << 4));
            let sd = regs.sub(0x80 + inputs * 0x20, 0x20);
            let h = Hda::start_stream(sd)?;
            serial_println!(
                "HDA_LISTO {:04x}:{:04x}, codec {codec} ({:08x}), DAC {dac:#x}, {RATE} Hz",
                dev.vendor(),
                dev.device_id(),
                cmd.get(codec, 0, PARAM_VENDOR)
            );
            return Some(h);
        }
        serial_println!(
            "HDA {:04x}:{:04x}: ningún codec tiene salida analógica",
            dev.vendor(),
            dev.device_id()
        );
        None
    }

    fn start_stream(sd: Mmio) -> Option<Hda> {
        // Reset del flujo.
        sd.w8(SD_CTL, 1);
        let t = time::millis();
        while sd.r8(SD_CTL) & 1 == 0 && time::millis() - t < 10 {}
        sd.w8(SD_CTL, 0);
        while sd.r8(SD_CTL) & 1 != 0 && time::millis() - t < 20 {}
        let bdl = dma::alloc(BUFFERS * 16, 128)?;
        let bufs = dma::alloc(BUFFERS * PERIOD, 128)?;
        for i in 0..BUFFERS {
            let mut e = [0u8; 16];
            e[..8].copy_from_slice(&(dma::phys(bufs) + (i * PERIOD) as u64).to_le_bytes());
            e[8..12].copy_from_slice(&(PERIOD as u32).to_le_bytes());
            // SAFETY: la BDL tiene BUFFERS entradas de 16 bytes.
            unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), bdl.add(i * 16), 16) };
        }
        sd.w32(SD_CBL, (BUFFERS * PERIOD) as u32);
        sd.w16(SD_LVI, BUFFERS as u16 - 1);
        sd.w16(SD_FMT, FORMAT_48K_16_STEREO);
        sd.w64_split(SD_BDPL, dma::phys(bdl));
        sd.w8(SD_CTL + 2, STREAM_TAG << 4);
        // Los buffers empiezan en silencio (ya están en cero) y el flujo arranca.
        sd.w8(SD_CTL, 1 << 1);
        Some(Hda {
            sd,
            bufs,
            real: [0; BUFFERS],
            current: 0,
            played: 0,
        })
    }
}

/// Busca el grupo de funciones de audio del codec, su camino de salida y lo configura. Devuelve
/// el nodo del DAC.
fn configure_codec(cmd: &mut Commands, codec: u8) -> Option<u8> {
    let root = cmd.get(codec, 0, PARAM_NODE_COUNT);
    let (first, count) = ((root >> 16) as u8, (root & 0xFF) as u8);
    let afg = (first..first.saturating_add(count))
        .find(|&n| cmd.get(codec, n, PARAM_FUNCTION_TYPE) & 0xFF == 1)?;
    let _ = cmd.send(hda::verb(codec, afg, SET_POWER_STATE, 0));
    let default_amp = cmd.get(codec, afg, PARAM_OUT_AMP_CAPS);
    let nodes = cmd.get(codec, afg, PARAM_NODE_COUNT);
    let (first, count) = ((nodes >> 16) as u8, (nodes & 0xFF) as u8);
    let mut widgets = Vec::new();
    for nid in first..first.saturating_add(count) {
        let caps = cmd.get(codec, nid, PARAM_WIDGET_CAPS);
        let kind = WidgetType::from_caps(caps);
        let (pin_caps, config) = if kind == WidgetType::Pin {
            (
                cmd.get(codec, nid, PARAM_PIN_CAPS),
                cmd.send(hda::verb(codec, nid, GET_CONFIG_DEFAULT, 0))
                    .unwrap_or(0),
            )
        } else {
            (0, 0)
        };
        let conn_len = cmd.get(codec, nid, PARAM_CONN_LEN);
        let mut responses = Vec::new();
        let per = if conn_len & 0x80 != 0 { 2 } else { 4 };
        for i in (0..(conn_len & 0x7F) as u8).step_by(per) {
            responses.push(
                cmd.send(hda::verb(codec, nid, GET_CONNECTION_LIST, i))
                    .unwrap_or(0),
            );
        }
        // Bit 3: el widget tiene sus propios parámetros de amplificador (si no, los del grupo).
        let amp_out_caps = if caps & 1 << 3 != 0 {
            cmd.get(codec, nid, PARAM_OUT_AMP_CAPS)
        } else {
            default_amp
        };
        widgets.push(Widget {
            nid,
            kind,
            caps,
            pin_caps,
            config,
            connections: hda::connections(conn_len, &responses),
            amp_out_caps,
        });
    }
    let Some(path) = hda::output_path(&widgets) else {
        // Para diagnosticar en una placa nueva: el grafo entero.
        for w in &widgets {
            serial_println!(
                "HDA codec {codec} nodo {:#x}: {:?}, caps {:#x}, pin {:#x}, config {:#010x}, entradas {:x?}",
                w.nid,
                w.kind,
                w.caps,
                w.pin_caps,
                w.config,
                w.connections
            );
        }
        return None;
    };
    for (i, step) in path.iter().enumerate() {
        let Some(w) = widgets.iter().find(|w| w.nid == step.nid) else {
            continue;
        };
        let _ = cmd.send(hda::verb(codec, w.nid, SET_POWER_STATE, 0));
        if let Some(sel) = step.select {
            let _ = cmd.send(hda::verb(codec, w.nid, SET_CONNECTION_SELECT, sel));
        }
        if w.has_out_amp() {
            let gain = hda::amp_0db(w.amp_out_caps);
            let _ = cmd.send(hda::verb16(
                codec,
                w.nid,
                SET_AMP,
                hda::amp_out_unmute(gain),
            ));
        }
        // En un mezclador, la entrada que viene del DAC (el paso siguiente) sin silencio.
        if w.kind == WidgetType::Mixer
            && w.has_in_amp()
            && let Some(next) = path.get(i + 1)
            && let Some(idx) = w.connections.iter().position(|&c| c == next.nid)
        {
            let _ = cmd.send(hda::verb16(
                codec,
                w.nid,
                SET_AMP,
                hda::amp_in_unmute(idx as u8, 0),
            ));
        }
        if w.kind == WidgetType::Pin {
            // Salida, y con amplificador de auriculares si es un conector de auriculares.
            let hp = if w.default_device() == 2 { 0x80 } else { 0 };
            let _ = cmd.send(hda::verb(codec, w.nid, SET_PIN_CONTROL, 0x40 | hp));
            if w.pin_caps & 1 << 16 != 0 {
                let _ = cmd.send(hda::verb(codec, w.nid, SET_EAPD, 0x02));
            }
            serial_println!(
                "HDA: codec {codec}, pin {:#x} ({}) → {} pasos",
                w.nid,
                match w.default_device() {
                    1 => "parlante",
                    2 => "auriculares",
                    _ => "salida de línea",
                },
                path.len()
            );
        }
    }
    path.last().map(|s| s.nid)
}

impl Output for Hda {
    fn rate(&self) -> u32 {
        RATE
    }

    fn poll(&mut self, fill: &mut dyn FnMut(&mut [i16]) -> usize) {
        let pos = self.sd.r32(SD_LPIB) as usize % (BUFFERS * PERIOD);
        let playing = pos / PERIOD;
        // Los buffers entre el último visto y el que suena ahora ya terminaron: se rellenan.
        while self.current != playing {
            let i = self.current;
            self.played += self.real[i] as u64;
            // SAFETY: el buffer `i` mide PERIOD bytes (PERIOD / 2 muestras de 16 bits) y la placa
            // no lo está leyendo (ya pasó por él).
            let buf = unsafe {
                core::slice::from_raw_parts_mut(self.bufs.add(i * PERIOD) as *mut i16, PERIOD / 2)
            };
            buf.fill(0);
            self.real[i] = fill(buf) as u32;
            self.current = (i + 1) % BUFFERS;
        }
    }

    fn played(&self) -> u64 {
        self.played
    }
}
