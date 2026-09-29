//! La salida de audio (K12): una tarea que alimenta los parlantes.
//!
//! El mezclador vive en el escritorio (lo manejan las apps y la voz de JARVIS), pero la placa no
//! puede esperar a que termine un cuadro: si un cuadro tarda 300 ms (maquetar una página
//! pesada), el audio se cortaría. Por eso son dos cosas:
//!
//! - el escritorio, en cada cuadro, mezcla lo que falta para tener ~[`AHEAD_MS`] de audio por
//!   delante y lo deja en un anillo ([`fill`]);
//! - la tarea "audio" (prioridad alta, como la red) despierta cada 5 ms, le devuelve a la placa
//!   los buffers que ya sonaron llenos con lo del anillo, y cuenta cuánto sonó ([`played`]).
//!
//! Así un cuadro lento come del anillo en vez de dejar a la placa sin nada. Si igual se vacía, va
//! silencio y se anota ("underrun").

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use jarvis_task::Priority;

use crate::irqlock::IrqMutex;
use crate::virtio_sound::Speaker;
use crate::{serial_println, task, time};

/// Cuánto audio se mezcla por adelantado (además de los 100 ms de buffers de la placa).
pub const AHEAD_MS: u64 = 120;

struct Ring {
    /// Muestras estéreo intercaladas, a la frecuencia de la placa.
    pcm: VecDeque<i16>,
    rate: u32,
}

static RING: IrqMutex<Ring> = IrqMutex::new(Ring {
    pcm: VecDeque::new(),
    rate: 0,
});
static PLAYED: AtomicU64 = AtomicU64::new(0);
static UNDERRUNS: AtomicU64 = AtomicU64::new(0);

/// Arranca la tarea que alimenta la placa.
pub fn start(mut speaker: Speaker) -> u32 {
    let rate = speaker.rate;
    RING.with(|r| r.rate = rate);
    task::spawn("audio", Priority::High, 32 * 1024, move || {
        loop {
            speaker.poll(&mut |buf: &mut [i16]| {
                let n = RING.with(|r| {
                    let n = (r.pcm.len() / 2).min(buf.len() / 2);
                    for (dst, src) in buf.iter_mut().zip(r.pcm.drain(..n * 2)) {
                        *dst = src;
                    }
                    n
                });
                if n < buf.len() / 2 {
                    UNDERRUNS.fetch_add(1, Ordering::Relaxed);
                }
                n
            });
            PLAYED.store(speaker.played, Ordering::Relaxed);
            task::wait(0, Some(time::millis() + 5));
        }
    });
    serial_println!("AUDIO_TAREA {} Hz, {} ms por delante", rate, AHEAD_MS);
    rate
}

/// Cuadros que faltan en el anillo para tener [`AHEAD_MS`] por delante (0 si no hay placa).
pub fn room() -> usize {
    RING.with(|r| {
        if r.rate == 0 {
            return 0;
        }
        let want = (r.rate as u64 * AHEAD_MS / 1000) as usize;
        want.saturating_sub(r.pcm.len() / 2)
    })
}

/// Deja audio mezclado (estéreo intercalado) para la placa.
pub fn fill(pcm: Vec<i16>) {
    RING.with(|r| r.pcm.extend(pcm));
}

/// Cuadros de audio de verdad que ya sonaron.
pub fn played() -> u64 {
    PLAYED.load(Ordering::Relaxed)
}

/// Buffers que salieron (en parte) con silencio porque el anillo estaba vacío.
pub fn underruns() -> u64 {
    UNDERRUNS.load(Ordering::Relaxed)
}
