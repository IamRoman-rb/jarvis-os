//! Configuración del sistema, como la de Windows 11 o Ubuntu: una lista de secciones a la
//! izquierda y, a la derecha, las opciones de la sección elegida.
//!
//! Cada opción es una fila con un control (interruptor, lista para elegir, campo de texto o
//! botón). La app trabaja sobre una copia de la [`Config`]; cada cambio se manda al escritorio
//! por el `Outbox`, que lo aplica al momento (fondo, reloj, mouse…) y lo guarda en el disco.
//!
//! Teclado: ↑ ↓ recorren las filas, ← → cambian el valor, Enter o Espacio activan, RePág/AvPág
//! (o Ctrl+Tab) cambian de sección.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};
use jarvis_gfx::hud::{Icon, icon};
use jarvis_gfx::shapes::{circle, rounded_outline, rounded_rect};
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::{Click, Ctx, SysView};
use crate::config::{Config, SOLID_COLORS, SearchEngine, Wallpaper, valid_name};
use crate::files::{TRASH, format_size, join};
use crate::firewall::{Action, Dir, Rule};
use crate::i18n::{Lang, tr, trf};
use crate::input::{Key, Mods};
use crate::system::{Launch, Power};
use crate::text_input::TextInput;
use crate::widgets::{
    FIELD_BG, SELECTED_BG, WINDOW_BG, bar, big, button, draw_fit, duration, ip, label, light, s16,
};

const SIDEBAR_W: i32 = 236;
const SECTION_H: i32 = 38;
const ROW_H: i32 = 60;
const HEADER_H: i32 = 76;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    System,
    Personalization,
    DateTime,
    Network,
    Browser,
    Sound,
    Devices,
    Apps,
    Storage,
    Security,
    Firewall,
}

pub const SECTIONS: [Section; 11] = [
    Section::System,
    Section::Personalization,
    Section::DateTime,
    Section::Network,
    Section::Browser,
    Section::Sound,
    Section::Devices,
    Section::Apps,
    Section::Storage,
    Section::Security,
    Section::Firewall,
];

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Section::System => tr("Sistema"),
            Section::Personalization => tr("Personalización"),
            Section::DateTime => tr("Hora e idioma"),
            Section::Network => tr("Red e Internet"),
            Section::Browser => tr("Navegador"),
            Section::Sound => tr("Sonido"),
            Section::Devices => tr("Mouse y teclado"),
            Section::Apps => tr("Aplicaciones"),
            Section::Storage => tr("Almacenamiento"),
            Section::Security => tr("Privacidad y seguridad"),
            Section::Firewall => tr("Firewall"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::System => Icon::Screen,
            Section::Personalization => Icon::Image,
            Section::DateTime => Icon::Gauge,
            Section::Network => Icon::Globe,
            Section::Browser => Icon::Globe,
            Section::Sound => Icon::Music,
            Section::Devices => Icon::Gear,
            Section::Apps => Icon::Package,
            Section::Storage => Icon::Folder,
            Section::Security => Icon::Lock,
            Section::Firewall => Icon::Globe,
        }
    }
}

/// Qué hace cada fila (para los clics y las teclas).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opt {
    Hostname,
    User,
    Restart,
    Shutdown,
    Wallpaper,
    Animations,
    StatusPanel,
    Language,
    Zone,
    Clock24,
    TestNet,
    Homepage,
    Search,
    LightPages,
    Images,
    Reader,
    Sounds,
    TestSound,
    MouseSpeed,
    WheelLines,
    InvertWheel,
    Keyboard,
    Updates,
    MorePackages,
    Remove(usize),
    EmptyTrash,
    Pin,
    LockAfter,
    LockNow,
    FwEnabled,
    FwDefaultOut,
    FwLog,
    /// Permitir o no a una app de [`crate::firewall::APPS`].
    FwApp(usize),
    FwAddSite,
    FwRule(usize),
    FwShowLog,
    Info,
}

enum Control {
    Toggle(bool),
    Choice(String),
    Text {
        value: String,
        secret: bool,
    },
    Button(&'static str),
    /// Texto a la derecha (solo lectura).
    Value(String),
    /// Barra de uso (porcentaje) con texto.
    Usage(u32, String),
}

struct Row {
    opt: Opt,
    title: String,
    detail: String,
    control: Control,
}

impl Row {
    fn new(opt: Opt, title: impl Into<String>, detail: impl Into<String>, control: Control) -> Row {
        Row {
            opt,
            title: title.into(),
            detail: detail.into(),
            control,
        }
    }

    fn interactive(&self) -> bool {
        !matches!(self.control, Control::Value(_) | Control::Usage(..))
    }
}

const LOCK_CHOICES: [u32; 5] = [0, 1, 5, 15, 30];

pub struct Settings {
    pub dirty: bool,
    pub section: Section,
    cfg: Config,
    selected: usize,
    /// Campo que se está editando (y su texto).
    editing: Option<(Opt, TextInput)>,
    /// Imágenes que se pueden usar de fondo (se buscan al entrar a Personalización).
    wallpapers: Vec<String>,
    /// Paquetes instalados (nombre, versión).
    packages: Vec<(String, String)>,
    /// Uso del disco por carpeta.
    storage: Vec<(String, u64)>,
    /// "Vaciar la Papelera" pide un segundo clic.
    confirm_trash: bool,
    /// Hasta cuándo suena el tono de prueba.
    tone_until: Option<u64>,
    net_test: Option<(u32, String)>,
    screen: (usize, usize),
    /// (usado, total) del disco, del último dibujo.
    disk: Option<(u64, u64)>,
}

impl Settings {
    pub fn new<D: BlockDevice>(section: Section, ctx: &mut Ctx<'_, D>) -> Settings {
        let mut s = Settings {
            dirty: true,
            section,
            cfg: ctx.config.clone(),
            selected: 0,
            editing: None,
            wallpapers: Vec::new(),
            packages: Vec::new(),
            storage: Vec::new(),
            confirm_trash: false,
            tone_until: None,
            net_test: None,
            screen: (0, 0),
            disk: None,
        };
        s.enter(section, ctx);
        s
    }

    pub fn title(&self) -> String {
        trf("Configuración · {}", &[self.section.name()])
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// La configuración cambió desde afuera (Win+A): se toma la nueva.
    pub fn sync(&mut self, cfg: &Config) {
        self.cfg = cfg.clone();
        self.dirty = true;
    }

    /// Cambia de sección y junta lo que esa sección necesita del disco.
    pub fn enter<D: BlockDevice>(&mut self, section: Section, ctx: &mut Ctx<'_, D>) {
        self.section = section;
        self.selected = 0;
        self.editing = None;
        self.confirm_trash = false;
        self.dirty = true;
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return;
        };
        match section {
            Section::Personalization => self.wallpapers = find_wallpapers(fs),
            Section::Apps => self.packages = crate::term::apt::installed(fs),
            Section::Storage => self.storage = folder_sizes(fs),
            _ => {}
        }
    }

    fn rows(&self, stats: &crate::system::SystemStats) -> Vec<Row> {
        use Control::*;
        let c = &self.cfg;
        let on = |b: bool| Toggle(b);
        match self.section {
            Section::System => {
                let (w, h) = self.screen;
                alloc::vec![
                    Row::new(
                        Opt::Hostname,
                        tr("Nombre del equipo"),
                        tr("Aparece en la terminal: usuario@equipo"),
                        Text {
                            value: c.hostname.clone(),
                            secret: false
                        }
                    ),
                    Row::new(
                        Opt::User,
                        tr("Usuario"),
                        tr("Tu nombre de usuario en la terminal"),
                        Text {
                            value: c.user.clone(),
                            secret: false
                        }
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Versión"),
                        tr("Kernel propio en Rust (x86_64, UEFI)"),
                        Value("JARVIS-OS 0.1 · hito K4".into())
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Procesador"),
                        "",
                        Value(if stats.cpu_name.is_empty() {
                            "x86_64".into()
                        } else {
                            stats.cpu_name.clone()
                        })
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Memoria"),
                        format!("RAM {}", format_size(stats.ram_total)),
                        Usage(
                            (stats.heap_used * 100)
                                .checked_div(stats.heap_total)
                                .unwrap_or(0) as u32,
                            trf(
                                "{} de {} (heap)",
                                &[
                                    &format_size(stats.heap_used),
                                    &format_size(stats.heap_total)
                                ]
                            ),
                        )
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Pantalla"),
                        tr("Resolución que eligió el firmware (UEFI GOP)"),
                        Value(format!("{w} × {h}"))
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Tiempo encendido"),
                        "",
                        Value(duration(stats.uptime_ms))
                    ),
                    Row::new(
                        Opt::Restart,
                        tr("Reiniciar"),
                        tr("Vuelve a arrancar la máquina"),
                        Button(tr("REINICIAR"))
                    ),
                    Row::new(
                        Opt::Shutdown,
                        tr("Apagar"),
                        tr("Todo lo del disco ya está guardado"),
                        Button(tr("APAGAR"))
                    ),
                ]
            }
            Section::Personalization => alloc::vec![
                Row::new(
                    Opt::Wallpaper,
                    tr("Fondo de escritorio"),
                    tr("El HUD, un color o una imagen BMP de /Imágenes"),
                    Choice(wallpaper_name(&c.wallpaper))
                ),
                Row::new(
                    Opt::Animations,
                    tr("Animaciones"),
                    tr("La esfera de JARVIS gira (apagarlo ahorra CPU)"),
                    on(c.animations)
                ),
                Row::new(
                    Opt::StatusPanel,
                    tr("Panel de estado"),
                    tr("Gráficos de CPU, memoria, disco y red abajo a la izquierda"),
                    on(c.status_panel)
                ),
            ],
            Section::DateTime => alloc::vec![
                Row::new(
                    Opt::Language,
                    tr("Idioma"),
                    tr("Menús, ventanas y Configuración (la terminal sigue en castellano)"),
                    Choice(c.language.name().into())
                ),
                Row::new(
                    Opt::Zone,
                    tr("Zona horaria"),
                    tr("El reloj de la máquina está en UTC"),
                    Choice(c.zone_label())
                ),
                Row::new(
                    Opt::Clock24,
                    tr("Formato de 24 horas"),
                    tr("Si no, 12 horas con a. m. / p. m."),
                    on(c.clock_24h)
                ),
            ],
            Section::Network => {
                let n = &stats.net;
                let state = match (n.present, n.ip) {
                    (false, _) => tr("Sin placa de red").to_string(),
                    (true, None) => tr("Pidiendo dirección (DHCP)...").to_string(),
                    (true, Some(a)) => trf("Conectado · {}", &[&ip(a)]),
                };
                let m = n.mac;
                let test = match &self.net_test {
                    Some((_, msg)) => msg.clone(),
                    None => tr("Pide http://info.cern.ch/ por la red propia").into(),
                };
                alloc::vec![
                    Row::new(
                        Opt::Info,
                        tr("Estado"),
                        "Placa virtio-net · TCP/IP smoltcp",
                        Value(state)
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Dirección física (MAC)"),
                        "",
                        Value(format!(
                            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                            m[0], m[1], m[2], m[3], m[4], m[5]
                        ))
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Puerta de enlace y DNS"),
                        tr("Los da el servidor DHCP"),
                        Value(format!(
                            "{} · {}",
                            n.gateway.map(ip).unwrap_or_else(|| "-".into()),
                            n.dns.map(ip).unwrap_or_else(|| "-".into())
                        ))
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Tráfico desde el arranque"),
                        "",
                        Value(trf(
                            "recibido {} · enviado {}",
                            &[&format_size(stats.net_rx), &format_size(stats.net_tx)]
                        ))
                    ),
                    Row::new(
                        Opt::Info,
                        "HTTPS",
                        tr("El kernel todavía no tiene TLS: usa el puente del anfitrión"),
                        Value("10.0.2.2:8118".into())
                    ),
                    Row::new(
                        Opt::TestNet,
                        tr("Probar la conexión"),
                        test,
                        Button(tr("PROBAR"))
                    ),
                ]
            }
            Section::Browser => alloc::vec![
                Row::new(
                    Opt::Homepage,
                    tr("Página de inicio"),
                    tr("Lo que abre el navegador (about:inicio = la de JARVIS)"),
                    Text {
                        value: c.homepage.clone(),
                        secret: false
                    }
                ),
                Row::new(
                    Opt::Search,
                    tr("Buscador"),
                    tr("Lo que escribís en la barra que no es una dirección"),
                    Choice(c.search.name().into())
                ),
                Row::new(
                    Opt::LightPages,
                    tr("Páginas claras"),
                    tr("Fondo blanco como en otros navegadores (si no, oscuro)"),
                    on(c.light_pages)
                ),
                Row::new(
                    Opt::Images,
                    tr("Mostrar imágenes"),
                    tr("Se convierten a BMP en el puente del anfitrión"),
                    on(c.load_images)
                ),
                Row::new(
                    Opt::Reader,
                    tr("Modo lectura"),
                    tr("Solo el contenido: sin menús, formularios ni estilos"),
                    on(c.reader_mode)
                ),
            ],
            Section::Sound => alloc::vec![
                Row::new(
                    Opt::Sounds,
                    tr("Sonidos del sistema"),
                    tr("Un pitido corto con los avisos de error"),
                    on(c.sounds)
                ),
                Row::new(
                    Opt::TestSound,
                    tr("Probar el parlante"),
                    tr("Un La (440 Hz) por el parlante de la PC"),
                    Button(tr("PROBAR"))
                ),
                Row::new(
                    Opt::Info,
                    tr("Dispositivo"),
                    tr("Canal 2 del PIT (8254)"),
                    Value(tr("Parlante de la PC").into())
                ),
            ],
            Section::Devices => alloc::vec![
                Row::new(
                    Opt::MouseSpeed,
                    tr("Velocidad del puntero"),
                    tr("1 = lento · 3 = normal · 5 = rápido"),
                    Choice(format!("{}", c.mouse_speed))
                ),
                Row::new(
                    Opt::WheelLines,
                    tr("Rueda del mouse"),
                    tr("Cuánto se mueve la página con cada paso"),
                    Choice(trf("{} renglones", &[&c.wheel_lines.to_string()]))
                ),
                Row::new(
                    Opt::InvertWheel,
                    tr("Invertir la rueda"),
                    tr("Desplazamiento \"natural\", como en un touchpad"),
                    on(c.invert_wheel)
                ),
                Row::new(
                    Opt::Keyboard,
                    tr("Distribución del teclado"),
                    tr("Latinoamérica: ñ, tildes (´ + vocal) y AltGr+Q = @"),
                    Choice(if c.latam_keyboard {
                        tr("Español (Latinoamérica)").into()
                    } else {
                        tr("Inglés (EE. UU.)").into()
                    })
                ),
            ],
            Section::Apps => {
                let mut rows = alloc::vec![
                    Row::new(
                        Opt::Updates,
                        tr("Buscar actualizaciones"),
                        tr("Abre la terminal con: apt update && apt upgrade"),
                        Button(tr("BUSCAR"))
                    ),
                    Row::new(
                        Opt::MorePackages,
                        tr("Instalar más programas"),
                        tr("Abre la terminal con la lista de paquetes (apt list)"),
                        Button(tr("VER"))
                    ),
                ];
                if self.packages.is_empty() {
                    rows.push(Row::new(
                        Opt::Info,
                        tr("Programas instalados"),
                        tr("Todavía no instalaste ninguno. Probá: apt install neofetch"),
                        Value("0".into()),
                    ));
                }
                for (i, (name, version)) in self.packages.iter().enumerate() {
                    rows.push(Row::new(
                        Opt::Remove(i),
                        name.clone(),
                        trf("versión {}", &[version]),
                        Button(tr("DESINSTALAR")),
                    ));
                }
                rows
            }
            Section::Storage => {
                let mut rows = Vec::new();
                if let Some((used, total)) = self.disk_usage_hint() {
                    rows.push(Row::new(
                        Opt::Info,
                        tr("Disco JARVIS (FAT32)"),
                        format!("{} libres", format_size(total - used)),
                        Usage(
                            (used * 100).checked_div(total).unwrap_or(0) as u32,
                            format!("{} de {}", format_size(used), format_size(total)),
                        ),
                    ));
                }
                let biggest = self.storage.iter().map(|s| s.1).max().unwrap_or(1).max(1);
                for (dir, size) in &self.storage {
                    rows.push(Row::new(
                        Opt::Info,
                        dir.clone(),
                        "",
                        Usage((size * 100 / biggest) as u32, format_size(*size)),
                    ));
                }
                rows.push(Row::new(
                    Opt::EmptyTrash,
                    tr("Vaciar la Papelera"),
                    if self.confirm_trash {
                        tr("¿Seguro? Se borra para siempre: hacé clic otra vez")
                    } else {
                        tr("Borra para siempre lo que hay en /Papelera")
                    },
                    Button(if self.confirm_trash {
                        tr("CONFIRMAR")
                    } else {
                        tr("VACIAR")
                    }),
                ));
                rows
            }
            Section::Security => {
                let lock = if c.lock_minutes == 0 {
                    tr("Nunca").to_string()
                } else {
                    trf("{} min", &[&c.lock_minutes.to_string()])
                };
                alloc::vec![
                    Row::new(
                        Opt::Pin,
                        tr("PIN de desbloqueo"),
                        tr("Solo números (hasta 8). Vacío = sin PIN"),
                        Text {
                            value: c.pin.clone(),
                            secret: true
                        }
                    ),
                    Row::new(
                        Opt::LockAfter,
                        tr("Bloquear sin actividad"),
                        tr("Después de un rato sin usar el teclado ni el mouse"),
                        Choice(lock)
                    ),
                    Row::new(
                        Opt::LockNow,
                        tr("Bloquear ahora"),
                        tr("Lo mismo que Win+L"),
                        Button(tr("BLOQUEAR"))
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Borrar = mover a la Papelera"),
                        tr("Ni las apps ni la terminal borran para siempre sin preguntar"),
                        Value(tr("Siempre").into())
                    ),
                ]
            }
            Section::Firewall => {
                let fw = &c.firewall;
                let mut rows = alloc::vec![
                    Row::new(
                        Opt::FwEnabled,
                        tr("Firewall"),
                        tr("Revisa cada conexión que sale antes de que llegue a la red"),
                        on(fw.enabled)
                    ),
                    Row::new(
                        Opt::FwDefaultOut,
                        tr("Conexiones salientes"),
                        tr("Lo que no dice ninguna regla"),
                        Choice(
                            if fw.default_out == Action::Allow {
                                tr("Permitir")
                            } else {
                                tr("Denegar")
                            }
                            .into()
                        )
                    ),
                    Row::new(
                        Opt::Info,
                        tr("Conexiones entrantes"),
                        tr("JARVIS-OS no ofrece servicios: se rechaza lo que no pidió"),
                        Value(tr("Bloqueadas").into())
                    ),
                    Row::new(
                        Opt::FwLog,
                        tr("Anotar lo bloqueado"),
                        tr("En /Sistema/firewall.log (ufw show blocked)"),
                        on(fw.log)
                    ),
                    Row::new(
                        Opt::FwShowLog,
                        tr("Ver lo bloqueado"),
                        tr("Abre la terminal con: ufw show blocked"),
                        Button(tr("VER"))
                    ),
                ];
                for (i, (name, desc)) in crate::firewall::APPS.iter().enumerate() {
                    let blocked = fw.rules.contains(&app_rule(name));
                    rows.push(Row::new(
                        Opt::FwApp(i),
                        trf("Permitir: {}", &[tr(desc)]),
                        trf("Regla: deny out app {}", &[name]),
                        on(!blocked),
                    ));
                }
                rows.push(Row::new(
                    Opt::FwAddSite,
                    tr("Bloquear un sitio"),
                    tr("Ejemplo: tiktok.com (también bloquea sus subdominios)"),
                    Text {
                        value: String::new(),
                        secret: false,
                    },
                ));
                for (i, r) in fw.rules.iter().enumerate() {
                    rows.push(Row::new(
                        Opt::FwRule(i),
                        trf("Regla {}", &[&(i + 1).to_string()]),
                        r.to_line(),
                        Button(tr("BORRAR")),
                    ));
                }
                rows
            }
        }
    }

    fn disk_usage_hint(&self) -> Option<(u64, u64)> {
        self.disk
    }

    // --- diseño -------------------------------------------------------------------------------

    fn section_rect(r: Rect, i: usize) -> Rect {
        Rect::new(
            r.x + 10,
            r.y + 64 + i as i32 * SECTION_H,
            SIDEBAR_W - 20,
            SECTION_H - 4,
        )
    }

    fn panel(r: Rect) -> Rect {
        Rect::new(r.x + SIDEBAR_W, r.y, r.w - SIDEBAR_W, r.h)
    }

    fn row_rect(r: Rect, i: usize) -> Rect {
        let p = Self::panel(r);
        Rect::new(
            p.x + 20,
            p.y + HEADER_H + i as i32 * ROW_H,
            p.w - 40,
            ROW_H - 6,
        )
    }

    fn control_rect(row: Rect) -> Rect {
        let w = (row.w * 2 / 5).clamp(150, 340);
        Rect::new(row.x + row.w - w - 14, row.y + 11, w, row.h - 22)
    }

    // --- dibujo -------------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, sys: &SysView<'_>) {
        self.screen = sys.screen;
        self.disk = sys
            .disk
            .map(|(_, free, total)| (total.saturating_sub(free), total));
        c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
        // Barra lateral.
        c.fill_rect(r.x, r.y, SIDEBAR_W, r.h, theme::PANEL);
        text::draw(
            c,
            r.x + 20,
            r.y + 20,
            tr("CONFIGURACIÓN"),
            &label(theme::CYAN),
        );
        for (i, s) in SECTIONS.iter().enumerate() {
            let sr = Self::section_rect(r, i);
            let active = *s == self.section;
            if active {
                rounded_rect(c, sr.x, sr.y, sr.w, sr.h, 4, SELECTED_BG, 255);
                c.fill_rect(sr.x, sr.y + 8, 3, sr.h - 16, theme::CYAN);
            }
            let col = if active { theme::TEXT } else { theme::TEXT_DIM };
            icon(
                c,
                s.icon(),
                sr.x + 22,
                sr.y + sr.h / 2,
                if active { theme::CYAN } else { theme::TEXT_DIM },
            );
            text::draw(c, sr.x + 42, sr.y + 9, s.name(), &s16(col));
        }
        // Sección.
        let p = Self::panel(r);
        text::draw(
            c,
            p.x + 20,
            p.y + 20,
            self.section.name(),
            &big(theme::TEXT),
        );
        let rows = self.rows(sys.stats);
        let visible = ((p.h - HEADER_H) / ROW_H).max(1) as usize;
        let first = self.selected.saturating_sub(visible - 1);
        for (i, row) in rows.iter().enumerate().skip(first).take(visible) {
            let rr = Self::row_rect(r, i - first);
            let sel = i == self.selected;
            rounded_rect(
                c,
                rr.x,
                rr.y,
                rr.w,
                rr.h,
                6,
                if sel { SELECTED_BG } else { theme::PANEL },
                255,
            );
            rounded_outline(
                c,
                rr.x,
                rr.y,
                rr.w,
                rr.h,
                6,
                if sel {
                    theme::CYAN.scale(160)
                } else {
                    theme::PANEL_RIM
                },
            );
            let cr = Self::control_rect(rr);
            let text_w = cr.x - rr.x - 30;
            draw_fit(
                c,
                rr.x + 16,
                rr.y + 10,
                &row.title,
                &s16(theme::TEXT),
                text_w,
            );
            draw_fit(
                c,
                rr.x + 16,
                rr.y + 31,
                &row.detail,
                &light(theme::TEXT_DIM),
                text_w,
            );
            self.draw_control(c, row, cr, sel);
        }
        if rows.len() > visible {
            let hint = trf(
                "{} de {} · flechas para ver más",
                &[&(self.selected + 1).to_string(), &rows.len().to_string()],
            );
            text::draw_right(c, p.x + p.w - 24, p.y + 30, &hint, &light(theme::TEXT_DIM));
        }
    }

    fn draw_control(&self, c: &mut Canvas<'_>, row: &Row, cr: Rect, sel: bool) {
        match &row.control {
            Control::Toggle(v) => {
                let (w, h) = (46, 24);
                let x = cr.x + cr.w - w;
                let y = cr.y + (cr.h - h) / 2;
                let fill = if *v {
                    theme::CYAN.scale(200)
                } else {
                    theme::PANEL_RIM
                };
                rounded_rect(c, x, y, w, h, h / 2, fill, 255);
                let knob = if *v { x + w - h / 2 } else { x + h / 2 };
                circle(
                    c,
                    knob,
                    y + h / 2,
                    h / 2 - 4,
                    if *v { theme::VOID } else { theme::TEXT_DIM },
                    true,
                );
                let st = light(theme::TEXT_DIM);
                text::draw_right(
                    c,
                    x - 10,
                    y + 4,
                    if *v {
                        tr("Activado")
                    } else {
                        tr("Desactivado")
                    },
                    &st,
                );
            }
            Control::Choice(v) => {
                rounded_rect(c, cr.x, cr.y, cr.w, cr.h, 4, FIELD_BG, 255);
                rounded_outline(
                    c,
                    cr.x,
                    cr.y,
                    cr.w,
                    cr.h,
                    4,
                    if sel { theme::CYAN } else { theme::PANEL_RIM },
                );
                let st = s16(theme::TEXT);
                text::draw(c, cr.x + 10, cr.y + (cr.h - 16) / 2, "<", &st);
                text::draw_right(c, cr.x + cr.w - 10, cr.y + (cr.h - 16) / 2, ">", &st);
                let shown = text::fit(v, &st, cr.w - 50);
                let tw = text::width(&shown, &st);
                text::draw(
                    c,
                    cr.x + (cr.w - tw) / 2,
                    cr.y + (cr.h - 16) / 2,
                    &shown,
                    &st,
                );
            }
            Control::Text { value, secret } => {
                rounded_rect(c, cr.x, cr.y, cr.w, cr.h, 4, FIELD_BG, 255);
                let editing = self.editing.as_ref().filter(|(o, _)| *o == row.opt);
                rounded_outline(
                    c,
                    cr.x,
                    cr.y,
                    cr.w,
                    cr.h,
                    4,
                    if editing.is_some() {
                        theme::CYAN
                    } else {
                        theme::PANEL_RIM
                    },
                );
                let raw = editing.map_or(value.as_str(), |(_, t)| t.text.as_str());
                let shown: String = if *secret {
                    "*".repeat(raw.chars().count())
                } else {
                    raw.to_string()
                };
                let placeholder = shown.is_empty() && editing.is_none();
                let st = s16(if placeholder {
                    theme::TEXT_DIM
                } else {
                    theme::TEXT
                });
                let tw = draw_fit(
                    c,
                    cr.x + 10,
                    cr.y + (cr.h - 16) / 2,
                    if placeholder {
                        tr("(vacío) · Enter para editar")
                    } else {
                        &shown
                    },
                    &st,
                    cr.w - 20,
                );
                if editing.is_some() {
                    c.fill_rect(cr.x + 12 + tw, cr.y + 7, 2, cr.h - 14, theme::CYAN);
                }
            }
            Control::Button(t) => {
                let w = crate::widgets::button_width(t).max(110);
                let b = Rect::new(cr.x + cr.w - w, cr.y, w, cr.h);
                let danger = matches!(
                    row.opt,
                    Opt::Shutdown | Opt::Remove(_) | Opt::EmptyTrash | Opt::FwRule(_)
                );
                button(
                    c,
                    b,
                    t,
                    if danger { theme::AMBER } else { theme::CYAN },
                    if sel { 70 } else { 30 },
                );
            }
            Control::Value(v) => {
                draw_fit(c, cr.x, cr.y + (cr.h - 16) / 2, v, &s16(theme::CYAN), cr.w);
            }
            Control::Usage(pct, t) => {
                text::draw_right(c, cr.x + cr.w, cr.y, t, &light(theme::TEXT_DIM));
                bar(
                    c,
                    Rect::new(cr.x, cr.y + cr.h - 8, cr.w, 7),
                    *pct,
                    if *pct > 85 { theme::AMBER } else { theme::CYAN },
                );
            }
        }
    }

    // --- acciones -----------------------------------------------------------------------------

    fn commit<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        ctx.out.config = Some(self.cfg.clone());
        ctx.log.push(format!("CONFIG {}", self.section.name()));
    }

    /// `delta`: -1 / +1 para las listas (← →), 0 para activar (Enter, clic).
    fn activate<D: BlockDevice>(&mut self, opt: Opt, delta: i32, ctx: &mut Ctx<'_, D>) {
        let step = |v: i32, n: i32| (v + if delta == 0 { 1 } else { delta }).rem_euclid(n);
        self.dirty = true;
        let c = &mut self.cfg;
        match opt {
            Opt::Hostname | Opt::User | Opt::Homepage | Opt::Pin | Opt::FwAddSite => {
                if delta == 0 {
                    let (value, max) = match opt {
                        Opt::Hostname => (c.hostname.clone(), 24),
                        Opt::User => (c.user.clone(), 24),
                        Opt::Homepage => (c.homepage.clone(), 200),
                        Opt::FwAddSite => (String::new(), 100),
                        _ => (String::new(), 8),
                    };
                    self.editing = Some((opt, TextInput::new(&value, max)));
                }
                return;
            }
            Opt::Restart => {
                ctx.out.power = Some(Power::Reboot);
                return;
            }
            Opt::Shutdown => {
                ctx.out.power = Some(Power::Shutdown);
                return;
            }
            Opt::Wallpaper => {
                let mut all: Vec<Wallpaper> = alloc::vec![Wallpaper::Hud];
                all.extend((0..SOLID_COLORS.len()).map(Wallpaper::Solid));
                all.extend(self.wallpapers.iter().cloned().map(Wallpaper::Image));
                let pos = all.iter().position(|w| *w == c.wallpaper).unwrap_or(0) as i32;
                c.wallpaper = all[step(pos, all.len() as i32) as usize].clone();
            }
            Opt::Animations => c.animations = !c.animations,
            Opt::StatusPanel => c.status_panel = !c.status_panel,
            Opt::Zone => c.utc_offset = (step(c.utc_offset as i32 + 12, 27) - 12) as i8,
            Opt::Clock24 => c.clock_24h = !c.clock_24h,
            Opt::Language => {
                let all = Lang::ALL;
                let pos = all.iter().position(|l| *l == c.language).unwrap_or(0) as i32;
                c.language = all[step(pos, all.len() as i32) as usize];
            }
            Opt::TestNet => {
                let id = ctx.out.fetch("http://info.cern.ch/");
                self.net_test = Some((id, tr("Probando...").into()));
                return;
            }
            Opt::Search => {
                let all = SearchEngine::ALL;
                let pos = all.iter().position(|s| *s == c.search).unwrap_or(0) as i32;
                c.search = all[step(pos, all.len() as i32) as usize];
            }
            Opt::LightPages => c.light_pages = !c.light_pages,
            Opt::Images => c.load_images = !c.load_images,
            Opt::Reader => c.reader_mode = !c.reader_mode,
            Opt::Sounds => c.sounds = !c.sounds,
            Opt::TestSound => {
                ctx.out.tone = Some(440);
                self.tone_until = Some(ctx.now_ms + 400);
                return;
            }
            Opt::MouseSpeed => c.mouse_speed = (step(c.mouse_speed as i32 - 1, 5) + 1) as u8,
            Opt::WheelLines => c.wheel_lines = (step(c.wheel_lines as i32 - 1, 5) + 1) as u8,
            Opt::InvertWheel => c.invert_wheel = !c.invert_wheel,
            Opt::Keyboard => c.latam_keyboard = !c.latam_keyboard,
            Opt::Updates => {
                ctx.out
                    .launch
                    .push(Launch::Terminal(Some("apt update && apt upgrade".into())));
                return;
            }
            Opt::MorePackages => {
                ctx.out
                    .launch
                    .push(Launch::Terminal(Some("apt list".into())));
                return;
            }
            Opt::Remove(i) => {
                if let Some((name, _)) = self.packages.get(i) {
                    ctx.out
                        .launch
                        .push(Launch::Terminal(Some(format!("apt remove {name}"))));
                }
                return;
            }
            Opt::EmptyTrash => {
                if !self.confirm_trash {
                    self.confirm_trash = true;
                    return;
                }
                self.confirm_trash = false;
                if let Some(fs) = ctx.fs.as_deref_mut() {
                    let now = crate::apps::timestamp(ctx.clock);
                    let items = fs.list(TRASH).unwrap_or_default();
                    let n = items.iter().filter(|e| e.name != ".origen").count();
                    for e in items {
                        let _ = fs.remove(&join(TRASH, &e.name));
                    }
                    let _ = fs.write_file(crate::files::TRASH_INDEX, b"", now);
                    ctx.log.push("ARCHIVOS_PAPELERA_VACIA".into());
                    ctx.out.notify(
                        trf("Papelera vacía ({} elementos).", &[&n.to_string()]),
                        false,
                    );
                    self.storage = folder_sizes(fs);
                }
                return;
            }
            Opt::LockAfter => {
                let pos = LOCK_CHOICES
                    .iter()
                    .position(|m| *m == c.lock_minutes)
                    .unwrap_or(0) as i32;
                c.lock_minutes = LOCK_CHOICES[step(pos, LOCK_CHOICES.len() as i32) as usize];
            }
            Opt::LockNow => {
                ctx.out.lock = true;
                return;
            }
            Opt::FwEnabled => c.firewall.enabled = !c.firewall.enabled,
            Opt::FwDefaultOut => {
                c.firewall.default_out = if c.firewall.default_out == Action::Allow {
                    Action::Deny
                } else {
                    Action::Allow
                }
            }
            Opt::FwLog => c.firewall.log = !c.firewall.log,
            Opt::FwApp(i) => {
                let Some((name, _)) = crate::firewall::APPS.get(i) else {
                    return;
                };
                let rule = app_rule(name);
                match c.firewall.rules.iter().position(|r| *r == rule) {
                    Some(k) => {
                        c.firewall.rules.remove(k);
                    }
                    // Al principio: le gana a cualquier otra regla.
                    None => c.firewall.rules.insert(0, rule),
                }
            }
            Opt::FwRule(i) => {
                if i < c.firewall.rules.len() {
                    c.firewall.rules.remove(i);
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            Opt::FwShowLog => {
                ctx.out
                    .launch
                    .push(Launch::Terminal(Some("ufw show blocked".into())));
                return;
            }
            Opt::Info => return,
        }
        self.commit(ctx);
    }

    fn finish_edit<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some((opt, input)) = self.editing.take() else {
            return;
        };
        let v = input.text.trim().to_string();
        let ok = match opt {
            Opt::Hostname if valid_name(&v) => {
                self.cfg.hostname = v;
                true
            }
            Opt::User if valid_name(&v) => {
                self.cfg.user = v;
                true
            }
            Opt::Homepage if !v.is_empty() => {
                self.cfg.homepage = v;
                true
            }
            Opt::Pin if v.len() <= 8 && v.chars().all(|c| c.is_ascii_digit()) => {
                self.cfg.pin = v;
                true
            }
            Opt::FwAddSite if !v.is_empty() => {
                match Rule::parse(&format!(
                    "deny out to {}",
                    v.trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/')
                )) {
                    Ok(r) if !self.cfg.firewall.rules.contains(&r) => {
                        self.cfg.firewall.rules.push(r);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if ok {
            self.commit(ctx);
        } else {
            ctx.out.notify(tr("Ese valor no sirve (usuario y equipo: letras, números y guiones; PIN: solo números)."), true);
        }
        self.dirty = true;
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, mods: Mods, ctx: &mut Ctx<'_, D>) -> bool {
        self.dirty = true;
        if let Some((opt, input)) = &mut self.editing {
            match key {
                Key::Enter | Key::Tab => self.finish_edit(ctx),
                Key::Escape => self.editing = None,
                Key::Char(ch) if *opt == Opt::Pin && !ch.is_ascii_digit() => {}
                other => {
                    input.handle(other);
                }
            }
            return true;
        }
        let n = self.rows(ctx.stats).len();
        let sec = SECTIONS
            .iter()
            .position(|s| *s == self.section)
            .unwrap_or(0);
        match key {
            Key::PageDown => self.enter(SECTIONS[(sec + 1) % SECTIONS.len()], ctx),
            Key::Tab if mods.ctrl && !mods.shift => {
                self.enter(SECTIONS[(sec + 1) % SECTIONS.len()], ctx)
            }
            Key::PageUp => self.enter(SECTIONS[(sec + SECTIONS.len() - 1) % SECTIONS.len()], ctx),
            Key::Tab if mods.ctrl => {
                self.enter(SECTIONS[(sec + SECTIONS.len() - 1) % SECTIONS.len()], ctx)
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = n.saturating_sub(1),
            Key::Left | Key::Right | Key::Enter | Key::Char(' ') => {
                let rows = self.rows(ctx.stats);
                if let Some(row) = rows.get(self.selected).filter(|r| r.interactive()) {
                    let delta = match key {
                        Key::Left => -1,
                        Key::Right => 1,
                        _ => 0,
                    };
                    let opt = row.opt;
                    let is_choice = matches!(row.control, Control::Choice(_));
                    if delta == 0 || is_choice {
                        self.activate(opt, delta, ctx);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        let (x, y) = (click.x, click.y);
        self.dirty = true;
        if self.editing.is_some() {
            self.finish_edit(ctx);
        }
        if let Some(i) =
            (0..SECTIONS.len()).find(|&i| Self::section_rect(content, i).contains(x, y))
        {
            self.enter(SECTIONS[i], ctx);
            return;
        }
        let rows = self.rows(ctx.stats);
        let visible = ((content.h - HEADER_H) / ROW_H).max(1) as usize;
        let first = self.selected.saturating_sub(visible - 1);
        for (i, row) in rows.iter().enumerate().skip(first).take(visible) {
            let rr = Self::row_rect(content, i - first);
            if !rr.contains(x, y) {
                continue;
            }
            self.selected = i;
            if !row.interactive() {
                return;
            }
            let cr = Self::control_rect(rr);
            let delta = match row.control {
                // En las listas, la mitad izquierda va para atrás.
                Control::Choice(_) if cr.contains(x, y) && x < cr.x + cr.w / 2 => -1,
                Control::Choice(_) => 1,
                _ => 0,
            };
            let opt = row.opt;
            self.activate(opt, delta, ctx);
            return;
        }
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(t) = self.tone_until
            && ctx.now_ms >= t
        {
            ctx.out.tone = Some(0);
            self.tone_until = None;
        }
    }

    pub fn net_response(&mut self, id: u32, result: &Result<crate::system::HttpResponse, String>) {
        if let Some((waiting, msg)) = &mut self.net_test
            && *waiting == id
        {
            *msg = match result {
                Ok(r) => trf(
                    "Funciona: respuesta {} con {} bytes",
                    &[&r.status.to_string(), &r.body.len().to_string()],
                ),
                Err(e) => trf("No anduvo: {}", &[e]),
            };
            self.dirty = true;
        }
    }
}

/// La regla que bloquea todo lo de una app.
fn app_rule(name: &str) -> Rule {
    Rule {
        action: Action::Deny,
        dir: Dir::Out,
        host: None,
        port: None,
        app: Some(name.to_string()),
    }
}

fn wallpaper_name(w: &Wallpaper) -> String {
    match w {
        Wallpaper::Hud => tr("HUD de JARVIS").into(),
        Wallpaper::Solid(n) => SOLID_COLORS
            .get(*n)
            .map_or(tr("Color").into(), |c| c.0.to_string()),
        Wallpaper::Image(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
    }
}

/// Imágenes BMP en /Imágenes/Fondos y /Imágenes.
fn find_wallpapers<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<String> {
    let mut out = Vec::new();
    for dir in ["/Imágenes/Fondos", "/Imágenes"] {
        for e in fs.list(dir).unwrap_or_default() {
            if !e.is_dir && e.name.to_ascii_lowercase().ends_with(".bmp") {
                out.push(join(dir, &e.name));
            }
        }
    }
    out
}

/// Tamaño total de cada carpeta de la raíz (recorriendo todo adentro).
fn folder_sizes<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<(String, u64)> {
    fn total<D: BlockDevice>(fs: &mut FileSystem<D>, path: &str, depth: u32) -> u64 {
        if depth > 12 {
            return 0;
        }
        fs.list(path)
            .unwrap_or_default()
            .iter()
            .map(|e| {
                if e.is_dir {
                    total(fs, &join(path, &e.name), depth + 1)
                } else {
                    e.size as u64
                }
            })
            .sum()
    }
    let mut out: Vec<(String, u64)> = fs
        .list("/")
        .unwrap_or_default()
        .iter()
        .filter(|e| e.is_dir && !e.name.starts_with('.'))
        .map(|e| e.name.clone())
        .collect::<Vec<_>>()
        .into_iter()
        .map(|name| {
            let size = total(fs, &join("/", &name), 0);
            (format!("/{name}"), size)
        })
        .collect();
    out.sort_by_key(|b| core::cmp::Reverse(b.1));
    out
}
