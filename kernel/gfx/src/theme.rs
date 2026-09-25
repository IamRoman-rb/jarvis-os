//! Paleta "Obsidian Kinetic HUD" (design/stitch/obsidian_kinetic_hud/DESIGN.md) y sus variantes.
//!
//! Cada color es un **casillero** ([`Slot`]) que se puede cambiar en tiempo de ejecución
//! (Configuración → Apariencia): el tema claro o el de alto contraste, y el color de acento. Todo
//! el sistema pide los colores con funciones (`theme::cyan()`, `theme::text()`) en vez de usar
//! constantes, así un cambio de tema llega a todas partes sin tocar cada pantalla.
//!
//! Los casilleros son atómicos: se leen sin locks desde cualquier lado (el kernel tiene un solo
//! hilo, pero los tests del host corren en paralelo).

use core::sync::atomic::{AtomicU32, Ordering};

use crate::Color;

/// Los colores del sistema. Los nombres son los del design system (el "cyan" es el color de
/// acento: con otro acento deja de ser cian, pero el papel es el mismo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Fondo base: "Void Black".
    Void,
    /// Resplandor del fondo: "Deep Obsidian".
    Obsidian,
    /// Superficie de paneles.
    Panel,
    /// Borde de paneles pasivos.
    PanelRim,
    /// Emisor principal (el acento): "Holographic Cyan".
    Cyan,
    /// Azul estructural: "Deep Vector Blue".
    VectorBlue,
    /// Partículas de la esfera: del azul profundo al celeste.
    ParticleDeep,
    ParticleBright,
    /// Advertencias: "Stark Reactor Amber".
    Amber,
    /// Crítico: "Crimson Pulse".
    Crimson,
    Text,
    TextDim,
    TextFaint,
    /// Fondo de las ventanas.
    Window,
    /// Fondo de los campos de texto.
    Field,
    /// Fila elegida en una lista.
    Selected,
    /// Menús y paneles flotantes (Inicio, Win+X, Win+A).
    Menu,
}

pub const SLOTS: usize = 17;

/// Un juego completo de colores (`0xRRGGBB` por casillero, en el orden de [`Slot`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette(pub [u32; SLOTS]);

impl Palette {
    /// El HUD oscuro de siempre.
    pub const HUD: Palette = Palette([
        0x050b14, 0x081226, 0x0d1a2e, 0x1e3b5c, 0x00f0ff, 0x0077ff, 0x1b4fd8, 0x8fd0ff, 0xff9d00,
        0xff2a55, 0xdce3f0, 0x7f93ab, 0x2a3a52, 0x08121f, 0x050c16, 0x0b2a3a, 0x081527,
    ]);

    /// Claro, como Windows o KDE Breeze: fondos casi blancos y texto oscuro.
    pub const LIGHT: Palette = Palette([
        0xe8edf4, 0xd9e2ee, 0xffffff, 0xbfcad8, 0x0067c0, 0x0a84ff, 0x3a6fd8, 0x0b5cad, 0xb86200,
        0xcc1440, 0x1b2430, 0x566476, 0xb7c2d0, 0xf6f8fb, 0xffffff, 0xd4e5fa, 0xf1f4f8,
    ]);

    /// Alto contraste: negro, blanco y amarillo (para ver mejor).
    pub const CONTRAST: Palette = Palette([
        0x000000, 0x000000, 0x000000, 0xffffff, 0xffff00, 0x00e5ff, 0x3a8dff, 0xffffff, 0xffb000,
        0xff4f6d, 0xffffff, 0xeeeeee, 0xa0a0a0, 0x000000, 0x000000, 0x403d00, 0x000000,
    ]);

    /// La misma paleta con otro color de acento (y un azul estructural que lo acompaña).
    pub const fn with_accent(self, accent: u32) -> Palette {
        let mut p = self;
        p.0[Slot::Cyan as usize] = accent;
        p.0[Slot::VectorBlue as usize] = darker(accent);
        p
    }

    pub const fn get(&self, s: Slot) -> Color {
        Color::hex(self.0[s as usize])
    }
}

/// 60 % del brillo: el "azul estructural" de un acento.
const fn darker(rgb: u32) -> u32 {
    let c = Color::hex(rgb).scale(153);
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

static CURRENT: [AtomicU32; SLOTS] = {
    let mut a = [const { AtomicU32::new(0) }; SLOTS];
    let mut i = 0;
    while i < SLOTS {
        a[i] = AtomicU32::new(Palette::HUD.0[i]);
        i += 1;
    }
    a
};

/// Cambia la paleta de todo el sistema.
pub fn apply(p: &Palette) {
    for (slot, v) in CURRENT.iter().zip(p.0) {
        slot.store(v, Ordering::Relaxed);
    }
}

/// La paleta que está en uso.
pub fn current() -> Palette {
    let mut p = [0; SLOTS];
    for (v, slot) in p.iter_mut().zip(&CURRENT) {
        *v = slot.load(Ordering::Relaxed);
    }
    Palette(p)
}

pub fn get(s: Slot) -> Color {
    Color::hex(CURRENT[s as usize].load(Ordering::Relaxed))
}

/// ¿El tema es claro? (Algunas cosas se dibujan distinto: sombras en vez de brillos.)
pub fn is_light() -> bool {
    get(Slot::Void).luma() > 128
}

macro_rules! slots {
    ($($name:ident => $slot:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name() -> Color {
                get(Slot::$slot)
            }
        )*
    };
}

slots! {
    void => Void,
    obsidian => Obsidian,
    panel => Panel,
    panel_rim => PanelRim,
    cyan => Cyan,
    vector_blue => VectorBlue,
    particle_deep => ParticleDeep,
    particle_bright => ParticleBright,
    amber => Amber,
    crimson => Crimson,
    text => Text,
    text_dim => TextDim,
    text_faint => TextFaint,
    window => Window,
    field => Field,
    selected => Selected,
    menu => Menu,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_paleta_por_defecto_es_la_del_design_system() {
        // Los valores de siempre: las capturas del HUD no cambian con el tema por defecto.
        assert_eq!(Palette::HUD.get(Slot::Cyan), Color::hex(0x00f0ff));
        assert_eq!(Palette::HUD.get(Slot::Void), Color::hex(0x050b14));
        assert_eq!(Palette::HUD.get(Slot::Window), Color::hex(0x08121f));
        let p = Palette::HUD.with_accent(0xff8800);
        assert_eq!(p.get(Slot::Cyan), Color::hex(0xff8800));
        assert_eq!(p.get(Slot::VectorBlue), Color::hex(0xff8800).scale(153));
        assert_eq!(p.get(Slot::Text), Palette::HUD.get(Slot::Text));
        assert!(Palette::LIGHT.get(Slot::Void).luma() > 128);
        assert!(Palette::HUD.get(Slot::Void).luma() < 128);
    }
}
