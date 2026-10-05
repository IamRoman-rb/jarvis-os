//! El asistente de instalación: lo primero que se ve al arrancar en vivo (desde la ISO o un
//! pendrive sin disco de JARVIS).
//!
//! JARVIS pide el nombre de usuario, una contraseña (dos veces) y el disco vacío donde instalar;
//! con eso el kernel copia el sistema y escribe en el disco nuevo la configuración con el
//! usuario y la contraseña hasheada (ver [`crate::pin`]). Al reiniciar desde el disco, la
//! pantalla de inicio de sesión pide esa contraseña y JARVIS presenta el sistema
//! ([`TOUR`]).
//!
//! Es una máquina de estados sin dibujo ni hardware: [`Setup::key`] y [`Setup::click`] devuelven
//! una [`Action`] que el escritorio ejecuta (pedirle la instalación al kernel, reiniciar o salir
//! al modo en vivo). [`draw`] la pinta a pantalla completa.

use alloc::string::String;
use alloc::vec::Vec;

use jarvis_gfx::shapes::{circle, rounded_outline, rounded_rect};
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::config::valid_name;
use crate::i18n::{tr, trf};
use crate::input::Key;
use crate::system::{DiskInfo, InstallState};
use crate::text_input::TextInput;
use crate::widgets::{big, button, field_bg, label, light, s16, selected_bg};

/// La contraseña: entre 4 y 64 caracteres.
pub const MIN_PASSWORD: usize = 4;
pub const MAX_PASSWORD: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    User,
    Password,
    Disk,
    Confirm,
    Installing,
    Done(Result<String, String>),
}

/// Lo que el escritorio tiene que hacer después de una tecla o un clic.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Cambió algo en pantalla.
    Redraw,
    /// "Probar sin instalar": seguir en el modo en vivo.
    Leave,
    /// Instalar en el disco número `disk` de `SystemStats::disks`.
    Install {
        disk: usize,
        user: String,
        password: String,
    },
    Reboot,
}

pub struct Setup {
    pub step: Step,
    user: TextInput,
    password: TextInput,
    repeat: TextInput,
    /// En el paso de la contraseña: escribiendo la repetición.
    on_repeat: bool,
    /// El disco elegido (índice en la lista de [`targets`]).
    sel: usize,
    error: Option<&'static str>,
}

/// Los discos donde se puede instalar (índices en `disks`): vacíos y que no sean el medio de
/// arranque.
pub fn targets(disks: &[DiskInfo]) -> Vec<usize> {
    disks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.blank && !d.boot_medium)
        .map(|(i, _)| i)
        .collect()
}

impl Default for Setup {
    fn default() -> Self {
        Self::new()
    }
}

impl Setup {
    pub fn new() -> Setup {
        Setup {
            step: Step::Welcome,
            user: TextInput::new("", 24),
            password: TextInput::new("", MAX_PASSWORD),
            repeat: TextInput::new("", MAX_PASSWORD),
            on_repeat: false,
            sel: 0,
            error: None,
        }
    }

    pub fn user(&self) -> &str {
        &self.user.text
    }

    /// Número de paso para mostrar (1–4) en los que piden algo.
    fn number(&self) -> Option<u8> {
        match self.step {
            Step::Welcome => None,
            Step::User => Some(1),
            Step::Password => Some(2),
            Step::Disk => Some(3),
            Step::Confirm => Some(4),
            Step::Installing | Step::Done(_) => None,
        }
    }

    fn go(&mut self, step: Step) -> Action {
        self.step = step;
        self.error = None;
        Action::Redraw
    }

    fn fail(&mut self, msg: &'static str) -> Action {
        self.error = Some(msg);
        Action::Redraw
    }

    /// Lo que dice JARVIS en cada paso.
    pub fn jarvis_says(&self) -> String {
        match &self.step {
            Step::Welcome => tr(
                "Hola, soy JARVIS. Voy a instalar JARVIS-OS en esta computadora.",
            )
            .into(),
            Step::User => tr("¿Cómo te llamo? Elegí un nombre de usuario.").into(),
            Step::Password => trf(
                "Encantado, {}. Ahora una contraseña para proteger tu sesión.",
                &[&capitalized(&self.user.text)],
            ),
            Step::Disk => tr("¿En qué disco instalo? Solo uso discos vacíos.").into(),
            Step::Confirm => tr("Último paso. Confirmá y empiezo a copiar el sistema.").into(),
            Step::Installing => tr("Instalando JARVIS-OS. Esto tarda un minuto...").into(),
            Step::Done(Ok(_)) => tr(
                "Listo. Reiniciá (si arrancaste de un pendrive, sacalo antes): te espero del otro lado.",
            )
            .into(),
            Step::Done(Err(_)) => tr("Algo salió mal. Podés elegir otro disco.").into(),
        }
    }

    /// El kernel cuenta cómo va la instalación.
    pub fn install_state(&mut self, state: &InstallState) -> Action {
        match (state, &self.step) {
            (InstallState::Done(r), Step::Installing) => self.go(Step::Done(r.clone())),
            _ => Action::None,
        }
    }

    pub fn key(&mut self, key: Key, disks: &[DiskInfo]) -> Action {
        match self.step.clone() {
            Step::Welcome => match key {
                Key::Enter => self.go(Step::User),
                Key::Escape => Action::Leave,
                _ => Action::None,
            },
            Step::User => match key {
                Key::Enter => {
                    if valid_name(&self.user.text) {
                        self.go(Step::Password)
                    } else {
                        self.fail("Usá letras, números, - o _ (hasta 24).")
                    }
                }
                Key::Escape => self.go(Step::Welcome),
                k => {
                    if self.user.handle(k) {
                        self.error = None;
                        Action::Redraw
                    } else {
                        Action::None
                    }
                }
            },
            Step::Password => match key {
                Key::Tab | Key::Down | Key::Up => {
                    self.on_repeat = !self.on_repeat;
                    Action::Redraw
                }
                Key::Enter if !self.on_repeat => {
                    if self.password.text.chars().count() < MIN_PASSWORD {
                        return self.fail("La contraseña necesita al menos 4 caracteres.");
                    }
                    self.on_repeat = true;
                    self.error = None;
                    Action::Redraw
                }
                Key::Enter => {
                    if self.password.text.chars().count() < MIN_PASSWORD {
                        self.on_repeat = false;
                        return self.fail("La contraseña necesita al menos 4 caracteres.");
                    }
                    if self.password.text != self.repeat.text {
                        self.repeat.text.clear();
                        return self.fail("Las contraseñas no coinciden. Repetila.");
                    }
                    self.sel = 0;
                    self.go(Step::Disk)
                }
                Key::Escape => {
                    self.password.text.clear();
                    self.repeat.text.clear();
                    self.on_repeat = false;
                    self.go(Step::User)
                }
                k => {
                    let field = if self.on_repeat {
                        &mut self.repeat
                    } else {
                        &mut self.password
                    };
                    if field.handle(k) {
                        self.error = None;
                        Action::Redraw
                    } else {
                        Action::None
                    }
                }
            },
            Step::Disk => {
                let n = targets(disks).len();
                match key {
                    Key::Up if self.sel > 0 => {
                        self.sel -= 1;
                        Action::Redraw
                    }
                    Key::Down if self.sel + 1 < n => {
                        self.sel += 1;
                        Action::Redraw
                    }
                    Key::Enter if n > 0 => self.go(Step::Confirm),
                    Key::Enter => self.fail("No hay ningún disco vacío. Agregá uno y reiniciá."),
                    Key::Escape => self.go(Step::Password),
                    _ => Action::None,
                }
            }
            Step::Confirm => match key {
                Key::Enter => {
                    let Some(&disk) = targets(disks).get(self.sel) else {
                        return self.go(Step::Disk);
                    };
                    self.step = Step::Installing;
                    self.error = None;
                    Action::Install {
                        disk,
                        user: self.user.text.clone(),
                        password: self.password.text.clone(),
                    }
                }
                Key::Escape => self.go(Step::Disk),
                _ => Action::None,
            },
            Step::Installing => Action::None,
            Step::Done(Ok(_)) => match key {
                Key::Enter => Action::Reboot,
                _ => Action::None,
            },
            Step::Done(Err(_)) => match key {
                Key::Enter | Key::Escape => self.go(Step::Disk),
                _ => Action::None,
            },
        }
    }

    /// Un clic: los botones hacen lo mismo que Enter y Esc; en la lista de discos, elige.
    pub fn click(&mut self, x: i32, y: i32, w: usize, h: usize, disks: &[DiskInfo]) -> Action {
        let l = Layout::new(w, h);
        if self.step == Step::Disk {
            for i in 0..targets(disks).len() {
                if l.row(i).contains(x, y) {
                    self.sel = i;
                    return Action::Redraw;
                }
            }
        }
        if self.step == Step::Password {
            for (i, r) in [l.field(0), l.field(1)].iter().enumerate() {
                if r.contains(x, y) {
                    self.on_repeat = i == 1;
                    return Action::Redraw;
                }
            }
        }
        if l.primary().contains(x, y) && self.primary_label().is_some() {
            return self.key(Key::Enter, disks);
        }
        if l.secondary().contains(x, y) && self.secondary_label().is_some() {
            return self.key(Key::Escape, disks);
        }
        Action::None
    }

    fn primary_label(&self) -> Option<&'static str> {
        match self.step {
            Step::Welcome => Some(tr("INSTALAR")),
            Step::User | Step::Password | Step::Disk => Some(tr("SIGUIENTE")),
            Step::Confirm => Some(tr("INSTALAR AHORA")),
            Step::Installing => None,
            Step::Done(Ok(_)) => Some(tr("REINICIAR")),
            Step::Done(Err(_)) => Some(tr("VOLVER")),
        }
    }

    fn secondary_label(&self) -> Option<&'static str> {
        match self.step {
            Step::Welcome => Some(tr("PROBAR SIN INSTALAR")),
            Step::User | Step::Password | Step::Disk | Step::Confirm => Some(tr("ATRÁS")),
            Step::Installing | Step::Done(_) => None,
        }
    }
}

fn capitalized(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Dónde va cada cosa.
struct Layout {
    card: Rect,
}

impl Layout {
    fn new(w: usize, h: usize) -> Layout {
        let (w, h) = (w as i32, h as i32);
        let cw = 620.min(w - 32);
        let ch = 400.min(h - 180);
        Layout {
            card: Rect::new((w - cw) / 2, (h - ch) / 2 + 60, cw, ch),
        }
    }

    /// Los campos de texto (0 = el primero).
    fn field(&self, i: i32) -> Rect {
        Rect::new(
            self.card.x + 40,
            self.card.y + 150 + i * 76,
            self.card.w - 80,
            44,
        )
    }

    fn row(&self, i: usize) -> Rect {
        Rect::new(
            self.card.x + 40,
            self.card.y + 140 + i as i32 * 52,
            self.card.w - 80,
            44,
        )
    }

    fn primary(&self) -> Rect {
        let c = self.card;
        Rect::new(c.x + c.w - 40 - 200, c.y + c.h - 70, 200, 44)
    }

    fn secondary(&self) -> Rect {
        let c = self.card;
        Rect::new(c.x + 40, c.y + c.h - 70, 220, 44)
    }
}

fn field(c: &mut Canvas<'_>, r: Rect, caption: &str, value: &str, secret: bool, focus: bool) {
    let st = label(theme::text_dim());
    text::draw(c, r.x, r.y - 24, caption, &st);
    rounded_rect(c, r.x, r.y, r.w, r.h, 8, field_bg(), 255);
    let rim = if focus {
        theme::cyan()
    } else {
        theme::panel_rim()
    };
    rounded_outline(c, r.x, r.y, r.w, r.h, 8, rim);
    let mut end = r.x + 16;
    if secret {
        let n = value.chars().count().min(((r.w - 40) / 22) as usize) as i32;
        for i in 0..n {
            circle(c, r.x + 24 + i * 22, r.y + r.h / 2, 6, theme::text(), true);
        }
        end = r.x + 14 + n * 22;
    } else {
        end += text::draw(c, r.x + 16, r.y + 13, value, &s16(theme::text()));
    }
    if focus {
        c.fill_rect(end + 2, r.y + 11, 2, 22, theme::cyan());
    }
}

/// El asistente a pantalla completa: la esfera de fondo, lo que dice JARVIS y el paso actual.
pub fn draw(c: &mut Canvas<'_>, w: usize, h: usize, s: &Setup, disks: &[DiskInfo]) {
    let l = Layout::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.fill_rect(0, 0, wi, hi, theme::void());
    jarvis_gfx::shapes::glow(
        c,
        wi / 2,
        hi / 3,
        hi / 3,
        theme::vector_blue().scale(110),
        70,
    );

    // Arriba: JARVIS y lo que dice.
    let title = "J A R V I S";
    let st = big(theme::cyan());
    text::draw(c, (wi - text::width(title, &st)) / 2, 48, title, &st);
    circle(c, wi / 2, 128, 22, theme::vector_blue(), true);
    circle(c, wi / 2, 128, 22, theme::cyan(), false);
    circle(c, wi / 2, 128, 8, theme::cyan(), true);
    let says = s.jarvis_says();
    let st = s16(Color::WHITE);
    let tw = text::width(&says, &st);
    text::draw(c, (wi - tw) / 2, 170, &says, &st);

    // La tarjeta del paso.
    let k = l.card;
    rounded_rect(c, k.x, k.y, k.w, k.h, 10, theme::panel(), 245);
    rounded_outline(c, k.x, k.y, k.w, k.h, 10, theme::panel_rim());
    let heading = match s.number() {
        Some(n) => trf(
            "INSTALACIÓN DE JARVIS-OS · PASO {} DE 4",
            &[&alloc::format!("{n}")],
        ),
        None => tr("INSTALACIÓN DE JARVIS-OS").into(),
    };
    text::draw(c, k.x + 40, k.y + 30, &heading, &label(theme::cyan()));

    let body = s16(theme::text());
    let dim = light(theme::text_dim());
    match &s.step {
        Step::Welcome => {
            let lines = [
                tr("JARVIS-OS va a quedar instalado en un disco vacío de esta máquina."),
                tr("Te voy a pedir un usuario, una contraseña y el disco."),
                tr("También podés probarlo sin instalar: nada se guarda al apagar."),
            ];
            for (i, t) in lines.iter().enumerate() {
                text::draw(c, k.x + 40, k.y + 90 + i as i32 * 34, t, &body);
            }
        }
        Step::User => {
            field(
                c,
                l.field(0),
                tr("NOMBRE DE USUARIO"),
                s.user(),
                false,
                true,
            );
            text::draw(
                c,
                k.x + 40,
                l.field(0).y + 64,
                tr("Letras, números, - o _. Así te voy a saludar."),
                &dim,
            );
        }
        Step::Password => {
            field(
                c,
                l.field(0),
                tr("CONTRASEÑA"),
                &s.password.text,
                true,
                !s.on_repeat,
            );
            field(
                c,
                l.field(1),
                tr("REPETILA"),
                &s.repeat.text,
                true,
                s.on_repeat,
            );
        }
        Step::Disk => {
            let list = targets(disks);
            if list.is_empty() {
                text::draw(
                    c,
                    k.x + 40,
                    k.y + 100,
                    tr("No encontré ningún disco vacío."),
                    &body,
                );
                text::draw(
                    c,
                    k.x + 40,
                    k.y + 134,
                    tr("Agregale uno a la máquina (SATA, NVMe o IDE) y reiniciá."),
                    &dim,
                );
            } else {
                text::draw(
                    c,
                    k.x + 40,
                    k.y + 100,
                    tr("DISCOS VACÍOS"),
                    &label(theme::text_dim()),
                );
            }
            for (i, &d) in list.iter().enumerate().take(3) {
                let r = l.row(i);
                let chosen = i == s.sel;
                if chosen {
                    rounded_rect(c, r.x, r.y, r.w, r.h, 6, selected_bg(), 255);
                    rounded_outline(c, r.x, r.y, r.w, r.h, 6, theme::cyan());
                } else {
                    rounded_outline(c, r.x, r.y, r.w, r.h, 6, theme::panel_rim());
                }
                let disk = &disks[d];
                text::draw(c, r.x + 16, r.y + 13, &disk.name, &body);
                let size = alloc::format!("{} GiB", disk.mib / 1024);
                let sw = text::width(&size, &dim);
                text::draw(c, r.x + r.w - 16 - sw, r.y + 13, &size, &dim);
            }
        }
        Step::Confirm => {
            let disk = targets(disks)
                .get(s.sel)
                .and_then(|&i| disks.get(i))
                .map_or("", |d| d.name.as_str());
            let rows = [
                (tr("USUARIO"), s.user().into()),
                (tr("CONTRASEÑA"), "******".into()),
                (tr("DISCO"), String::from(disk)),
            ];
            for (i, (k2, v)) in rows.iter().enumerate() {
                let y = k.y + 90 + i as i32 * 40;
                text::draw(c, k.x + 40, y, k2, &label(theme::text_dim()));
                text::draw(c, k.x + 220, y, v, &body);
            }
            text::draw(
                c,
                k.x + 40,
                k.y + 220,
                tr("Se va a usar el disco entero."),
                &label(theme::amber()),
            );
        }
        Step::Installing => {
            text::draw(
                c,
                k.x + 40,
                k.y + 100,
                tr("Copiando el sistema al disco..."),
                &body,
            );
            text::draw(
                c,
                k.x + 40,
                k.y + 134,
                tr("No apagues la máquina. La pantalla queda quieta mientras tanto."),
                &dim,
            );
        }
        Step::Done(Ok(msg)) => {
            text::draw(
                c,
                k.x + 40,
                k.y + 100,
                tr("JARVIS-OS quedó instalado."),
                &body,
            );
            text::draw(c, k.x + 40, k.y + 134, msg, &dim);
        }
        Step::Done(Err(e)) => {
            text::draw(c, k.x + 40, k.y + 100, tr("No se pudo instalar:"), &body);
            text::draw(c, k.x + 40, k.y + 134, e, &label(theme::amber()));
        }
    }
    if let Some(e) = s.error {
        text::draw(c, k.x + 40, k.y + k.h - 110, tr(e), &label(theme::amber()));
    }
    if let Some(t) = s.primary_label() {
        button(c, l.primary(), t, theme::cyan(), 60);
    }
    if let Some(t) = s.secondary_label() {
        button(c, l.secondary(), t, theme::text_dim(), 0);
    }
    let hint = tr("ENTER: SEGUIR · ESC: VOLVER");
    let st = label(theme::text_dim());
    text::draw(c, (wi - text::width(hint, &st)) / 2, hi - 40, hint, &st);
}

/// Lo que dice JARVIS la primera vez que entrás al sistema instalado, una frase por vez.
pub const TOUR: [&str; 6] = [
    "Bienvenido a JARVIS-OS, {}. Te muestro dónde está cada cosa.",
    "Arriba a la izquierda está la barra con tus aplicaciones: archivos, terminal, música y más.",
    "La tecla Windows abre el menú de inicio; Windows + X, el menú rápido.",
    "Abajo a la izquierda, el estado de la máquina: procesador, memoria, disco y red.",
    "Para hablar conmigo, abrí mi consola: el último ícono de la barra.",
    "Eso es todo. Estoy a tu disposición.",
];
