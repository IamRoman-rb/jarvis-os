//! Reloj monótono del kernel en milisegundos, basado en el **TSC**.
//!
//! El TSC (Time Stamp Counter) es un contador de 64 bits de la CPU que avanza a frecuencia fija y
//! se lee con la instrucción `rdtsc`. A diferencia de contar interrupciones, no se puede "perder":
//! el valor siempre refleja el tiempo transcurrido. Lo que no se sabe de antemano es a qué
//! frecuencia avanza, así que al arrancar se **calibra**: se miden cuántos ciclos de TSC pasan en
//! 50 ms medidos con el PIT. Es lo mismo que hace Linux (`pit_calibrate_tsc`).
//! Referencia: <https://wiki.osdev.org/TSC>.

//!
//! Si no hay PIT (K13: hay PC nuevas sin él), se arranca con la frecuencia que declara la CPU
//! (`cpuid` 0x16) y, apenas se leen las tablas ACPI, se recalibra con el timer PM de ACPI, que
//! cuenta a 3,579545 MHz en toda PC.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use jarvis_drivers::acpi::{Fadt, PM_TIMER_HZ, Register, pm_timer_delta};
use x86_64::instructions::port::Port;

use crate::pit;

const CALIBRATION_MS: u32 = 50;
/// Lecturas del puerto 0x61 antes de dar al PIT por ausente: cada una tarda ~1 µs (el bus
/// ISA es lento a propósito), así que 5 millones son varios segundos, lejos de los 50 ms.
const PIT_GIVE_UP: u64 = 5_000_000;

static TSC_START: AtomicU64 = AtomicU64::new(0);
static TSC_PER_MS: AtomicU64 = AtomicU64::new(0);
/// La calibración inicial fue una estimación (no hubo PIT).
static ESTIMATED: AtomicBool = AtomicBool::new(false);

pub fn rdtsc() -> u64 {
    // SAFETY: `rdtsc` existe en toda CPU x86_64 y solo lee un contador; no toca memoria.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Calibra el TSC. Devuelve su frecuencia en MHz (para el log).
pub fn init() -> u64 {
    let per_ms = match pit::wait_ms_polling(CALIBRATION_MS, PIT_GIVE_UP, rdtsc) {
        Some(start) => ((rdtsc() - start) / CALIBRATION_MS as u64).max(1),
        None => {
            ESTIMATED.store(true, Ordering::Relaxed);
            cpuid_mhz().unwrap_or(2000) * 1000
        }
    };
    TSC_PER_MS.store(per_ms, Ordering::Relaxed);
    TSC_START.store(rdtsc(), Ordering::Relaxed);
    per_ms / 1000
}

/// La frecuencia base que declara la CPU (`cpuid` hoja 0x16, Intel desde Skylake). Es una
/// aproximación del TSC, suficiente hasta recalibrar.
fn cpuid_mhz() -> Option<u64> {
    // La hoja 0 dice hasta qué hoja hay.
    if core::arch::x86_64::__cpuid(0).eax < 0x16 {
        return None;
    }
    let mhz = core::arch::x86_64::__cpuid(0x16).eax as u64 & 0xFFFF;
    (mhz != 0).then_some(mhz)
}

/// ¿El TSC se calibró a ojo (sin PIT)?
pub fn needs_recalibration() -> bool {
    ESTIMATED.load(Ordering::Relaxed)
}

/// Recalibra el TSC con el timer PM de ACPI. Devuelve los MHz, o `None` si no hay timer PM en
/// puertos (el caso normal en x86).
pub fn calibrate_with_pm_timer(fadt: &Fadt) -> Option<u64> {
    let Some(Register::Io(port)) = fadt.pm_timer else {
        return None;
    };
    let read = || {
        // SAFETY: el puerto del timer PM lo declaró la FADT; leerlo no tiene efectos.
        unsafe { Port::<u32>::new(port).read() }
    };
    let ticks = PM_TIMER_HZ * CALIBRATION_MS as u64 / 1000;
    let t0 = read();
    let c0 = rdtsc();
    while (pm_timer_delta(t0, read(), fadt.pm_timer_32) as u64) < ticks {
        core::hint::spin_loop();
    }
    let per_ms = ((rdtsc() - c0) / CALIBRATION_MS as u64).max(1);
    // Que `millis()` siga desde donde iba: se reancla el inicio con la nueva escala.
    let elapsed = millis();
    TSC_PER_MS.store(per_ms, Ordering::Relaxed);
    TSC_START.store(rdtsc() - elapsed * per_ms, Ordering::Relaxed);
    ESTIMATED.store(false, Ordering::Relaxed);
    Some(per_ms / 1000)
}

/// Milisegundos desde la calibración.
/// Ciclos del TSC → microsegundos (para medir esperas cortas).
pub fn tsc_to_us(cycles: u64) -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed).max(1);
    cycles * 1000 / per_ms
}

/// Nanosegundos desde la calibración (para el reloj monótono de los programas).
pub fn nanos() -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed).max(1) as u128;
    let cycles = rdtsc().saturating_sub(TSC_START.load(Ordering::Relaxed)) as u128;
    (cycles * 1_000_000 / per_ms) as u64
}

pub fn millis() -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed);
    if per_ms == 0 {
        return 0;
    }
    (rdtsc() - TSC_START.load(Ordering::Relaxed)) / per_ms
}
