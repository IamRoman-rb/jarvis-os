//! Escritorio de JARVIS-OS: ventanas, apps, atajos de teclado y la composición de la pantalla.
//!
//! El kernel solo traduce el hardware (teclas, bytes del mouse, sectores del disco, paquetes de
//! red) y le pasa eventos a [`Desktop`]. Toda la lógica de la interfaz vive acá, sin `std`, y se
//! prueba en el host con un disco en memoria (ver `tests/`).

#![no_std]

extern crate alloc;

pub mod apps;
pub mod bmp;
pub mod chrome;
pub mod cursor;
pub mod desktop;
pub mod files;
pub mod files_view;
pub mod input;
pub mod shell;
pub mod system;
pub mod text_input;
pub mod web;
pub mod widgets;
pub mod wm;

pub use desktop::{DEMO_PHRASES, Desktop, Dirty, GREETING, Requests};
pub use input::{Event, Key, Mods, MouseDecoder, MousePacket};
pub use system::{AppKind, HttpResponse, Launch, NetInfo, NetRequest, Power, SystemStats};
