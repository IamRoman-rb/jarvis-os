//! Gráficos del HUD de JARVIS-OS.
//!
//! Todo lo que está acá es dibujo puro sobre un buffer de bytes: no toca hardware. Por eso
//! compila sin `std` (lo usa el kernel) y a la vez se testea en el host con `cargo test`.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod canvas;
pub mod clock;
pub mod hud;
pub mod shapes;
pub mod sphere;
pub mod text;
pub mod theme;

pub use canvas::{Canvas, Color, PixelFormat};
