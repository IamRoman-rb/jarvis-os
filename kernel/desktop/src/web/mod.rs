//! Lo que queda de la web en el kernel: direcciones (`url`), pedidos y respuestas HTTP (`http`,
//! los usa la red para apt, curl y los demás) y JSON (`json`). El navegador
//! propio se sacó (ADR 0014): para navegar está Brave, en el anfitrión (ADR 0007).

use alloc::string::{String, ToString};

pub mod http;
pub mod json;
pub mod url;

/// Bytes de una respuesta → texto. Casi todo es UTF-8; si no lo es, se asume Latin-1 (cada byte
/// es un carácter), que era lo común en páginas viejas en castellano.
pub fn decode_bytes(b: &[u8]) -> String {
    match core::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}
