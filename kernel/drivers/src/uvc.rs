//! Webcams USB (UVC, *USB Video Class*): la mitad del driver que interpreta (ADR 0022).
//!
//! Casi todas las webcams (también la de la PC de Roman, 5843:7884) hablan UVC: una interfaz de
//! **control** (clase 0x0E, subclase 1) y una de **streaming** (subclase 2) cuyos descriptores
//! de clase dicen qué formatos manda (MJPEG, YUY2) y en qué tamaños. Para empezar a recibir:
//!
//! 1. **Probe/commit** ([`probe_control`], [`parse_probe`]): la PC propone formato, tamaño e
//!    intervalo con `SET_CUR(VS_PROBE_CONTROL)`, la cámara contesta con `GET_CUR` cuánto va a
//!    mandar por paquete y por cuadro, y `SET_CUR(VS_COMMIT_CONTROL)` lo fija.
//! 2. Se elige el **alternate setting** de la interfaz de streaming cuyo endpoint isócrono
//!    alcanza ([`Streaming::alternate_for`]) y se lo activa con `SET_INTERFACE`.
//! 3. Llegan **paquetes**: cada uno empieza con una cabecera (2–12 bytes) con el bit FID, que
//!    cambia de cuadro en cuadro, y EOF en el último. [`FrameAssembler`] los junta en cuadros.
//! 4. Un cuadro MJPEG se decodifica como JPEG (muchas cámaras no mandan las tablas de Huffman:
//!    [`mjpeg_fix`] les pone las estándar); uno YUY2 se pasa a RGB con [`yuy2_to_rgb`].
//!
//! La otra mitad (las transferencias isócronas de xHCI y SET_INTERFACE) va en el kernel. Sin el
//! Extension Pack de VirtualBox ni una webcam de verdad conectada a JARVIS-OS no se puede probar
//! de punta a punta; esta mitad se prueba con descriptores de una webcam real y cuadros
//! armados en el test.
//!
//! Referencias: *USB Device Class Definition for Video Devices* 1.1 (§2.4.3.3 cabecera de
//! paquete, §3.9 descriptores de streaming, §4.3.1.1 probe/commit) y *Payload Format for MJPEG*;
//! las tablas de Huffman son las de JPEG (ITU T.81, anexo K.3), las mismas que agrega Linux
//! (uvcvideo) y libuvc.

use alloc::vec::Vec;

use crate::le;

/// Clase de video (USB), y las subclases de sus interfaces.
pub const CLASS_VIDEO: u8 = 0x0E;
pub const SUBCLASS_CONTROL: u8 = 1;
pub const SUBCLASS_STREAMING: u8 = 2;

/// Pedidos de clase (`bRequest`).
pub const SET_CUR: u8 = 0x01;
pub const GET_CUR: u8 = 0x81;
/// Controles de la interfaz de streaming (`wValue` = control << 8).
pub const VS_PROBE_CONTROL: u8 = 1;
pub const VS_COMMIT_CONTROL: u8 = 2;
/// Largo del bloque de probe/commit en UVC 1.0 (1.1 agrega 8 bytes al final; se aceptan los dos).
pub const PROBE_LEN: usize = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mjpeg,
    Yuy2,
    /// Otro sin comprimir (NV12, …): no lo usamos.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub format: Format,
    pub format_index: u8,
    pub frame_index: u8,
    pub width: u16,
    pub height: u16,
    /// Intervalo por defecto en unidades de 100 ns (333333 = 30 cuadros por segundo).
    pub interval: u32,
    /// Los intervalos que acepta (si los lista; si es un rango, mínimo y máximo).
    pub intervals: Vec<u32>,
}

/// Un alternate setting de la interfaz de streaming con su endpoint isócrono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alternate {
    pub alternate: u8,
    pub endpoint: u8,
    /// Bytes por intervalo de servicio (`wMaxPacketSize` × transacciones adicionales).
    pub bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Streaming {
    pub control_interface: u8,
    pub interface: u8,
    pub frames: Vec<Frame>,
    /// Ordenados de menor a mayor ancho de banda; el 0 (sin ancho de banda) no está.
    pub alternates: Vec<Alternate>,
    /// Hay un endpoint masivo en el alternate 0 (algunas cámaras no usan isócronos).
    pub bulk_endpoint: Option<u8>,
}

impl Streaming {
    /// El tamaño más cercano a `w`×`h` sin pasarse (si todos se pasan, el más chico), con
    /// MJPEG antes que YUY2 (pesa mucho menos por el USB).
    pub fn choose(&self, w: u16, h: u16) -> Option<&Frame> {
        let usable = |f: &&Frame| f.format != Format::Other;
        let area = |f: &Frame| f.width as u32 * f.height as u32;
        let fits = |f: &&Frame| f.width <= w && f.height <= h;
        let best = |format: Format| {
            self.frames
                .iter()
                .filter(usable)
                .filter(|f| f.format == format)
                .filter(fits)
                .max_by_key(|f| area(f))
        };
        best(Format::Mjpeg)
            .or_else(|| best(Format::Yuy2))
            .or_else(|| self.frames.iter().filter(usable).min_by_key(|f| area(f)))
    }

    /// El alternate más chico que alcanza para `payload` bytes por intervalo (lo que dijo la
    /// cámara en el probe). Si ninguno alcanza, el más grande.
    pub fn alternate_for(&self, payload: u32) -> Option<Alternate> {
        self.alternates
            .iter()
            .find(|a| a.bytes >= payload)
            .or(self.alternates.last())
            .copied()
    }
}

/// Recorre la configuración entera y junta lo de video: la interfaz de control, la de
/// streaming, sus formatos y tamaños, y los alternates con su endpoint.
pub fn parse_streaming(config: &[u8]) -> Option<Streaming> {
    if config.len() < 9 || config[1] != 2 {
        return None;
    }
    let total = (le(config, 2, 2) as usize).min(config.len());
    let mut s = Streaming {
        control_interface: 0,
        interface: 0,
        frames: Vec::new(),
        alternates: Vec::new(),
        bulk_endpoint: None,
    };
    let (mut found_control, mut found_streaming) = (false, false);
    // La interfaz en la que estamos: (número, alternate, ¿es la de streaming?).
    let mut current: Option<(u8, u8, bool)> = None;
    // El formato del último VS_FORMAT_* (lo heredan sus VS_FRAME_*).
    let mut format = (Format::Other, 0u8);
    let mut at = 0;
    while at + 2 <= total {
        let (len, kind) = (config[at] as usize, config[at + 1]);
        if len < 2 || at + len > total {
            break;
        }
        let d = &config[at..at + len];
        match kind {
            // Interfaz.
            4 if len >= 9 => {
                let video = d[5] == CLASS_VIDEO;
                if video && d[6] == SUBCLASS_CONTROL && !found_control {
                    s.control_interface = d[2];
                    found_control = true;
                }
                let streaming = video && d[6] == SUBCLASS_STREAMING;
                if streaming && !found_streaming {
                    s.interface = d[2];
                    found_streaming = true;
                }
                current = Some((d[2], d[3], streaming && d[2] == s.interface));
            }
            // Endpoint.
            5 if len >= 7 => {
                if let Some((_, alt, true)) = current {
                    let raw = le(d, 4, 2) as u32;
                    let bytes = (raw & 0x7FF) * (1 + ((raw >> 11) & 3));
                    match d[3] & 3 {
                        1 if d[2] & 0x80 != 0 && alt != 0 => s.alternates.push(Alternate {
                            alternate: alt,
                            endpoint: d[2],
                            bytes,
                        }),
                        2 if d[2] & 0x80 != 0 => s.bulk_endpoint = Some(d[2]),
                        _ => {}
                    }
                }
            }
            // Descriptor de clase de una interfaz (CS_INTERFACE).
            0x24 if len >= 3 => {
                if let Some((_, _, true)) = current {
                    match d[2] {
                        // VS_FORMAT_UNCOMPRESSED: el GUID empieza con el FourCC.
                        0x04 if len >= 21 => {
                            let f = if &d[5..9] == b"YUY2" {
                                Format::Yuy2
                            } else {
                                Format::Other
                            };
                            format = (f, d[3]);
                        }
                        // VS_FORMAT_MJPEG.
                        0x06 if len >= 5 => format = (Format::Mjpeg, d[3]),
                        // VS_FRAME_UNCOMPRESSED / VS_FRAME_MJPEG (el mismo formato).
                        0x05 | 0x07 if len >= 26 => {
                            let count = d[25] as usize;
                            let n = if count == 0 { 3 } else { count };
                            let intervals = (0..n)
                                .map(|i| le(d, 26 + 4 * i, 4) as u32)
                                .filter(|&v| v != 0)
                                .collect();
                            s.frames.push(Frame {
                                format: format.0,
                                format_index: format.1,
                                frame_index: d[3],
                                width: le(d, 5, 2) as u16,
                                height: le(d, 7, 2) as u16,
                                interval: le(d, 21, 4) as u32,
                                intervals,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        at += len;
    }
    if !found_streaming {
        return None;
    }
    s.alternates.sort_by_key(|a| a.bytes);
    Some(s)
}

/// El bloque de probe que propone la PC: formato, tamaño e intervalo (lo demás lo completa la
/// cámara). `bmHint` = 1: que respete el intervalo.
pub fn probe_control(frame: &Frame) -> [u8; PROBE_LEN] {
    let mut p = [0u8; PROBE_LEN];
    p[0..2].copy_from_slice(&1u16.to_le_bytes());
    p[2] = frame.format_index;
    p[3] = frame.frame_index;
    p[4..8].copy_from_slice(&frame.interval.to_le_bytes());
    p
}

/// Lo que contestó la cámara al probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub format_index: u8,
    pub frame_index: u8,
    pub interval: u32,
    /// El cuadro más grande que va a mandar (para el buffer).
    pub max_frame: u32,
    /// Los bytes por intervalo de servicio que necesita (elige el alternate).
    pub max_payload: u32,
}

pub fn parse_probe(d: &[u8]) -> Option<Probe> {
    if d.len() < PROBE_LEN {
        return None;
    }
    Some(Probe {
        format_index: d[2],
        frame_index: d[3],
        interval: le(d, 4, 4) as u32,
        max_frame: le(d, 18, 4) as u32,
        max_payload: le(d, 22, 4) as u32,
    })
}

/// El paquete de setup de un pedido de clase a la interfaz de streaming: SET_CUR (de la PC a la
/// cámara) o GET_CUR (al revés) del control `control`.
pub fn class_request(get: bool, control: u8, interface: u8) -> [u8; 8] {
    let (request_type, request) = if get {
        (0xA1, GET_CUR)
    } else {
        (0x21, SET_CUR)
    };
    crate::usb::setup(
        request_type,
        request,
        (control as u16) << 8,
        interface as u16,
        PROBE_LEN as u16,
    )
}

/// SET_INTERFACE: activa un alternate de la interfaz (el 0 apaga el streaming).
pub fn set_interface(interface: u8, alternate: u8) -> [u8; 8] {
    crate::usb::setup(0x01, 11, alternate as u16, interface as u16, 0)
}

/// Junta los paquetes de un endpoint de video en cuadros enteros.
#[derive(Debug, Default)]
pub struct FrameAssembler {
    data: Vec<u8>,
    fid: Option<bool>,
    broken: bool,
    /// Tope de un cuadro (lo que dijo el probe); lo que pase se descarta.
    pub max_frame: usize,
}

impl FrameAssembler {
    pub fn new(max_frame: usize) -> FrameAssembler {
        FrameAssembler {
            max_frame,
            ..FrameAssembler::default()
        }
    }

    /// Un paquete (con su cabecera). Devuelve el cuadro que terminó, si terminó alguno.
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        // Un paquete vacío (o solo con un byte) no trae nada: la cámara no tenía qué mandar.
        let hlen = *packet.first()? as usize;
        if hlen < 2 || hlen > packet.len() {
            return None;
        }
        let flags = packet[1];
        let (fid, eof, err) = (flags & 1 != 0, flags & 2 != 0, flags & 0x40 != 0);
        let mut done = None;
        // FID cambió sin EOF: el cuadro anterior terminó (hay cámaras que no marcan EOF).
        if self.fid.is_some_and(|f| f != fid) && !self.data.is_empty() {
            done = self.finish();
        }
        self.fid = Some(fid);
        if err {
            self.broken = true;
        }
        let payload = &packet[hlen..];
        if self.data.len() + payload.len() > self.max_frame.max(1) {
            self.broken = true;
        } else {
            self.data.extend_from_slice(payload);
        }
        if eof {
            // Si cerró uno por FID y este mismo paquete trae EOF, gana el más nuevo.
            if let Some(f) = self.finish() {
                done = Some(f);
            }
            // El próximo paquete trae el otro FID: que no lo tome como cambio de cuadro.
            self.fid = None;
        }
        done
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        let data = core::mem::take(&mut self.data);
        let broken = core::mem::take(&mut self.broken);
        (!broken && !data.is_empty()).then_some(data)
    }
}

/// YUY2 (4:2:2: Y0 U Y1 V por cada dos píxeles) → RGB `0x00RRGGBB`, con BT.601 en enteros.
pub fn yuy2_to_rgb(width: usize, height: usize, data: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(width * height);
    for pair in data.as_chunks::<4>().0.iter().take(width * height / 2) {
        let (u, v) = (pair[1] as i32 - 128, pair[3] as i32 - 128);
        for y in [pair[0], pair[2]] {
            let c = (y as i32 - 16).max(0) * 298;
            let clamp = |x: i32| ((x + 128) >> 8).clamp(0, 255) as u32;
            let r = clamp(c + 409 * v);
            let g = clamp(c - 100 * u - 208 * v);
            let b = clamp(c + 516 * u);
            out.push(r << 16 | g << 8 | b);
        }
    }
    out
}

/// Las cuatro tablas de Huffman estándar de JPEG (T.81, anexo K.3) en un segmento DHT.
#[rustfmt::skip]
const STANDARD_DHT: [u8; 420] = {
    const DC_LUMA: [u8; 29] = [
        0x00, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0,
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    ];
    const DC_CHROMA: [u8; 29] = [
        0x01, 0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0,
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    ];
    const AC_LUMA: [u8; 179] = [
        0x10, 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d,
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
        0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
        0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
        0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
        0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
        0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
        0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
        0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
        0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
        0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa,
    ];
    const AC_CHROMA: [u8; 179] = [
        0x11, 0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77,
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
        0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
        0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
        0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
        0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
        0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
        0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
        0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
        0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa,
    ];
    // FF C4, largo (2 + 418), y las cuatro tablas.
    let mut out = [0u8; 420];
    out[0] = 0xFF;
    out[1] = 0xC4;
    out[2] = (418u16 >> 8) as u8;
    out[3] = 418u16 as u8;
    let mut at = 4;
    let tables: [&[u8]; 4] = [&DC_LUMA, &AC_LUMA, &DC_CHROMA, &AC_CHROMA];
    let mut t = 0;
    while t < 4 {
        let table = tables[t];
        let mut i = 0;
        while i < table.len() {
            out[at] = table[i];
            at += 1;
            i += 1;
        }
        t += 1;
    }
    out
};

/// Un cuadro MJPEG de webcam muchas veces viene sin tablas de Huffman (el formato MJPEG de
/// AVI/UVC las da por sabidas). Si falta el DHT, se agregan las estándar antes del SOS; si ya
/// está, se devuelve igual.
pub fn mjpeg_fix(jpeg: &[u8]) -> Vec<u8> {
    let mut at = 2;
    if jpeg.len() < 4 || jpeg[0..2] != [0xFF, 0xD8] {
        return jpeg.to_vec();
    }
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xFF {
            break;
        }
        let marker = jpeg[at + 1];
        match marker {
            0xC4 => return jpeg.to_vec(),
            0xDA => {
                let mut out = Vec::with_capacity(jpeg.len() + STANDARD_DHT.len());
                out.extend_from_slice(&jpeg[..at]);
                out.extend_from_slice(&STANDARD_DHT);
                out.extend_from_slice(&jpeg[at..]);
                return out;
            }
            _ => {
                let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
                at += 2 + len;
            }
        }
    }
    jpeg.to_vec()
}
