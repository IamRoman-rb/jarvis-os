//! Escritorio de JARVIS-OS: eventos, modos y la app Archivos.
//!
//! El kernel solo traduce el hardware (teclas, bytes del mouse, sectores del disco) y le pasa
//! eventos a [`Desktop`]. Toda la lógica de la interfaz vive acá, sin `std`, y se prueba en el
//! host con un disco en memoria (ver `tests/`).

#![no_std]

extern crate alloc;

pub mod cursor;
pub mod desktop;
pub mod files;
pub mod files_view;
pub mod input;
pub mod text_input;

pub use desktop::{DEMO_PHRASES, Desktop, GREETING, Mode};
pub use input::{Event, Key, MouseDecoder, MousePacket};
