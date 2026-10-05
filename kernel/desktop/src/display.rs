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

/// Las resoluciones para elegir en Configuración → Pantallas (ancho, alto y proporción). Además
/// está la automática ([`AUTO`]): la que informa el monitor.
pub const RESOLUTIONS: [(u32, u32, &str); 20] = [
    (800, 600, "4:3"),
    (1024, 768, "4:3"),
    (1152, 864, "4:3"),
    (1280, 720, "16:9"),
    (1280, 800, "16:10"),
    (1280, 960, "4:3"),
    (1280, 1024, "5:4"),
    (1366, 768, "16:9"),
    (1440, 900, "16:10"),
    (1600, 900, "16:9"),
    (1600, 1200, "4:3"),
    (1680, 1050, "16:10"),
    (1920, 1080, "16:9"),
    (1920, 1200, "16:10"),
    (2560, 1080, "21:9"),
    (2560, 1440, "16:9"),
    (2560, 1600, "16:10"),
    (3440, 1440, "21:9"),
    (3840, 2160, "16:9"),
    (4096, 2160, "17:9"),
];

/// La resolución que informa el monitor.
pub const AUTO: (u32, u32) = (0, 0);

/// La más grande de la lista (para reservar la memoria).
pub const MAX_RESOLUTION: (u32, u32) = (4096, 2160);

/// `auto` o `ANCHOxALTO` de la lista (otra cosa: automática).
pub fn parse_resolution(v: &str) -> (u32, u32) {
    let Some((w, h)) = v.trim().split_once('x') else {
        return AUTO;
    };
    let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) else {
        return AUTO;
    };
    if RESOLUTIONS.iter().any(|r| (r.0, r.1) == (w, h)) {
        (w, h)
    } else {
        AUTO
    }
}

pub fn resolution_code(r: (u32, u32)) -> alloc::string::String {
    if r == AUTO {
        "auto".into()
    } else {
        alloc::format!("{}x{}", r.0, r.1)
    }
}

/// La resolución siguiente (`forward`) o la anterior: automática, después la lista.
pub fn cycle_resolution(r: (u32, u32), forward: bool) -> (u32, u32) {
    let n = RESOLUTIONS.len() as i32 + 1;
    let pos = RESOLUTIONS
        .iter()
        .position(|x| (x.0, x.1) == r)
        .map_or(0, |i| i as i32 + 1);
    let next = (pos + if forward { 1 } else { -1 }).rem_euclid(n);
    if next == 0 {
        AUTO
    } else {
        let x = RESOLUTIONS[next as usize - 1];
        (x.0, x.1)
    }
}

/// Los monitores con la resolución elegida: la del principal (o la del único que se ve; al
/// duplicar, la de los dos). Automática: como los informa la placa.
pub fn with_resolution(
    outputs: &[(u32, u32)],
    res: (u32, u32),
    mode: Mode,
    primary: usize,
) -> Vec<(u32, u32)> {
    let mut v = outputs.to_vec();
    if res == AUTO || v.is_empty() {
        return v;
    }
    let k = if v.len() < 2 {
        0
    } else {
        match mode {
            Mode::OnlyFirst => 0,
            Mode::OnlySecond => 1,
            Mode::Duplicate => {
                v.fill(res);
                return v;
            }
            Mode::Extend => primary.min(1),
        }
    };
    v[k] = res;
    v
}

/// Los píxeles a reservar al arrancar: para la resolución más grande de la lista que entre en
/// `budget` bytes (las tres imágenes del escritorio, en cualquier modo); si no entra ninguna,
/// solo para las de los monitores.
pub fn capacity(outputs: &[(u32, u32)], budget: usize) -> usize {
    let native = max_pixels(outputs);
    RESOLUTIONS
        .iter()
        .rev()
        .map(|&(rw, rh, _)| {
            let big: Vec<(u32, u32)> = outputs
                .iter()
                .map(|&(w, h)| (w.max(rw), h.max(rh)))
                .collect();
            max_pixels(&big)
        })
        .find(|px| px * 4 * 3 <= budget)
        .unwrap_or(native)
        .max(native)
}

/// ¿Entra la resolución `res` en `capacity` píxeles con estos monitores y este modo? (0: sin
/// tope, como en los tests).
pub fn fits(
    outputs: &[(u32, u32)],
    res: (u32, u32),
    mode: Mode,
    vertical: bool,
    primary: usize,
    capacity: usize,
) -> bool {
    if capacity == 0 || res == AUTO {
        return true;
    }
    let l = layout(
        &with_resolution(outputs, res, mode, primary),
        mode,
        vertical,
        primary,
    );
    (l.size.0 as usize) * (l.size.1 as usize) <= capacity
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

    #[test]
    fn resoluciones() {
        assert_eq!(parse_resolution("1920x1080"), (1920, 1080));
        assert_eq!(parse_resolution("auto"), AUTO);
        assert_eq!(parse_resolution("1234x567"), AUTO, "solo las de la lista");
        assert_eq!(resolution_code((2560, 1440)), "2560x1440");
        assert_eq!(cycle_resolution(AUTO, true), (800, 600));
        assert_eq!(cycle_resolution(AUTO, false), MAX_RESOLUTION);
        assert_eq!(cycle_resolution(MAX_RESOLUTION, true), AUTO);
        assert_eq!(cycle_resolution((1280, 800), true), (1280, 960));
        let mut r = AUTO;
        for _ in 0..=RESOLUTIONS.len() {
            r = cycle_resolution(r, true);
        }
        assert_eq!(r, AUTO, "da la vuelta entera");
        // Un monitor solo, el principal, los dos al duplicar, el que se ve.
        assert_eq!(
            with_resolution(&[(1280, 800)], AUTO, Mode::Extend, 0),
            [(1280, 800)]
        );
        assert_eq!(
            with_resolution(&[(1280, 800)], (1920, 1080), Mode::Extend, 0),
            [(1920, 1080)]
        );
        let r = (1600, 900);
        assert_eq!(with_resolution(&TWO, r, Mode::Extend, 1), [(1280, 800), r]);
        assert_eq!(with_resolution(&TWO, r, Mode::Duplicate, 0), [r, r]);
        assert_eq!(
            with_resolution(&TWO, r, Mode::OnlySecond, 0),
            [(1280, 800), r]
        );
        // La memoria: con lugar, hasta la más grande; sin lugar, la de los monitores.
        assert!(capacity(&[(1280, 800)], usize::MAX) >= 4096 * 2160);
        assert_eq!(capacity(&[(1280, 800)], 1024), 1280 * 800);
        // Con 64 MiB: hasta 3440×1440 (las tres imágenes), no 4K.
        let px = capacity(&[(1280, 800)], 64 << 20);
        assert_eq!(px, 3440 * 1440);
        let one = [(1280, 800)];
        assert!(fits(&one, (3440, 1440), Mode::Extend, false, 0, px));
        assert!(!fits(&one, (3840, 2160), Mode::Extend, false, 0, px));
        assert!(fits(&one, AUTO, Mode::Extend, false, 0, 1));
    }
}
