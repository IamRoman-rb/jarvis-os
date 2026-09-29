//! TLS en el kernel (K10, ADR 0009): une el cliente de `jarvis_tls` con el azar del kernel
//! (`entropy.rs`) y la hora del reloj CMOS (`rtc.rs`). No maneja hardware propio.
//!
//! La hora se lee del RTC **una vez**, al arrancar, y después se le suma `time::millis()`: así la
//! tarea de la red no toca los puertos del CMOS a la vez que el reloj del escritorio (el CMOS se
//! lee escribiendo un índice y leyendo el dato: dos lectores mezclados se pisan).

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

use jarvis_tls::{ClientConfig, GetRandomFailed, SecureRandom, TimeProvider, TlsClient, UnixTime};
use spin::Once;

use crate::{entropy, rtc, time};

/// Segundos Unix al arrancar (0 = el RTC no dio una fecha válida) y `time::millis()` en ese momento.
static BOOT_UNIX: AtomicU64 = AtomicU64::new(0);
static BOOT_MILLIS: AtomicU64 = AtomicU64::new(0);

static CONFIG: Once<Arc<ClientConfig>> = Once::new();

#[derive(Debug)]
struct KernelRandom;

impl SecureRandom for KernelRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        // Sin entropía suficiente, TLS no arranca: una clave adivinable es peor que no conectar.
        if entropy::ready() && entropy::fill(buf) {
            Ok(())
        } else {
            Err(GetRandomFailed)
        }
    }
}

static RANDOM: KernelRandom = KernelRandom;

#[derive(Debug)]
struct KernelClock;

impl TimeProvider for KernelClock {
    fn current_time(&self) -> Option<UnixTime> {
        let boot = BOOT_UNIX.load(Ordering::Relaxed);
        if boot == 0 {
            // Sin hora no se puede saber si un certificado venció: rustls rechaza la conexión.
            return None;
        }
        let elapsed = time::millis().saturating_sub(BOOT_MILLIS.load(Ordering::Relaxed)) / 1000;
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            boot + elapsed,
        )))
    }
}

/// Lo que dejó la prueba del arranque (para el log).
pub struct Report {
    pub roots: usize,
    pub clock_ok: bool,
    /// Largo del ClientHello de prueba y cuánto tardó (µs), o por qué falló.
    pub hello: Result<(usize, u64), jarvis_tls::Error>,
}

/// Arma la configuración (una vez) y prueba el primer paso de un handshake: generar las claves
/// efímeras (X25519 y P-256) y el ClientHello. No usa la red.
pub fn init() -> Report {
    if let Some(secs) = rtc::read_utc()
        .filter(|t| t.is_valid())
        .and_then(|t| t.unix_seconds())
    {
        BOOT_UNIX.store(secs, Ordering::Relaxed);
        BOOT_MILLIS.store(time::millis(), Ordering::Relaxed);
    }
    let roots = jarvis_tls::mozilla_roots();
    let n_roots = roots.len();
    let hello = jarvis_tls::client_config(&RANDOM, Arc::new(KernelClock), roots).and_then(|c| {
        let c = CONFIG.call_once(|| c).clone();
        let start = time::rdtsc();
        let mut client = TlsClient::new(c, "example.com")?;
        let us = time::tsc_to_us(time::rdtsc().wrapping_sub(start));
        Ok((client.take_outgoing().len(), us))
    });
    Report {
        roots: n_roots,
        clock_ok: BOOT_UNIX.load(Ordering::Relaxed) != 0,
        hello,
    }
}

/// La configuración de los clientes TLS, si `init` pudo armarla.
#[allow(dead_code)] // la usa la red (siguiente etapa de K10)
pub fn config() -> Option<Arc<ClientConfig>> {
    CONFIG.get().cloned()
}
