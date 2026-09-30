//! Audio sin hardware: la información del micrófono que detectó el kernel y el nivel de un
//! pedazo de PCM (para el medidor). La salida (K12) está en `sound.rs`.

use alloc::string::String;

/// Lo que el kernel sabe del micrófono (Configuración → Micrófono).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicInfo {
    /// La placa ("virtio-sound").
    pub device: String,
    pub rate: u32,
    pub channels: u8,
    /// Nivel actual (0..=100) y el pico reciente.
    pub level: u8,
    pub peak: u8,
    /// Llega audio (la placa devuelve buffers).
    pub receiving: bool,
}

/// El nivel de un pedazo de PCM (0..=100): está en `jarvis-audio` (K12), con el mezclador.
pub use jarvis_audio::level;
