//! Fuentes de entropía del kernel y el generador global (K10, ADR 0009).
//!
//! Hardware: las instrucciones RDSEED y RDRAND de la CPU (Intel desde Broadwell/Ivy Bridge, AMD
//! desde Zen) y el TSC. Referencias: Intel SDM vol. 1, 7.3.17 "Random Number Generator
//! Instructions"; <https://wiki.osdev.org/Random_Number_Generator>.
//!
//! La lógica (acumular, estimar, estirar con ChaCha20) está en `jarvis_tls::rng`, con tests; acá
//! solo se leen las fuentes. RDRAND puede no estar (el `qemu64` de QEMU no lo tiene), así que la
//! variación del TSC tiene que alcanzar sola.

use alloc::vec::Vec;
use core::arch::x86_64::{__cpuid, __cpuid_count};

use jarvis_tls::rng::{Pool, Rng, jitter_credit};

use crate::irqlock::IrqMutex;
use crate::time;

static RNG: IrqMutex<Option<Rng>> = IrqMutex::new(None);

/// Mediciones del TSC por vuelta, y vueltas como mucho antes de rendirse.
const SAMPLES: usize = 256;
const MAX_ROUNDS: usize = 64;

/// Lo que se juntó al arrancar (para el log).
pub struct Report {
    pub rdseed: bool,
    pub rdrand: bool,
    pub jitter_bits: u32,
    pub bits: u32,
    pub ready: bool,
}

/// Junta entropía y arma el generador. Se llama una vez al arrancar, con el heap listo.
pub fn init() -> Report {
    let mut pool = Pool::new();
    pool.add(&time::rdtsc().to_le_bytes(), 0);

    let rdrand = __cpuid(1).ecx & (1 << 30) != 0;
    let rdseed = __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 18) != 0;
    for _ in 0..8 {
        // RDSEED sale directo de la fuente física: se le cree la mitad. RDRAND pasa por un
        // DRBG de la CPU: se le cree un cuarto. Nunca se depende solo de la CPU.
        if rdseed && let Some(v) = hw_random(true) {
            pool.add(&v.to_le_bytes(), 32);
        }
        if rdrand && let Some(v) = hw_random(false) {
            pool.add(&v.to_le_bytes(), 16);
        }
    }

    let mut jitter_bits = 0;
    let mut deltas = Vec::with_capacity(SAMPLES);
    for _ in 0..MAX_ROUNDS {
        deltas.clear();
        for _ in 0..SAMPLES {
            deltas.push(timed_work());
        }
        let bits = jitter_credit(&deltas);
        jitter_bits += bits;
        for d in &deltas {
            pool.add(&d.to_le_bytes(), 0);
        }
        pool.add(&[], bits);
        if pool.ready() {
            break;
        }
    }

    let bits = pool.credited();
    let ready = pool.ready();
    // Sin entropía suficiente igual se arma el generador (la red lo va a necesitar para los
    // puertos y los id de DNS), pero `ready` queda en falso y TLS no se usa.
    let rng = Rng::from_seed(pool.seed());
    RNG.with(|r| *r = Some(rng));
    READY.store(ready, core::sync::atomic::Ordering::Relaxed);
    Report {
        rdseed,
        rdrand,
        jitter_bits,
        bits,
        ready,
    }
}

static READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// ¿Hubo entropía suficiente para generar claves?
#[allow(dead_code)] // lo usa TLS (siguiente etapa de K10)
pub fn ready() -> bool {
    READY.load(core::sync::atomic::Ordering::Relaxed)
}

/// Llena `out` con bytes al azar. Falso si todavía no hay generador.
#[allow(dead_code)] // lo usa TLS (siguiente etapa de K10)
pub fn fill(out: &mut [u8]) -> bool {
    RNG.with(|r| match r {
        Some(rng) => {
            // Cada pedido mezcla el TSC: barato y suma algo de imprevisibilidad.
            rng.reseed(&time::rdtsc().to_le_bytes());
            rng.fill(out);
            true
        }
        None => false,
    })
}

/// Cuántos ciclos tarda un trabajo corto que toca memoria. La variación entre repeticiones es la
/// entropía (cachés, TLB, el hipervisor).
fn timed_work() -> u64 {
    let mut buf = [0u64; 32];
    let start = time::rdtsc();
    for i in 0..buf.len() {
        // `black_box` para que el compilador no borre el trabajo.
        buf[i] = core::hint::black_box(buf[(i * 7) % 32].wrapping_add(i as u64));
    }
    core::hint::black_box(&buf);
    time::rdtsc().wrapping_sub(start)
}

/// Un valor de RDSEED (`seed`) o RDRAND. Pueden fallar si la fuente está agotada: se reintenta
/// unas veces (lo que recomienda Intel) y si no, `None`.
fn hw_random(seed: bool) -> Option<u64> {
    for _ in 0..10 {
        let mut v = 0u64;
        // SAFETY: solo se llama si `cpuid` confirmó que la instrucción existe; no toca más
        // memoria que `v`.
        let ok = unsafe {
            if seed {
                rdseed64(&mut v)
            } else {
                rdrand64(&mut v)
            }
        };
        if ok {
            return Some(v);
        }
    }
    None
}

/// # Safety
/// La CPU tiene que tener RDSEED (CPUID.7.0:EBX[18]).
#[target_feature(enable = "rdseed")]
unsafe fn rdseed64(v: &mut u64) -> bool {
    // Con la instrucción habilitada por `target_feature`, la intrínseca es segura.
    core::arch::x86_64::_rdseed64_step(v) == 1
}

/// # Safety
/// La CPU tiene que tener RDRAND (CPUID.1:ECX[30]).
#[target_feature(enable = "rdrand")]
unsafe fn rdrand64(v: &mut u64) -> bool {
    // Con la instrucción habilitada por `target_feature`, la intrínseca es segura.
    core::arch::x86_64::_rdrand64_step(v) == 1
}
