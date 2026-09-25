//! Gestor de ventanas: posición, tamaño, orden (qué ventana está encima), foco, minimizar,
//! maximizar, acoplar a un costado y el orden de "uso reciente" que usa Alt+Tab.
//!
//! Es pura geometría y estado: no dibuja ni sabe qué app hay adentro de cada ventana. Así se
//! prueba sola, y el escritorio (`desktop.rs`) compara el antes y el después de cada evento para
//! saber qué zonas de la pantalla hay que redibujar.
//!
//! ```text
//! ┌─[ícono] Título ─────────────────────────────────  —   ▢   ✕ ┐  ← barra de título (TITLE_H)
//! │                                                             │
//! │                      contenido de la app                    │
//! │                                                            ◢│  ← esquina para cambiar el tamaño
//! └─────────────────────────────────────────────────────────────┘
//! ```

use alloc::vec::Vec;

use jarvis_gfx::Rect;

pub type WinId = u32;

/// Alto de la barra de título.
pub const TITLE_H: i32 = 34;
/// Ancho de cada botón de la barra de título (como en Windows).
pub const BUTTON_W: i32 = 46;

/// Minimizar, maximizar y cerrar, relativos a la esquina de una ventana de ancho `w`. A la
/// derecha como en Windows, o a la izquierda (cerrar, minimizar, maximizar) como en macOS.
pub fn button_rects(w: i32) -> (Rect, Rect, Rect) {
    let at = |x: i32| Rect::new(x, 1, BUTTON_W, TITLE_H - 1);
    if crate::look::buttons_left() {
        (at(1 + BUTTON_W), at(1 + 2 * BUTTON_W), at(1))
    } else {
        let close = w - BUTTON_W - 1;
        (at(close - 2 * BUTTON_W), at(close - BUTTON_W), at(close))
    }
}
/// Zona de los bordes que sirve para cambiar el tamaño.
const GRIP: i32 = 8;
pub const MIN_W: i32 = 380;
/// Alto de la barra de arriba que aparece cuando hay una ventana maximizada (con los gráficos
/// de estado, la hora, la IP y las ventanas abiertas). Una ventana maximizada ocupa todo lo
/// demás de la pantalla.
pub const TOPBAR_H: i32 = 30;
pub const MIN_H: i32 = 240;

/// Qué parte de una ventana hay en un punto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Title,
    Minimize,
    Maximize,
    Close,
    /// Borde derecho, inferior o la esquina: cambiar el tamaño.
    Resize {
        right: bool,
        bottom: bool,
    },
    /// El contenido, en coordenadas relativas a la ventana.
    Content(i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WinId,
    /// Dónde está la ventana ahora (si está maximizada o acoplada, ocupa esa zona).
    pub rect: Rect,
    /// Zona a la que vuelve al "restaurar" (si está maximizada o acoplada).
    pub restore: Option<Rect>,
    pub maximized: bool,
    pub minimized: bool,
    /// En qué escritorio virtual está (Win+Ctrl+D crea otro).
    pub desktop: usize,
    /// Está en otro escritorio virtual (no se ve ni se puede tocar).
    pub away: bool,
}

impl Window {
    pub fn visible(&self) -> bool {
        !self.minimized && !self.away
    }

    /// Rectángulos de los botones: (minimizar, maximizar, cerrar).
    pub fn buttons(&self) -> (Rect, Rect, Rect) {
        let r = self.rect;
        let at = |b: Rect| Rect::new(r.x + b.x, r.y + b.y, b.w, b.h);
        let (min, max, close) = button_rects(r.w);
        (at(min), at(max), at(close))
    }

    /// Zona del contenido, relativa a la esquina de la ventana.
    pub fn content(&self) -> Rect {
        Rect::new(1, TITLE_H, self.rect.w - 2, self.rect.h - TITLE_H - 1)
    }

    pub fn part_at(&self, x: i32, y: i32) -> Option<Part> {
        let r = self.rect;
        if !r.contains(x, y) {
            return None;
        }
        let (min, max, close) = self.buttons();
        if close.contains(x, y) {
            return Some(Part::Close);
        }
        if max.contains(x, y) {
            return Some(Part::Maximize);
        }
        if min.contains(x, y) {
            return Some(Part::Minimize);
        }
        if !self.maximized {
            let right = x >= r.x + r.w - GRIP;
            let bottom = y >= r.y + r.h - GRIP;
            if (right || bottom) && y >= r.y + TITLE_H {
                return Some(Part::Resize { right, bottom });
            }
        }
        if y < r.y + TITLE_H {
            return Some(Part::Title);
        }
        Some(Part::Content(x - r.x, y - r.y))
    }
}

pub struct WindowManager {
    /// De atrás hacia adelante: la última es la que está encima.
    windows: Vec<Window>,
    /// Orden de uso, la más reciente primero (Alt+Tab).
    mru: Vec<WinId>,
    focus: Option<WinId>,
    /// La zona de la pantalla donde se acoplan las ventanas (debajo de la barra de íconos).
    work: Rect,
    /// Donde va una ventana maximizada: toda la pantalla menos la barra de arriba.
    full: Rect,
    screen: Rect,
    next_id: WinId,
    /// Ventanas que minimizó "mostrar el escritorio" (Win+D), para volver a mostrarlas.
    peeked: Vec<WinId>,
    cascade: i32,
    /// Escritorio virtual actual y cuántos hay.
    current: usize,
    desktops: usize,
}

impl WindowManager {
    pub fn new(screen: Rect, work: Rect) -> Self {
        WindowManager {
            windows: Vec::new(),
            mru: Vec::new(),
            focus: None,
            work,
            full: Rect::new(screen.x, screen.y + TOPBAR_H, screen.w, screen.h - TOPBAR_H),
            screen,
            next_id: 1,
            peeked: Vec::new(),
            cascade: 0,
            current: 0,
            desktops: 1,
        }
    }

    pub fn work_area(&self) -> Rect {
        self.work
    }

    /// De atrás hacia adelante.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn get(&self, id: WinId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn get_mut(&mut self, id: WinId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn focused(&self) -> Option<WinId> {
        self.focus
    }

    /// Orden de Alt+Tab: la de uso más reciente primero (incluye las minimizadas).
    pub fn mru(&self) -> &[WinId] {
        &self.mru
    }

    /// Como [`mru`](Self::mru), pero solo las del escritorio virtual actual (Alt+Tab).
    pub fn mru_here(&self) -> Vec<WinId> {
        self.mru
            .iter()
            .copied()
            .filter(|&id| self.get(id).is_some_and(|w| !w.away))
            .collect()
    }

    /// (escritorio actual, cuántos hay), contando desde 0.
    pub fn desktops(&self) -> (usize, usize) {
        (self.current, self.desktops)
    }

    /// Win+Ctrl+D: un escritorio virtual nuevo, vacío, y se pasa a él.
    pub fn new_desktop(&mut self) {
        self.desktops += 1;
        self.switch_desktop(self.desktops - 1);
    }

    /// Win+Ctrl+← / →.
    pub fn switch_desktop(&mut self, n: usize) {
        if n >= self.desktops {
            return;
        }
        self.current = n;
        for w in &mut self.windows {
            w.away = w.desktop != n;
        }
        self.peeked.clear();
        self.focus_top();
    }

    /// Win+Ctrl+F4: cierra el escritorio actual; sus ventanas pasan al de la izquierda.
    pub fn close_desktop(&mut self) {
        if self.desktops <= 1 {
            return;
        }
        let closing = self.current;
        let target = closing.saturating_sub(1);
        for w in &mut self.windows {
            if w.desktop == closing {
                w.desktop = target;
            } else if w.desktop > closing {
                w.desktop -= 1;
            }
        }
        self.desktops -= 1;
        self.switch_desktop(target);
    }

    /// Win+Home: minimiza todas menos la que tiene el foco.
    pub fn minimize_others(&mut self) {
        let keep = self.focus;
        for w in &mut self.windows {
            if Some(w.id) != keep && !w.away {
                w.minimized = true;
            }
        }
    }

    /// Win+Shift+M: vuelve a mostrar las minimizadas.
    pub fn restore_all(&mut self) {
        for w in &mut self.windows {
            if !w.away {
                w.minimized = false;
            }
        }
        self.peeked.clear();
        self.focus_top();
    }

    /// Win+Shift+↑: estira la ventana de arriba a abajo (mismo ancho).
    pub fn stretch_vertical(&mut self, id: WinId) {
        let work = self.work;
        let Some(w) = self.get_mut(id) else { return };
        if w.restore.is_none() {
            w.restore = Some(w.rect);
        }
        w.maximized = false;
        w.rect.y = work.y;
        w.rect.h = work.h;
    }

    /// Un lugar razonable para una ventana nueva de `w` × `h`: centrada en la zona de trabajo y
    /// corrida un poco respecto de la anterior (en cascada), como hace Windows.
    pub fn place(&mut self, w: i32, h: i32) -> Rect {
        let w = w.min(self.work.w).max(MIN_W.min(self.work.w));
        let h = h.min(self.work.h).max(MIN_H.min(self.work.h));
        let offset = self.cascade * 28;
        self.cascade = (self.cascade + 1) % 6;
        let x = self.work.x + (self.work.w - w) / 2 + offset - 40;
        let y = self.work.y + (self.work.h - h) / 2 + offset - 40;
        Rect::new(
            x.clamp(self.work.x, self.work.x + self.work.w - w),
            y.clamp(self.work.y, self.work.y + self.work.h - h),
            w,
            h,
        )
    }

    /// Abre una ventana encima de todas y con el foco.
    pub fn open(&mut self, rect: Rect) -> WinId {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push(Window {
            id,
            rect,
            restore: None,
            maximized: false,
            minimized: false,
            desktop: self.current,
            away: false,
        });
        self.set_focus(Some(id));
        self.peeked.clear();
        id
    }

    pub fn close(&mut self, id: WinId) {
        self.windows.retain(|w| w.id != id);
        self.mru.retain(|&m| m != id);
        self.peeked.retain(|&m| m != id);
        if self.focus == Some(id) {
            self.focus_top();
        }
    }

    fn set_focus(&mut self, id: Option<WinId>) {
        self.focus = id;
        if let Some(id) = id {
            self.mru.retain(|&m| m != id);
            self.mru.insert(0, id);
        }
    }

    /// Le da el foco a la ventana visible de más arriba (o a ninguna: el escritorio).
    fn focus_top(&mut self) {
        let top = self
            .windows
            .iter()
            .rev()
            .find(|w| w.visible())
            .map(|w| w.id);
        self.focus = top;
        if let Some(id) = top {
            self.set_focus(Some(id));
        }
    }

    /// Trae la ventana adelante, la restaura si estaba minimizada y le da el foco.
    pub fn activate(&mut self, id: WinId) {
        let Some(pos) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        let desktop = self.windows[pos].desktop;
        if desktop != self.current {
            self.switch_desktop(desktop);
        }
        let Some(pos) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        let mut w = self.windows.remove(pos);
        w.minimized = false;
        self.windows.push(w);
        self.set_focus(Some(id));
        self.peeked.clear();
    }

    /// Nadie tiene el foco: las teclas van al escritorio (JARVIS).
    pub fn focus_desktop(&mut self) {
        self.focus = None;
    }

    pub fn minimize(&mut self, id: WinId) {
        if let Some(w) = self.get_mut(id) {
            w.minimized = true;
        }
        if self.focus == Some(id) {
            self.focus_top();
        }
    }

    /// Hasta dónde llega una ventana maximizada: toda la pantalla menos la barra de arriba, o
    /// (sin la barra de arriba) la zona de trabajo, debajo de la barra de íconos.
    pub fn set_topbar(&mut self, topbar: bool) {
        let full = if topbar {
            Rect::new(
                self.work.x,
                TOPBAR_H,
                self.work.w,
                self.work.y + self.work.h - TOPBAR_H,
            )
        } else {
            self.work
        };
        if full == self.full {
            return;
        }
        self.full = full;
        for w in &mut self.windows {
            if w.maximized {
                w.rect = full;
            }
        }
    }

    /// ¿Hay una ventana maximizada a la vista? Entonces se muestra la barra de arriba.
    pub fn any_maximized(&self) -> bool {
        self.windows.iter().any(|w| w.visible() && w.maximized)
    }

    pub fn toggle_maximize(&mut self, id: WinId) {
        let work = self.full;
        let Some(w) = self.get_mut(id) else { return };
        if w.maximized {
            w.maximized = false;
            if let Some(r) = w.restore.take() {
                w.rect = r;
            }
        } else {
            if w.restore.is_none() {
                w.restore = Some(w.rect);
            }
            w.maximized = true;
            w.rect = work;
        }
        self.activate(id);
    }

    /// Win+↓: si está maximizada o acoplada, vuelve a su tamaño; si no, se minimiza.
    pub fn restore_or_minimize(&mut self, id: WinId) {
        let Some(w) = self.get_mut(id) else { return };
        if let Some(r) = w.restore.take() {
            w.maximized = false;
            w.rect = r;
        } else {
            self.minimize(id);
        }
    }

    /// Win+← / Win+→: ocupa la mitad de la pantalla. Si ya estaba acoplada del otro lado,
    /// vuelve a su tamaño (como en Windows).
    pub fn snap(&mut self, id: WinId, side: Side) {
        let work = self.work;
        let half = work.w / 2;
        let target = match side {
            Side::Left => Rect::new(work.x, work.y, half, work.h),
            Side::Right => Rect::new(work.x + work.w - half, work.y, half, work.h),
        };
        let Some(w) = self.get_mut(id) else { return };
        let other_side = w.restore.is_some() && !w.maximized && w.rect != target;
        if other_side {
            if let Some(r) = w.restore.take() {
                w.rect = r;
            }
        } else {
            if w.restore.is_none() {
                w.restore = Some(w.rect);
            }
            w.maximized = false;
            w.rect = target;
        }
        self.activate(id);
    }

    /// Mueve la ventana (arrastrando la barra de título). La barra siempre queda a la vista.
    /// Si estaba maximizada o acoplada, al arrastrarla recupera su tamaño normal.
    pub fn move_to(&mut self, id: WinId, x: i32, y: i32) {
        let screen = self.screen;
        let Some(w) = self.get_mut(id) else { return };
        if let Some(r) = w.restore.take() {
            w.rect.w = r.w;
            w.rect.h = r.h;
            w.maximized = false;
        }
        w.rect.x = x.clamp(screen.x - w.rect.w + 120, screen.x + screen.w - 120);
        w.rect.y = y.clamp(screen.y, screen.y + screen.h - TITLE_H);
    }

    pub fn resize(&mut self, id: WinId, w: i32, h: i32) {
        let Some(win) = self.get_mut(id) else { return };
        win.rect.w = w.max(MIN_W);
        win.rect.h = h.max(MIN_H);
        win.restore = None;
        win.maximized = false;
    }

    /// Win+D: minimiza todo; la segunda vez vuelve a mostrar lo que minimizó.
    pub fn toggle_desktop(&mut self) {
        if self.peeked.is_empty() {
            let shown: Vec<WinId> = self
                .windows
                .iter()
                .filter(|w| w.visible())
                .map(|w| w.id)
                .collect();
            for &id in &shown {
                if let Some(w) = self.get_mut(id) {
                    w.minimized = true;
                }
            }
            self.focus = None;
            self.peeked = shown;
        } else {
            let peeked = core::mem::take(&mut self.peeked);
            for &id in &peeked {
                if let Some(w) = self.get_mut(id) {
                    w.minimized = false;
                }
            }
            self.focus_top();
        }
    }

    /// Win+M: minimiza todo (sin recordar qué).
    pub fn minimize_all(&mut self) {
        for w in &mut self.windows {
            if !w.away {
                w.minimized = true;
            }
        }
        self.focus = None;
        self.peeked.clear();
    }

    /// La ventana visible de más arriba en (x, y) y qué parte de ella.
    pub fn at(&self, x: i32, y: i32) -> Option<(WinId, Part)> {
        self.windows
            .iter()
            .rev()
            .filter(|w| w.visible())
            .find_map(|w| w.part_at(x, y).map(|p| (w.id, p)))
    }

    /// ¿Alguna ventana visible tapa por completo `r`? (Para no animar la esfera si no se ve.)
    pub fn covers(&self, r: &Rect) -> bool {
        self.windows.iter().any(|w| w.visible() && w.rect.covers(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1280, 800);
    const WORK: Rect = Rect::new(0, 80, 1280, 720);

    fn wm_with(n: usize) -> (WindowManager, Vec<WinId>) {
        let mut wm = WindowManager::new(SCREEN, WORK);
        let ids = (0..n)
            .map(|i| wm.open(Rect::new(100 + i as i32 * 10, 150, 600, 400)))
            .collect();
        (wm, ids)
    }

    #[test]
    fn la_ultima_abierta_esta_encima_y_tiene_el_foco() {
        let (wm, ids) = wm_with(3);
        assert_eq!(wm.focused(), Some(ids[2]));
        assert_eq!(wm.windows().last().unwrap().id, ids[2]);
        assert_eq!(wm.mru(), &[ids[2], ids[1], ids[0]]);
        // En (105, 200) se superponen las tres: gana la de arriba.
        assert!(matches!(wm.at(125, 200), Some((id, Part::Content(..))) if id == ids[2]));
    }

    #[test]
    fn minimizar_pasa_el_foco_y_activar_restaura() {
        let (mut wm, ids) = wm_with(2);
        wm.minimize(ids[1]);
        assert_eq!(wm.focused(), Some(ids[0]));
        assert!(!wm.get(ids[1]).unwrap().visible());
        wm.activate(ids[1]);
        assert!(wm.get(ids[1]).unwrap().visible());
        assert_eq!(wm.focused(), Some(ids[1]));
    }

    #[test]
    fn maximizar_y_restaurar_vuelve_al_tamanio_original() {
        let (mut wm, ids) = wm_with(1);
        let before = wm.get(ids[0]).unwrap().rect;
        wm.toggle_maximize(ids[0]);
        // Toda la pantalla, menos la barra de arriba.
        assert_eq!(
            wm.get(ids[0]).unwrap().rect,
            Rect::new(0, TOPBAR_H, 1280, 800 - TOPBAR_H)
        );
        assert!(wm.any_maximized());
        assert!(wm.covers(&Rect::new(400, 300, 100, 100)));
        wm.toggle_maximize(ids[0]);
        assert_eq!(wm.get(ids[0]).unwrap().rect, before);
        assert!(!wm.any_maximized());
    }

    #[test]
    fn acoplar_a_los_costados_y_volver() {
        let (mut wm, ids) = wm_with(1);
        let before = wm.get(ids[0]).unwrap().rect;
        wm.snap(ids[0], Side::Left);
        assert_eq!(wm.get(ids[0]).unwrap().rect, Rect::new(0, 80, 640, 720));
        // Win+→ estando a la izquierda: vuelve a su lugar (como en Windows).
        wm.snap(ids[0], Side::Right);
        assert_eq!(wm.get(ids[0]).unwrap().rect, before);
        wm.snap(ids[0], Side::Right);
        assert_eq!(wm.get(ids[0]).unwrap().rect, Rect::new(640, 80, 640, 720));
        wm.restore_or_minimize(ids[0]);
        assert_eq!(wm.get(ids[0]).unwrap().rect, before);
        wm.restore_or_minimize(ids[0]);
        assert!(wm.get(ids[0]).unwrap().minimized);
    }

    #[test]
    fn mostrar_escritorio_es_un_interruptor() {
        let (mut wm, ids) = wm_with(3);
        wm.minimize(ids[0]);
        wm.toggle_desktop();
        assert!(wm.windows().iter().all(|w| w.minimized));
        assert_eq!(wm.focused(), None);
        wm.toggle_desktop();
        // Vuelven las que estaban a la vista; la que ya estaba minimizada sigue igual.
        assert!(wm.get(ids[0]).unwrap().minimized);
        assert!(wm.get(ids[1]).unwrap().visible() && wm.get(ids[2]).unwrap().visible());
        assert_eq!(wm.focused(), Some(ids[2]));
    }

    #[test]
    fn botones_y_bordes() {
        let (wm, ids) = wm_with(1);
        let w = wm.get(ids[0]).unwrap();
        let (min, max, close) = w.buttons();
        assert_eq!(w.part_at(close.x + 5, close.y + 5), Some(Part::Close));
        assert_eq!(w.part_at(max.x + 5, max.y + 5), Some(Part::Maximize));
        assert_eq!(w.part_at(min.x + 5, min.y + 5), Some(Part::Minimize));
        assert_eq!(w.part_at(w.rect.x + 50, w.rect.y + 5), Some(Part::Title));
        assert_eq!(
            w.part_at(w.rect.x + w.rect.w - 2, w.rect.y + w.rect.h - 2),
            Some(Part::Resize {
                right: true,
                bottom: true
            })
        );
        assert_eq!(
            w.part_at(w.rect.x + 10, w.rect.y + 50),
            Some(Part::Content(10, 50))
        );
    }

    #[test]
    fn cerrar_y_mover() {
        let (mut wm, ids) = wm_with(2);
        wm.close(ids[1]);
        assert_eq!(wm.windows().len(), 1);
        assert_eq!(wm.focused(), Some(ids[0]));
        wm.move_to(ids[0], -5000, -40);
        let r = wm.get(ids[0]).unwrap().rect;
        assert!(
            r.x + r.w >= 120 && r.y == 0,
            "la barra de título queda a la vista: {r:?}"
        );
        wm.resize(ids[0], 10, 10);
        let r = wm.get(ids[0]).unwrap().rect;
        assert_eq!((r.w, r.h), (MIN_W, MIN_H));
    }

    #[test]
    fn escritorios_virtuales() {
        let (mut wm, ids) = wm_with(2);
        wm.new_desktop();
        assert_eq!(wm.desktops(), (1, 2));
        assert_eq!(wm.focused(), None, "el escritorio nuevo está vacío");
        assert!(wm.windows().iter().all(|w| !w.visible()));
        let c = wm.open(Rect::new(0, 100, 500, 400));
        assert_eq!(wm.mru_here(), [c]);
        wm.switch_desktop(0);
        assert_eq!(wm.focused(), Some(ids[1]));
        assert!(!wm.get(c).unwrap().visible());
        // Activar una ventana de otro escritorio lleva a ese escritorio.
        wm.activate(c);
        assert_eq!(wm.desktops(), (1, 2));
        // Cerrar el escritorio: sus ventanas pasan al de la izquierda.
        wm.close_desktop();
        assert_eq!(wm.desktops(), (0, 1));
        assert!(wm.windows().iter().all(|w| w.visible()));
        wm.minimize_others();
        assert_eq!(wm.windows().iter().filter(|w| w.visible()).count(), 1);
        wm.restore_all();
        assert_eq!(wm.windows().iter().filter(|w| w.visible()).count(), 3);
    }
}
