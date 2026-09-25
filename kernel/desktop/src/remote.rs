//! Protocolo con el puente de Brave (ADR 0007).
//!
//! Brave corre en el anfitrión sin ventana; el puente (`xtask/src/brave.rs`) le saca cuadros y
//! le pasa el mouse y el teclado. Este módulo es el idioma que hablan los dos lados, y lo usan
//! los dos: el kernel (sin `std`) y el puente (en el anfitrión). Así no puede haber dos versiones.
//!
//! Cada mensaje es `[largo u32 LE][tipo u8][datos]`, donde el largo cuenta el tipo y los datos.
//! Los números van en little endian y los textos como `[largo u16][UTF-8]`.
//!
//! La imagen viaja en **mosaicos** de hasta 64×64 píxeles: el puente compara cada cuadro con el
//! anterior y manda solo los mosaicos que cambiaron, en RGB comprimido con LZ4. Un cursor que
//! parpadea cuesta un mosaico, no la pantalla entera. Para no ahogar la red, el puente no manda
//! un cuadro nuevo hasta que el kernel confirma el anterior ([`ToBrave::FrameAck`]).

use alloc::string::String;
use alloc::vec::Vec;

/// Puerto del puente de Brave en el anfitrión (QEMU lo ve en 10.0.2.2).
pub const PORT: u16 = 8119;
/// Lado de los mosaicos.
pub const TILE: u16 = 64;
/// Tope de un mensaje (un cuadro 1920×1080 sin comprimir son 6 MiB).
pub const MAX_MSG: usize = 8 * 1024 * 1024;

/// Modificadores, con los mismos bits que usa el protocolo de DevTools.
pub const MOD_ALT: u8 = 1;
pub const MOD_CTRL: u8 = 2;
pub const MOD_META: u8 = 4;
pub const MOD_SHIFT: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    None,
    Left,
    Middle,
    Right,
}

/// Del kernel al puente.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToBrave {
    /// Lo primero que se manda: el token (vacío si el puente es local) y el tamaño de la página.
    Hello {
        token: String,
        w: u16,
        h: u16,
    },
    Resize {
        w: u16,
        h: u16,
    },
    Mouse {
        kind: MouseKind,
        button: Button,
        x: i16,
        y: i16,
        clicks: u8,
        mods: u8,
    },
    /// Rueda, en píxeles (positivo = hacia abajo).
    Wheel {
        x: i16,
        y: i16,
        dx: i16,
        dy: i16,
    },
    /// Una tecla apretada y soltada. `key` es el nombre del DOM ("Enter", "a"), `vk` el código
    /// virtual de Windows y `text` lo que escribe (vacío para las teclas especiales).
    Key {
        key: String,
        vk: u16,
        text: String,
        mods: u8,
    },
    Navigate(String),
    Back,
    Forward,
    Reload,
    TabNew,
    TabClose(u16),
    TabSelect(u16),
    /// El kernel ya dibujó el último cuadro: se puede mandar el siguiente.
    FrameAck,
}

/// Un pedazo de la página: RGB comprimido con LZ4 (sin el tamaño adelante).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub lz4: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabInfo {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabsState {
    pub tabs: Vec<TabInfo>,
    pub active: u16,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

/// Del puente al kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromBrave {
    /// Tamaño total de la página y los mosaicos que cambiaron.
    Frame {
        w: u16,
        h: u16,
        tiles: Vec<Tile>,
    },
    State(TabsState),
    /// Algo salió mal del lado del anfitrión (Brave no está instalado, token incorrecto…).
    Error(String),
}

// --- Codificación -------------------------------------------------------------------------------

struct W(Vec<u8>);

impl W {
    fn new(kind: u8) -> W {
        let mut v = Vec::with_capacity(16);
        v.extend_from_slice(&[0, 0, 0, 0, kind]);
        W(v)
    }
    fn u8(&mut self, v: u8) -> &mut W {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i16(&mut self, v: i16) -> &mut W {
        self.u16(v as u16)
    }
    fn u32(&mut self, v: u32) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn str(&mut self, s: &str) -> &mut W {
        let b = s.as_bytes();
        let n = b.len().min(u16::MAX as usize);
        self.u16(n as u16);
        self.0.extend_from_slice(&b[..n]);
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut W {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }
    fn done(&mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_le_bytes());
        core::mem::take(&mut self.0)
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        Some(self.u16()? as i16)
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        Some(self.take(n)?.to_vec())
    }
}

fn mouse_kind(v: u8) -> Option<MouseKind> {
    Some(match v {
        0 => MouseKind::Move,
        1 => MouseKind::Down,
        2 => MouseKind::Up,
        _ => return None,
    })
}

fn button(v: u8) -> Option<Button> {
    Some(match v {
        0 => Button::None,
        1 => Button::Left,
        2 => Button::Middle,
        3 => Button::Right,
        _ => return None,
    })
}

impl ToBrave {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ToBrave::Hello { token, w, h } => W::new(1).str(token).u16(*w).u16(*h).done(),
            ToBrave::Resize { w, h } => W::new(2).u16(*w).u16(*h).done(),
            ToBrave::Mouse {
                kind,
                button,
                x,
                y,
                clicks,
                mods,
            } => W::new(3)
                .u8(*kind as u8)
                .u8(*button as u8)
                .i16(*x)
                .i16(*y)
                .u8(*clicks)
                .u8(*mods)
                .done(),
            ToBrave::Wheel { x, y, dx, dy } => W::new(4).i16(*x).i16(*y).i16(*dx).i16(*dy).done(),
            ToBrave::Key {
                key,
                vk,
                text,
                mods,
            } => W::new(5).str(key).u16(*vk).str(text).u8(*mods).done(),
            ToBrave::Navigate(url) => W::new(6).str(url).done(),
            ToBrave::Back => W::new(7).done(),
            ToBrave::Forward => W::new(8).done(),
            ToBrave::Reload => W::new(9).done(),
            ToBrave::TabNew => W::new(10).done(),
            ToBrave::TabClose(i) => W::new(11).u16(*i).done(),
            ToBrave::TabSelect(i) => W::new(12).u16(*i).done(),
            ToBrave::FrameAck => W::new(13).done(),
        }
    }

    /// `body` = tipo + datos (sin el largo).
    pub fn decode(body: &[u8]) -> Option<ToBrave> {
        let mut r = R(body);
        Some(match r.u8()? {
            1 => ToBrave::Hello {
                token: r.str()?,
                w: r.u16()?,
                h: r.u16()?,
            },
            2 => ToBrave::Resize {
                w: r.u16()?,
                h: r.u16()?,
            },
            3 => ToBrave::Mouse {
                kind: mouse_kind(r.u8()?)?,
                button: button(r.u8()?)?,
                x: r.i16()?,
                y: r.i16()?,
                clicks: r.u8()?,
                mods: r.u8()?,
            },
            4 => ToBrave::Wheel {
                x: r.i16()?,
                y: r.i16()?,
                dx: r.i16()?,
                dy: r.i16()?,
            },
            5 => ToBrave::Key {
                key: r.str()?,
                vk: r.u16()?,
                text: r.str()?,
                mods: r.u8()?,
            },
            6 => ToBrave::Navigate(r.str()?),
            7 => ToBrave::Back,
            8 => ToBrave::Forward,
            9 => ToBrave::Reload,
            10 => ToBrave::TabNew,
            11 => ToBrave::TabClose(r.u16()?),
            12 => ToBrave::TabSelect(r.u16()?),
            13 => ToBrave::FrameAck,
            _ => return None,
        })
    }
}

impl FromBrave {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            FromBrave::Frame { w, h, tiles } => {
                let mut m = W::new(0x81);
                m.u16(*w).u16(*h).u16(tiles.len() as u16);
                for t in tiles {
                    m.u16(t.x).u16(t.y).u16(t.w).u16(t.h).bytes(&t.lz4);
                }
                m.done()
            }
            FromBrave::State(s) => {
                let mut m = W::new(0x82);
                m.u16(s.tabs.len() as u16);
                for t in &s.tabs {
                    m.str(&t.title).str(&t.url);
                }
                m.u16(s.active)
                    .u8(s.loading as u8)
                    .u8(s.can_back as u8)
                    .u8(s.can_forward as u8)
                    .done()
            }
            FromBrave::Error(e) => W::new(0x83).str(e).done(),
        }
    }

    pub fn decode(body: &[u8]) -> Option<FromBrave> {
        let mut r = R(body);
        Some(match r.u8()? {
            0x81 => {
                let (w, h, n) = (r.u16()?, r.u16()?, r.u16()?);
                let mut tiles = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    tiles.push(Tile {
                        x: r.u16()?,
                        y: r.u16()?,
                        w: r.u16()?,
                        h: r.u16()?,
                        lz4: r.bytes()?,
                    });
                }
                FromBrave::Frame { w, h, tiles }
            }
            0x82 => {
                let n = r.u16()?;
                let mut tabs = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    tabs.push(TabInfo {
                        title: r.str()?,
                        url: r.str()?,
                    });
                }
                FromBrave::State(TabsState {
                    tabs,
                    active: r.u16()?,
                    loading: r.u8()? != 0,
                    can_back: r.u8()? != 0,
                    can_forward: r.u8()? != 0,
                })
            }
            0x83 => FromBrave::Error(r.str()?),
            _ => return None,
        })
    }
}

/// Junta los bytes que llegan de a pedazos y los corta en mensajes.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// El próximo mensaje completo (tipo + datos), si ya llegó. `Err` si el largo es absurdo
    /// (la conexión está rota o no es el puente de Brave).
    pub fn next_message(&mut self) -> Option<Result<Vec<u8>, String>> {
        if self.buf.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if n == 0 || n > MAX_MSG {
            self.buf.clear();
            return Some(Err(alloc::format!("mensaje inválido ({n} bytes)")));
        }
        if self.buf.len() < 4 + n {
            return None;
        }
        let body = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Some(Ok(body))
    }
}

/// Descomprime un mosaico. `None` si los datos no dan el tamaño esperado.
pub fn tile_pixels(t: &Tile) -> Option<Vec<u8>> {
    let size = t.w as usize * t.h as usize * 3;
    let px = lz4_flex::block::decompress(&t.lz4, size).ok()?;
    (px.len() == size).then_some(px)
}

/// Comprime un mosaico (lo usa el puente y los tests).
pub fn compress_tile(x: u16, y: u16, w: u16, h: u16, rgb: &[u8]) -> Tile {
    Tile {
        x,
        y,
        w,
        h,
        lz4: lz4_flex::block::compress(rgb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn roundtrip_to(m: ToBrave) {
        let bytes = m.encode();
        let mut f = Framer::default();
        // De a un byte: tiene que armarse igual.
        for b in &bytes {
            f.push(&[*b]);
        }
        let body = f.next_message().unwrap().unwrap();
        assert_eq!(ToBrave::decode(&body), Some(m));
        assert!(f.next_message().is_none());
    }

    #[test]
    fn mensajes_al_puente_ida_y_vuelta() {
        for m in [
            ToBrave::Hello {
                token: "abc".into(),
                w: 1024,
                h: 700,
            },
            ToBrave::Resize { w: 1, h: 2 },
            ToBrave::Mouse {
                kind: MouseKind::Down,
                button: Button::Right,
                x: -3,
                y: 400,
                clicks: 2,
                mods: MOD_CTRL | MOD_SHIFT,
            },
            ToBrave::Wheel {
                x: 5,
                y: 6,
                dx: 0,
                dy: -120,
            },
            ToBrave::Key {
                key: "ñ".into(),
                vk: 192,
                text: "ñ".into(),
                mods: 0,
            },
            ToBrave::Navigate("https://search.brave.com/".into()),
            ToBrave::Back,
            ToBrave::Forward,
            ToBrave::Reload,
            ToBrave::TabNew,
            ToBrave::TabClose(3),
            ToBrave::TabSelect(1),
            ToBrave::FrameAck,
        ] {
            roundtrip_to(m);
        }
    }

    #[test]
    fn cuadros_y_estado_ida_y_vuelta() {
        let rgb: Vec<u8> = (0..64 * 32 * 3).map(|i| (i % 7) as u8).collect();
        let frame = FromBrave::Frame {
            w: 800,
            h: 600,
            tiles: vec![compress_tile(64, 128, 64, 32, &rgb)],
        };
        let state = FromBrave::State(TabsState {
            tabs: vec![
                TabInfo {
                    title: "Brave".into(),
                    url: "https://brave.com/".into(),
                },
                TabInfo::default(),
            ],
            active: 1,
            loading: true,
            can_back: true,
            can_forward: false,
        });
        let mut f = Framer::default();
        f.push(&frame.encode());
        f.push(&state.encode());
        f.push(&FromBrave::Error("sin Brave".into()).encode());
        let got: Vec<FromBrave> = core::iter::from_fn(|| f.next_message())
            .map(|b| FromBrave::decode(&b.unwrap()).unwrap())
            .collect();
        assert_eq!(got.len(), 3);
        if let FromBrave::Frame { tiles, .. } = &got[0] {
            assert_eq!(tile_pixels(&tiles[0]).unwrap(), rgb);
        } else {
            panic!("no es un cuadro");
        }
        assert_eq!(got[0], frame);
        assert_eq!(got[1], state);
    }

    #[test]
    fn largo_absurdo_es_error_y_mosaico_roto_no_se_dibuja() {
        let mut f = Framer::default();
        f.push(&u32::MAX.to_le_bytes());
        assert!(f.next_message().unwrap().is_err());
        let mut t = compress_tile(0, 0, 4, 4, &[9; 48]);
        t.w = 8; // no coincide con lo comprimido
        assert!(tile_pixels(&t).is_none());
        assert!(ToBrave::decode(&[200]).is_none());
        assert!(FromBrave::decode(&[0x81, 1]).is_none());
    }
}
