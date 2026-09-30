//! Intel High Definition Audio (HDA): la placa de sonido de casi toda PC desde 2004.
//!
//! La placa tiene dos partes. El **controlador** (en el chipset) mueve el audio por DMA desde la
//! memoria. Uno o más **codecs** (el chip Realtek de la placa madre, el HDMI de la placa de video)
//! convierten a analógico. Al codec se le habla con **verbos** de 32 bits (codec, nodo, verbo,
//! dato) que el controlador lleva por un anillo (CORB) y cuyas respuestas deja en otro (RIRB).
//!
//! Adentro del codec hay un **grafo de widgets**: convertidores (DAC), mezcladores, selectores y
//! "pin complex" (los conectores: parlante, auriculares, línea). Cada widget dice de qué otros
//! toma su entrada. Para que suene hay que elegir un pin de salida, encontrar un camino hasta un
//! DAC, elegir en cada selector la entrada correcta, sacar el silencio (mute) de los
//! amplificadores y encender el pin. Esa búsqueda está acá, sin hardware.
//! Referencias: "High Definition Audio Specification" 1.0a (Intel) §3 (controlador) y §7
//! (codecs y verbos), <https://wiki.osdev.org/Intel_High_Definition_Audio>.

use alloc::vec;
use alloc::vec::Vec;

/// Un verbo de 12 bits con 8 bits de dato (la mayoría de los "Get" y "Set").
pub fn verb(codec: u8, node: u8, verb: u16, data: u8) -> u32 {
    (codec as u32) << 28 | (node as u32) << 20 | ((verb & 0xFFF) as u32) << 8 | data as u32
}

/// Un verbo de 4 bits con 16 bits de dato (formato del convertidor, amplificadores).
pub fn verb16(codec: u8, node: u8, verb: u8, data: u16) -> u32 {
    (codec as u32) << 28 | (node as u32) << 20 | ((verb & 0xF) as u32) << 16 | data as u32
}

pub const GET_PARAMETER: u16 = 0xF00;
pub const GET_CONNECTION_LIST: u16 = 0xF02;
pub const GET_CONFIG_DEFAULT: u16 = 0xF1C;
pub const SET_CONNECTION_SELECT: u16 = 0x701;
pub const SET_POWER_STATE: u16 = 0x705;
pub const SET_STREAM_CHANNEL: u16 = 0x706;
pub const SET_PIN_CONTROL: u16 = 0x707;
pub const SET_EAPD: u16 = 0x70C;
pub const SET_FORMAT: u8 = 0x2;
pub const SET_AMP: u8 = 0x3;

// Parámetros de GET_PARAMETER.
pub const PARAM_VENDOR: u8 = 0x00;
pub const PARAM_NODE_COUNT: u8 = 0x04;
pub const PARAM_FUNCTION_TYPE: u8 = 0x05;
pub const PARAM_WIDGET_CAPS: u8 = 0x09;
pub const PARAM_PIN_CAPS: u8 = 0x0C;
pub const PARAM_CONN_LEN: u8 = 0x0E;
pub const PARAM_OUT_AMP_CAPS: u8 = 0x12;

/// El formato de 48 kHz, 16 bits, estéreo: base 48 kHz (bit 14 en 0), sin multiplicar ni
/// dividir, 16 bits (001 en los bits 4–6), 2 canales (1 = canales − 1).
pub const FORMAT_48K_16_STEREO: u16 = 0x0011;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetType {
    Output,
    Input,
    Mixer,
    Selector,
    Pin,
    Other,
}

impl WidgetType {
    pub fn from_caps(caps: u32) -> WidgetType {
        match (caps >> 20) & 0xF {
            0 => WidgetType::Output,
            1 => WidgetType::Input,
            2 => WidgetType::Mixer,
            3 => WidgetType::Selector,
            4 => WidgetType::Pin,
            _ => WidgetType::Other,
        }
    }
}

/// Lo que se sabe de un widget después de preguntarle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widget {
    pub nid: u8,
    pub kind: WidgetType,
    pub caps: u32,
    pub pin_caps: u32,
    /// "Configuración por defecto" del pin (la que grabó el fabricante de la placa).
    pub config: u32,
    pub connections: Vec<u8>,
    pub amp_out_caps: u32,
}

impl Widget {
    /// Tiene amplificador de salida (bit 2 de las capacidades).
    pub fn has_out_amp(&self) -> bool {
        self.caps & (1 << 2) != 0
    }
    pub fn has_in_amp(&self) -> bool {
        self.caps & (1 << 1) != 0
    }
    /// Qué es el pin según su configuración: 0 salida de línea, 1 parlante, 2 auriculares…
    pub fn default_device(&self) -> u8 {
        ((self.config >> 20) & 0xF) as u8
    }
    /// ¿Hay algo conectado físicamente? (bits 30–31: 1 = nada.)
    pub fn connected(&self) -> bool {
        (self.config >> 30) != 1
    }
    pub fn can_output(&self) -> bool {
        self.pin_caps & (1 << 4) != 0
    }
}

/// Un paso del camino: el widget y, si tiene varias entradas, cuál elegir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub nid: u8,
    /// Índice en su lista de conexiones del siguiente widget del camino (hacia el DAC).
    pub select: Option<u8>,
}

/// El camino de un pin de salida a un DAC: el primero es el pin, el último el DAC.
pub fn output_path(widgets: &[Widget]) -> Option<Vec<Step>> {
    // Preferencia: parlante (1), auriculares (2), salida de línea (0). Entre iguales, el de nid
    // más bajo (en los codecs Realtek, el conector trasero verde).
    let rank = |w: &Widget| match w.default_device() {
        1 => 0,
        2 => 1,
        0 => 2,
        _ => 3,
    };
    let mut pins: Vec<&Widget> = widgets
        .iter()
        .filter(|w| w.kind == WidgetType::Pin && w.can_output() && w.connected())
        .filter(|w| rank(w) < 3)
        .collect();
    pins.sort_by_key(|w| (rank(w), w.nid));
    pins.iter()
        .find_map(|p| path_from(widgets, p.nid, &mut vec![]))
}

/// Búsqueda en profundidad hasta un DAC (sin repetir nodos: hay grafos con ciclos).
fn path_from(widgets: &[Widget], nid: u8, seen: &mut Vec<u8>) -> Option<Vec<Step>> {
    if seen.contains(&nid) || seen.len() > 8 {
        return None;
    }
    let w = widgets.iter().find(|w| w.nid == nid)?;
    if w.kind == WidgetType::Output {
        return Some(vec![Step { nid, select: None }]);
    }
    if !matches!(
        w.kind,
        WidgetType::Pin | WidgetType::Mixer | WidgetType::Selector
    ) {
        return None;
    }
    seen.push(nid);
    for (i, &next) in w.connections.iter().enumerate() {
        if let Some(mut rest) = path_from(widgets, next, seen) {
            // Los mezcladores suman todas sus entradas: no se elige; el resto, sí.
            let select =
                (w.kind != WidgetType::Mixer && w.connections.len() > 1).then_some(i as u8);
            rest.insert(0, Step { nid, select });
            return Some(rest);
        }
    }
    seen.pop();
    None
}

/// La lista de conexiones de un widget, a partir de las respuestas de GET_CONNECTION_LIST (4
/// entradas de 8 bits por respuesta en la forma corta; `len` sale de PARAM_CONN_LEN).
pub fn connections(len_param: u32, responses: &[u32]) -> Vec<u8> {
    let len = (len_param & 0x7F) as usize;
    let long = len_param & 0x80 != 0;
    let per = if long { 2 } else { 4 };
    let bits = if long { 16 } else { 8 };
    let mut out = Vec::new();
    let mut last = 0u8;
    for i in 0..len {
        let Some(r) = responses.get(i / per) else {
            break;
        };
        let entry = (r >> ((i % per) * bits)) & ((1 << bits) - 1);
        let nid = (entry & 0x7F) as u8;
        // Bit alto: "rango": todos los nodos desde el anterior hasta este.
        let range = entry & (1 << (bits - 1)) != 0;
        if range && last != 0 {
            out.extend(last + 1..=nid);
        } else {
            out.push(nid);
        }
        last = nid;
    }
    out
}

/// El dato de SET_AMP para sacar el silencio de la salida en los dos canales con ganancia
/// `gain` (bits 0–6): bit 15 salida, 13 y 12 izquierdo y derecho.
pub fn amp_out_unmute(gain: u8) -> u16 {
    0xB000 | (gain & 0x7F) as u16
}

/// Ídem para la entrada `index` de un mezclador (bit 14 entrada, bits 8–11 el índice).
pub fn amp_in_unmute(index: u8, gain: u8) -> u16 {
    0x7000 | ((index & 0xF) as u16) << 8 | (gain & 0x7F) as u16
}

/// La ganancia "0 dB" de un amplificador (su `offset`), que es la que no distorsiona.
pub fn amp_0db(amp_caps: u32) -> u8 {
    (amp_caps & 0x7F) as u8
}
