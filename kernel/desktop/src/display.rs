//! Varios monitores: cómo se reparte el escritorio entre las salidas de la placa de video.
//!
//! El escritorio es **una sola imagen** (el `frame` que compone todo). Cada salida muestra un
//! rectángulo de esa imagen (virtio-gpu: `SET_SCANOUT`). Con eso salen todos los modos:
//!
//! - **Extender**: la imagen mide los dos monitores juntos (uno al lado del otro, o uno arriba
//!   del otro) y cada salida muestra su parte. Las ventanas se pueden llevar de uno al otro.
//! - **Duplicar**: la imagen mide lo que el monitor más chico y las dos salidas muestran lo mismo.
//! - **Solo uno**: la imagen mide lo que ese monitor; la otra salida se apaga.
//!
//! El monitor **principal** (el de la barra de íconos, JARVIS y la barra de arriba) siempre queda
//! en la esquina (0, 0) de la imagen: así todo lo que se dibuja "en la pantalla" sigue usando las
//! mismas cuentas que con un solo monitor. Por eso "el principal" se elige cambiando qué salida va
//! primero, y la otra queda a su derecha o abajo.

use alloc::vec;
use alloc::vec::Vec;

use jarvis_gfx::Rect;

/// Qué hacer con dos monitores (Win+P, como en Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Extend,
    Duplicate,
    OnlyFirst,
    OnlySecond,
}

impl Mode {
    pub const ALL: [Mode; 4] = [
        Mode::OnlyFirst,
        Mode::Duplicate,
        Mode::Extend,
        Mode::OnlySecond,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Mode::Extend => "extender",
            Mode::Duplicate => "duplicar",
            Mode::OnlyFirst => "solo1",
            Mode::OnlySecond => "solo2",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Extend => "Extender",
            Mode::Duplicate => "Duplicar",
            Mode::OnlyFirst => "Solo la pantalla 1",
            Mode::OnlySecond => "Solo la pantalla 2",
        }
    }
}

/// Cómo quedó repartido: el tamaño de la imagen, los monitores en coordenadas del escritorio
/// (el principal primero) y qué rectángulo de la imagen muestra cada salida (`None` = apagada).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub size: (u32, u32),
    pub screens: Vec<Rect>,
    pub scanouts: Vec<Option<Rect>>,
}

/// Reparte el escritorio. `outputs`: el tamaño de cada salida (monitor). `vertical`: el segundo
/// va abajo del principal (si no, a la derecha). `primary`: qué salida es la principal.
pub fn layout(outputs: &[(u32, u32)], mode: Mode, vertical: bool, primary: usize) -> Layout {
    let rect = |w: u32, h: u32| Rect::new(0, 0, w as i32, h as i32);
    let mut scanouts = vec![None; outputs.len()];
    if outputs.len() < 2 {
        let (w, h) = outputs.first().copied().unwrap_or((1280, 800));
        if !scanouts.is_empty() {
            scanouts[0] = Some(rect(w, h));
        }
        return Layout {
            size: (w, h),
            screens: vec![rect(w, h)],
            scanouts,
        };
    }
    let primary = primary.min(1);
    let other = 1 - primary;
    let (pw, ph) = outputs[primary];
    let (ow, oh) = outputs[other];
    match mode {
        Mode::Extend => {
            let main = rect(pw, ph);
            let second = if vertical {
                Rect::new(0, ph as i32, ow as i32, oh as i32)
            } else {
                Rect::new(pw as i32, 0, ow as i32, oh as i32)
            };
            let all = main.union(&second);
            scanouts[primary] = Some(main);
            scanouts[other] = Some(second);
            Layout {
                size: (all.w as u32, all.h as u32),
                screens: vec![main, second],
                scanouts,
            }
        }
        Mode::Duplicate => {
            let (w, h) = (pw.min(ow), ph.min(oh));
            scanouts[0] = Some(rect(w, h));
            scanouts[1] = Some(rect(w, h));
            Layout {
                size: (w, h),
                screens: vec![rect(w, h)],
                scanouts,
            }
        }
        Mode::OnlyFirst | Mode::OnlySecond => {
            let k = if mode == Mode::OnlyFirst { 0 } else { 1 };
            let (w, h) = outputs[k];
            scanouts[k] = Some(rect(w, h));
            Layout {
                size: (w, h),
                screens: vec![rect(w, h)],
                scanouts,
            }
        }
    }
}

/// El tamaño más grande que puede llegar a pedir `layout` (para reservar la memoria una vez).
pub fn max_pixels(outputs: &[(u32, u32)]) -> usize {
    let sum_w: u64 = outputs.iter().map(|o| o.0 as u64).sum();
    let sum_h: u64 = outputs.iter().map(|o| o.1 as u64).sum();
    let max_w = outputs.iter().map(|o| o.0 as u64).max().unwrap_or(0);
    let max_h = outputs.iter().map(|o| o.1 as u64).max().unwrap_or(0);
    (sum_w * max_h).max(max_w * sum_h) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: [(u32, u32); 2] = [(1280, 800), (1024, 768)];

    #[test]
    fn extender_al_costado_y_abajo() {
        let l = layout(&TWO, Mode::Extend, false, 0);
        assert_eq!(l.size, (2304, 800));
        assert_eq!(l.screens[1], Rect::new(1280, 0, 1024, 768));
        assert_eq!(l.scanouts[1], Some(Rect::new(1280, 0, 1024, 768)));
        let l = layout(&TWO, Mode::Extend, true, 0);
        assert_eq!(l.size, (1280, 1568));
        assert_eq!(l.screens[1], Rect::new(0, 800, 1024, 768));
    }

    #[test]
    fn el_principal_siempre_arranca_en_cero() {
        let l = layout(&TWO, Mode::Extend, false, 1);
        assert_eq!(l.screens[0], Rect::new(0, 0, 1024, 768));
        // La salida 2 (índice 1) muestra el principal, la 1 lo que está a la derecha.
        assert_eq!(l.scanouts[1], Some(Rect::new(0, 0, 1024, 768)));
        assert_eq!(l.scanouts[0], Some(Rect::new(1024, 0, 1280, 800)));
    }

    #[test]
    fn duplicar_y_uno_solo() {
        let l = layout(&TWO, Mode::Duplicate, false, 0);
        assert_eq!(l.size, (1024, 768));
        assert_eq!(l.scanouts, vec![Some(Rect::new(0, 0, 1024, 768)); 2]);
        let l = layout(&TWO, Mode::OnlySecond, false, 0);
        assert_eq!(l.size, (1024, 768));
        assert_eq!(l.scanouts, vec![None, Some(Rect::new(0, 0, 1024, 768))]);
        let l = layout(&[(800, 600)], Mode::Extend, false, 0);
        assert_eq!((l.size, l.screens.len()), ((800, 600), 1));
        assert!(max_pixels(&TWO) >= 2304 * 800);
    }
}
