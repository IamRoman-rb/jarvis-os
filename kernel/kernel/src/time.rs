//! Reloj monótono del kernel en milisegundos, basado en el **TSC**.
//!
//! El TSC (Time Stamp Counter) es un contador de 64 bits de la CPU que avanza a frecuencia fija y
//! se lee con la instrucción `rdtsc`. A diferencia de contar interrupciones, no se puede "perder":
//! el valor siempre refleja el tiempo transcurrido. Lo que no se sabe de antemano es a qué
//! frecuencia avanza, así que al arrancar se **calibra**: se miden cuántos ciclos de TSC pasan en
//! 50 ms medidos con el PIT. Es lo mismo que hace Linux (`pit_calibrate_tsc`).
//! Referencia: <https://wiki.osdev.org/TSC>.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::pit;

const CALIBRATION_MS: u32 = 50;

static TSC_START: AtomicU64 = AtomicU64::new(0);
static TSC_PER_MS: AtomicU64 = AtomicU64::new(0);

pub fn rdtsc() -> u64 {
    // SAFETY: `rdtsc` existe en toda CPU x86_64 y solo lee un contador; no toca memoria.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Calibra el TSC. Devuelve su frecuencia en MHz (para el log).
pub fn init() -> u64 {
    let start = pit::wait_ms_polling(CALIBRATION_MS, rdtsc);
    let end = rdtsc();
    let per_ms = ((end - start) / CALIBRATION_MS as u64).max(1);
    TSC_PER_MS.store(per_ms, Ordering::Relaxed);
    TSC_START.store(end, Ordering::Relaxed);
    per_ms / 1000
}

/// Milisegundos desde la calibración.
/// Ciclos del TSC → microsegundos (para medir esperas cortas).
pub fn tsc_to_us(cycles: u64) -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed).max(1);
    cycles * 1000 / per_ms
}

pub fn millis() -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed);
    if per_ms == 0 {
        return 0;
    }
    (rdtsc() - TSC_START.load(Ordering::Relaxed)) / per_ms
}
