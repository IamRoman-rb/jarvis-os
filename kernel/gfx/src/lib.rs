//! Gráficos del HUD de JARVIS-OS.
//!
//! Todo lo que está acá es dibujo puro sobre un buffer de bytes: no toca hardware. Por eso
//! compila sin `std` (lo usa el kernel) y a la vez se testea en el host con `cargo test`.
//! Usa `alloc` (vectores, strings): en el kernel, eso requiere que haya un heap.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod assistant;
pub mod canvas;
pub mod clock;
pub mod hud;
pub mod shapes;
pub mod sphere;
pub mod text;
pub mod theme;
pub mod trig;
pub mod vfont;
pub mod webfont;

pub use canvas::{Canvas, Color, MAX_CLIP, PixelFormat, Rect};
