//! Paleta "Obsidian Kinetic HUD" (design/stitch/obsidian_kinetic_hud/DESIGN.md).

use crate::Color;

/// Fondo base: "Void Black".
pub const VOID: Color = Color::hex(0x050b14);
/// Resplandor del fondo: "Deep Obsidian".
pub const OBSIDIAN: Color = Color::hex(0x081226);
/// Superficie de paneles.
pub const PANEL: Color = Color::hex(0x0d1a2e);
/// Borde de paneles pasivos: "Deep Vector Blue" apagado.
pub const PANEL_RIM: Color = Color::hex(0x1e3b5c);
/// Emisor principal: "Holographic Cyan".
pub const CYAN: Color = Color::hex(0x00f0ff);
/// Azul estructural: "Deep Vector Blue".
pub const VECTOR_BLUE: Color = Color::hex(0x0077ff);
/// Partículas de la esfera: del azul profundo al celeste.
pub const PARTICLE_DEEP: Color = Color::hex(0x1b4fd8);
pub const PARTICLE_BRIGHT: Color = Color::hex(0x8fd0ff);
/// Advertencias: "Stark Reactor Amber".
pub const AMBER: Color = Color::hex(0xff9d00);
/// Crítico: "Crimson Pulse".
pub const CRIMSON: Color = Color::hex(0xff2a55);
/// Texto.
pub const TEXT: Color = Color::hex(0xdce3f0);
pub const TEXT_DIM: Color = Color::hex(0x7f93ab);
pub const TEXT_FAINT: Color = Color::hex(0x2a3a52);
