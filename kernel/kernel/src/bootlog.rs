//! El registro del arranque (K13, ADR 0011): en una PC real no hay puerto serie, así que todo lo
//! que el kernel escribe por el serie hasta llegar al escritorio también queda acá.
//!
//! - **En pantalla, mientras arranca**: las últimas líneas se dibujan abajo, sobre el framebuffer
//!   del firmware. Si un driver se cuelga, en la pantalla queda la última línea que llegó.
//! - **En el error del núcleo**: el panic muestra las últimas líneas debajo del mensaje.
//! - **En el disco**: `/Sistema/arranque.log`, para leerlo después con el Editor.
//!
//! El buffer tiene tamaño fijo (sin heap: el serie se usa antes de que exista) y, si se llena,
//! se guarda el principio (lo que explica el arranque) y se cuenta lo que no entró.

use spin::Mutex;

/// 48 KiB: un arranque normal escribe unos 8.
const CAPACITY: usize = 48 * 1024;

struct Log {
    bytes: [u8; CAPACITY],
    len: usize,
    dropped: usize,
    /// Mientras sea `true`, cada línea también va a la consola de la pantalla.
    console: Option<fn(&str)>,
}

static LOG: Mutex<Log> = Mutex::new(Log {
    bytes: [0; CAPACITY],
    len: 0,
    dropped: 0,
    console: None,
});

/// Anota texto (lo llama el serie). Si el registro está tomado (un panic a mitad de una línea),
/// se pierde esa línea antes que colgar.
pub fn record(text: &str) {
    let console = {
        let Some(mut log) = LOG.try_lock() else {
            return;
        };
        let room = CAPACITY - log.len;
        let n = text.len().min(room);
        let at = log.len;
        log.bytes[at..at + n].copy_from_slice(&text.as_bytes()[..n]);
        log.len += n;
        log.dropped += text.len() - n;
        log.console
    };
    // Fuera del lock: la consola vuelve a leer el registro.
    if let Some(draw) = console
        && text.ends_with('\n')
    {
        with_tail(24, draw);
    }
}

/// Desde acá cada línea se dibuja en pantalla con `draw` (que recibe las últimas líneas).
pub fn start_console(draw: fn(&str)) {
    LOG.lock().console = Some(draw);
    with_tail(24, draw);
}

/// El escritorio ya dibuja: la consola deja de pintar encima.
pub fn stop_console() {
    LOG.lock().console = None;
}

/// Las últimas `lines` líneas (sin cortar caracteres UTF-8 a la mitad).
pub fn with_tail(lines: usize, f: impl FnOnce(&str)) {
    let Some(log) = LOG.try_lock() else {
        return;
    };
    let text = valid_prefix(&log.bytes[..log.len]);
    let start = text
        .trim_end_matches('\n')
        .rmatch_indices('\n')
        .nth(lines.saturating_sub(1))
        .map_or(0, |(i, _)| i + 1);
    f(&text[start..]);
}

/// Todo el registro, con una nota al final si algo no entró.
pub fn with_all(f: impl FnOnce(&str, usize)) {
    let log = LOG.lock();
    f(valid_prefix(&log.bytes[..log.len]), log.dropped);
}

/// El prefijo que es UTF-8 válido (el buffer puede terminar a mitad de un carácter).
fn valid_prefix(bytes: &[u8]) -> &str {
    match core::str::from_utf8(bytes) {
        Ok(s) => s,
        // SAFETY: `valid_up_to` es justamente la parte que es UTF-8 válido.
        Err(e) => unsafe { core::str::from_utf8_unchecked(&bytes[..e.valid_up_to()]) },
    }
}
