//! Configuración del sistema: lo que se cambia en la app Configuración y queda guardado en el
//! disco, en `/Sistema/config.ini` (texto `clave=valor`, se puede editar a mano).
//!
//! Las claves que no se conocen se ignoran y las que faltan toman el valor por defecto: así un
//! archivo de una versión vieja (o editado a mano con errores) nunca impide arrancar.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;

use crate::firewall::Firewall;
use crate::i18n::Lang;

pub const PATH: &str = "/Sistema/config.ini";

/// Colores lisos para el fondo de pantalla.
pub const SOLID_COLORS: [(&str, u32); 6] = [
    ("Noche", 0x050b14),
    ("Azul profundo", 0x0a2350),
    ("Grafito", 0x1c1f24),
    ("Bosque", 0x0f2a1d),
    ("Vino", 0x2a0f1a),
    ("Arena", 0x3a3226),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wallpaper {
    /// El HUD de JARVIS: grilla de puntos y luces.
    Hud,
    /// Un color de [`SOLID_COLORS`].
    Solid(usize),
    /// Una imagen BMP del disco (se estira a la pantalla).
    Image(String),
}

/// Tema de colores (Configuración → Apariencia).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    /// El HUD oscuro de siempre.
    Hud,
    Light,
    Contrast,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 3] = [ThemeKind::Hud, ThemeKind::Light, ThemeKind::Contrast];

    pub fn code(self) -> &'static str {
        match self {
            ThemeKind::Hud => "hud",
            ThemeKind::Light => "claro",
            ThemeKind::Contrast => "contraste",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Hud => "HUD oscuro",
            ThemeKind::Light => "Claro",
            ThemeKind::Contrast => "Alto contraste",
        }
    }
}

/// Cómo aparecen y se van las ventanas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimStyle {
    None,
    Fade,
    Zoom,
    Slide,
}

impl AnimStyle {
    pub const ALL: [AnimStyle; 4] = [
        AnimStyle::None,
        AnimStyle::Fade,
        AnimStyle::Zoom,
        AnimStyle::Slide,
    ];

    pub fn code(self) -> &'static str {
        match self {
            AnimStyle::None => "ninguna",
            AnimStyle::Fade => "desvanecer",
            AnimStyle::Zoom => "zoom",
            AnimStyle::Slide => "deslizar",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AnimStyle::None => "Ninguna",
            AnimStyle::Fade => "Desvanecer",
            AnimStyle::Zoom => "Zoom",
            AnimStyle::Slide => "Deslizar",
        }
    }
}

/// Qué hace el doble clic en la barra de título.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleDouble {
    Maximize,
    Minimize,
    Nothing,
}

impl TitleDouble {
    pub const ALL: [TitleDouble; 3] = [
        TitleDouble::Maximize,
        TitleDouble::Minimize,
        TitleDouble::Nothing,
    ];

    pub fn code(self) -> &'static str {
        match self {
            TitleDouble::Maximize => "maximizar",
            TitleDouble::Minimize => "minimizar",
            TitleDouble::Nothing => "nada",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TitleDouble::Maximize => "Maximizar",
            TitleDouble::Minimize => "Minimizar",
            TitleDouble::Nothing => "Nada",
        }
    }
}

/// Duración de las transiciones según la velocidad elegida (0 lenta, 1 normal, 2 rápida).
pub const ANIM_SPEEDS: [(&str, u64); 3] = [("Lenta", 320), ("Normal", 180), ("Rápida", 100)];

/// Tamaños de letra para la terminal y el editor.
pub const FONT_SIZES: [u8; 3] = [16, 20, 24];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchEngine {
    DuckDuckGo,
    Brave,
    Bing,
    Wikipedia,
}

impl SearchEngine {
    /// Los que devuelven resultados sin JavaScript (Google ya no: pide JavaScript para buscar).
    pub const ALL: [SearchEngine; 4] = [
        SearchEngine::DuckDuckGo,
        SearchEngine::Brave,
        SearchEngine::Bing,
        SearchEngine::Wikipedia,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Brave => "Brave Search",
            SearchEngine::Bing => "Bing",
            SearchEngine::Wikipedia => "Wikipedia",
        }
    }

    /// Dirección de búsqueda (la consulta va al final, ya codificada).
    pub fn url(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "https://html.duckduckgo.com/html/?q=",
            SearchEngine::Brave => "https://search.brave.com/search?q=",
            SearchEngine::Bing => "https://www.bing.com/search?q=",
            SearchEngine::Wikipedia => "https://es.wikipedia.org/w/index.php?search=",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    // Personalización
    pub wallpaper: Wallpaper,
    /// La esfera gira (si no, queda quieta: menos CPU).
    pub animations: bool,
    pub status_panel: bool,
    // Apariencia
    pub theme: ThemeKind,
    /// Índice en [`crate::look::ACCENTS`] (0 = el del tema).
    pub accent: u8,
    // Tipografía
    /// Texto de la interfaz en 20 px (si no, 16).
    pub ui_large: bool,
    pub bold_titles: bool,
    /// Letra de la terminal y del editor, en píxeles (16, 20 o 24).
    pub term_size: u8,
    pub editor_size: u8,
    // Ventanas
    pub anim_style: AnimStyle,
    /// Índice en [`ANIM_SPEEDS`].
    pub anim_speed: u8,
    pub buttons_left: bool,
    pub title_double: TitleDouble,
    /// Arrastrar una ventana contra un borde la acopla (arriba: maximiza).
    pub snap_edges: bool,
    /// Al arrastrar se mueve un contorno y la ventana salta al soltar (más liviano).
    pub drag_outline: bool,
    /// La ventana bajo el mouse toma el foco sin hacer clic.
    pub focus_follows: bool,
    // Barra de tareas
    /// Al maximizar, la barra de arriba con las ventanas y los gráficos.
    pub topbar: bool,
    /// Qué gráficos muestra (bits de `look::STAT_*`).
    pub top_stats: u8,
    pub clock_seconds: bool,
    pub cursor_big: bool,
    // Pantallas (varios monitores)
    pub display_mode: crate::display::Mode,
    /// El segundo monitor va abajo del principal (si no, a la derecha).
    pub display_vertical: bool,
    /// Qué salida es la principal (0 o 1).
    pub display_primary: u8,
    // Fecha y hora
    /// Diferencia con UTC en horas (Argentina: -3).
    pub utc_offset: i8,
    pub clock_24h: bool,
    // Sonido
    /// Pitidos del sistema (avisos). La música suena igual.
    pub sounds: bool,
    // Mouse
    /// 1 (lento) a 5 (rápido). 3 = normal.
    pub mouse_speed: u8,
    /// Renglones por paso de la rueda: 1 a 5.
    pub wheel_lines: u8,
    pub invert_wheel: bool,
    /// Teclado latinoamericano (si no, el de EE. UU.).
    pub latam_keyboard: bool,
    // Navegador
    pub homepage: String,
    pub search: SearchEngine,
    /// Páginas con fondo claro (como un navegador común) u oscuro (el estilo del HUD).
    pub light_pages: bool,
    pub load_images: bool,
    /// Modo lectura: sin menús ni formularios, solo el contenido.
    pub reader_mode: bool,
    // Brave (ADR 0007)
    /// Dónde está el puente de Brave: `host:puerto` (QEMU ve al anfitrión en 10.0.2.2).
    pub brave_server: String,
    /// Token del puente, si corre en otra máquina con `--red` (vacío = local).
    pub brave_token: String,
    pub brave_home: String,
    /// El navegador principal es Brave (si no, el navegador simple de JARVIS).
    pub brave_default: bool,
    // Sincronización (ADR 0007)
    /// El código de emparejado (vacío = no sincroniza). El mismo en las dos máquinas.
    pub sync_code: String,
    /// Dónde está el relé: `host:puerto`.
    pub sync_relay: String,
    // Cuentas y seguridad
    pub user: String,
    pub hostname: String,
    /// PIN para desbloquear (vacío = sin PIN).
    pub pin: String,
    /// Bloquear solo después de estos minutos sin usar (0 = nunca).
    pub lock_minutes: u32,
    /// Qué conexiones se permiten (ver [`crate::firewall`]).
    pub firewall: Firewall,
    /// Idioma de la interfaz.
    pub language: Lang,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            wallpaper: Wallpaper::Hud,
            animations: true,
            status_panel: true,
            theme: ThemeKind::Hud,
            accent: 0,
            ui_large: false,
            bold_titles: false,
            term_size: 16,
            editor_size: 16,
            anim_style: AnimStyle::Zoom,
            anim_speed: 1,
            buttons_left: false,
            title_double: TitleDouble::Maximize,
            snap_edges: true,
            drag_outline: false,
            focus_follows: false,
            topbar: true,
            top_stats: crate::look::STATS_ALL,
            clock_seconds: false,
            cursor_big: false,
            display_mode: crate::display::Mode::Extend,
            display_vertical: false,
            display_primary: 0,
            utc_offset: -3,
            clock_24h: true,
            sounds: true,
            mouse_speed: 3,
            wheel_lines: 3,
            invert_wheel: false,
            latam_keyboard: true,
            homepage: "about:inicio".into(),
            search: SearchEngine::DuckDuckGo,
            light_pages: true,
            load_images: true,
            reader_mode: false,
            brave_server: "10.0.2.2:8119".into(),
            brave_token: String::new(),
            brave_home: "https://search.brave.com/".into(),
            brave_default: true,
            sync_code: String::new(),
            sync_relay: "10.0.2.2:8120".into(),
            user: "roman".into(),
            hostname: "jarvis".into(),
            pin: String::new(),
            lock_minutes: 0,
            firewall: Firewall::default(),
            language: Lang::Es,
        }
    }
}

fn yes(v: &str) -> bool {
    matches!(v.trim(), "si" | "sí" | "1" | "true" | "on")
}

fn yn(b: bool) -> &'static str {
    if b { "si" } else { "no" }
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "fondo" => {
                    c.wallpaper = if v == "hud" {
                        Wallpaper::Hud
                    } else if let Some(n) = v.strip_prefix("color:") {
                        Wallpaper::Solid(
                            n.parse::<usize>().unwrap_or(0).min(SOLID_COLORS.len() - 1),
                        )
                    } else if let Some(p) = v.strip_prefix("imagen:") {
                        Wallpaper::Image(p.into())
                    } else {
                        Wallpaper::Hud
                    }
                }
                "animaciones" => c.animations = yes(v),
                "panel_estado" => c.status_panel = yes(v),
                "tema" => {
                    c.theme = ThemeKind::ALL
                        .into_iter()
                        .find(|t| t.code() == v)
                        .unwrap_or(ThemeKind::Hud)
                }
                "acento" => {
                    c.accent = v
                        .parse::<u8>()
                        .unwrap_or(0)
                        .min(crate::look::ACCENTS.len() as u8 - 1)
                }
                "texto_grande" => c.ui_large = yes(v),
                "titulos_negrita" => c.bold_titles = yes(v),
                "letra_terminal" => c.term_size = font_size(v),
                "letra_editor" => c.editor_size = font_size(v),
                "animacion_ventanas" => {
                    c.anim_style = AnimStyle::ALL
                        .into_iter()
                        .find(|a| a.code() == v)
                        .unwrap_or(AnimStyle::Zoom)
                }
                "velocidad_animaciones" => {
                    c.anim_speed = match v {
                        "lenta" => 0,
                        "rapida" | "rápida" => 2,
                        _ => 1,
                    }
                }
                "botones" => c.buttons_left = v == "izquierda",
                "doble_clic_titulo" => {
                    c.title_double = TitleDouble::ALL
                        .into_iter()
                        .find(|t| t.code() == v)
                        .unwrap_or(TitleDouble::Maximize)
                }
                "acoplar_bordes" => c.snap_edges = yes(v),
                "arrastrar_contorno" => c.drag_outline = yes(v),
                "foco_sigue_mouse" => c.focus_follows = yes(v),
                "barra_arriba" => c.topbar = yes(v),
                "estadisticas" => c.top_stats = parse_stats(v),
                "reloj_segundos" => c.clock_seconds = yes(v),
                "cursor_grande" => c.cursor_big = yes(v),
                "pantallas_modo" => {
                    c.display_mode = crate::display::Mode::ALL
                        .into_iter()
                        .find(|m| m.code() == v)
                        .unwrap_or(crate::display::Mode::Extend)
                }
                "pantallas_vertical" => c.display_vertical = yes(v),
                "pantalla_principal" => c.display_primary = if v.trim() == "2" { 1 } else { 0 },
                "zona_utc" => c.utc_offset = v.parse::<i8>().unwrap_or(-3).clamp(-12, 14),
                "reloj_24h" => c.clock_24h = yes(v),
                "sonidos" => c.sounds = yes(v),
                "mouse_velocidad" => c.mouse_speed = v.parse::<u8>().unwrap_or(3).clamp(1, 5),
                "rueda_renglones" => c.wheel_lines = v.parse::<u8>().unwrap_or(3).clamp(1, 5),
                "rueda_invertida" => c.invert_wheel = yes(v),
                "teclado" => c.latam_keyboard = v != "us",
                "pagina_inicio" if !v.is_empty() => c.homepage = v.into(),
                "buscador" => {
                    c.search = SearchEngine::ALL
                        .into_iter()
                        .find(|s| s.name().eq_ignore_ascii_case(v))
                        .unwrap_or(SearchEngine::DuckDuckGo)
                }
                "paginas_claras" => c.light_pages = yes(v),
                "imagenes" => c.load_images = yes(v),
                "modo_lectura" => c.reader_mode = yes(v),
                "brave_servidor" if parse_server(v).is_some() => c.brave_server = v.into(),
                "brave_token" if v.len() <= 64 && !v.contains(char::is_whitespace) => {
                    c.brave_token = v.into()
                }
                "brave_inicio" if !v.is_empty() => c.brave_home = v.into(),
                "navegador_principal" => c.brave_default = v != "simple",
                "sync_codigo" if v.is_empty() || jarvis_sync::pair::normalize(v).is_some() => {
                    c.sync_code = v.into()
                }
                "sync_rele" if parse_server(v).is_some() => c.sync_relay = v.into(),
                "usuario" if valid_name(v) => c.user = v.into(),
                "equipo" if valid_name(v) => c.hostname = v.into(),
                "pin" if v.chars().all(|ch| ch.is_ascii_digit()) && v.len() <= 8 => {
                    c.pin = v.into()
                }
                "bloquear_minutos" => c.lock_minutes = v.parse::<u32>().unwrap_or(0).min(240),
                "idioma" => c.language = Lang::from_code(v),
                k => {
                    c.firewall.parse_key(k, v);
                }
            }
        }
        c
    }

    pub fn serialize(&self) -> String {
        let fondo = match &self.wallpaper {
            Wallpaper::Hud => "hud".to_string(),
            Wallpaper::Solid(n) => format!("color:{n}"),
            Wallpaper::Image(p) => format!("imagen:{p}"),
        };
        let lines: Vec<String> = alloc::vec![
            "# Configuración de JARVIS-OS (la escribe la app Configuración).".into(),
            format!("fondo={fondo}"),
            format!("animaciones={}", yn(self.animations)),
            format!("panel_estado={}", yn(self.status_panel)),
            format!("tema={}", self.theme.code()),
            format!("acento={}", self.accent),
            format!("texto_grande={}", yn(self.ui_large)),
            format!("titulos_negrita={}", yn(self.bold_titles)),
            format!("letra_terminal={}", self.term_size),
            format!("letra_editor={}", self.editor_size),
            format!("animacion_ventanas={}", self.anim_style.code()),
            format!(
                "velocidad_animaciones={}",
                ["lenta", "normal", "rapida"][self.anim_speed.min(2) as usize]
            ),
            format!(
                "botones={}",
                if self.buttons_left {
                    "izquierda"
                } else {
                    "derecha"
                }
            ),
            format!("doble_clic_titulo={}", self.title_double.code()),
            format!("acoplar_bordes={}", yn(self.snap_edges)),
            format!("arrastrar_contorno={}", yn(self.drag_outline)),
            format!("foco_sigue_mouse={}", yn(self.focus_follows)),
            format!("barra_arriba={}", yn(self.topbar)),
            format!("estadisticas={}", stats_text(self.top_stats)),
            format!("reloj_segundos={}", yn(self.clock_seconds)),
            format!("cursor_grande={}", yn(self.cursor_big)),
            format!("pantallas_modo={}", self.display_mode.code()),
            format!("pantallas_vertical={}", yn(self.display_vertical)),
            format!("pantalla_principal={}", self.display_primary + 1),
            format!("zona_utc={}", self.utc_offset),
            format!("reloj_24h={}", yn(self.clock_24h)),
            format!("sonidos={}", yn(self.sounds)),
            format!("mouse_velocidad={}", self.mouse_speed),
            format!("rueda_renglones={}", self.wheel_lines),
            format!("rueda_invertida={}", yn(self.invert_wheel)),
            format!(
                "teclado={}",
                if self.latam_keyboard { "latam" } else { "us" }
            ),
            format!("pagina_inicio={}", self.homepage),
            format!("buscador={}", self.search.name()),
            format!("paginas_claras={}", yn(self.light_pages)),
            format!("imagenes={}", yn(self.load_images)),
            format!("modo_lectura={}", yn(self.reader_mode)),
            format!("brave_servidor={}", self.brave_server),
            format!("brave_token={}", self.brave_token),
            format!("brave_inicio={}", self.brave_home),
            format!(
                "navegador_principal={}",
                if self.brave_default {
                    "brave"
                } else {
                    "simple"
                }
            ),
            format!("sync_codigo={}", self.sync_code),
            format!("sync_rele={}", self.sync_relay),
            format!("usuario={}", self.user),
            format!("equipo={}", self.hostname),
            format!("pin={}", self.pin),
            format!("bloquear_minutos={}", self.lock_minutes),
            format!("idioma={}", self.language.code()),
        ];
        let mut lines = lines;
        lines.extend(self.firewall.serialize());
        let mut s = lines.join("\n");
        s.push('\n');
        s
    }

    /// Duración de las transiciones de ventanas (0 = sin transiciones).
    pub fn anim_ms(&self) -> u64 {
        if !self.animations || self.anim_style == AnimStyle::None {
            return 0;
        }
        ANIM_SPEEDS[self.anim_speed.min(2) as usize].1
    }

    /// Multiplica el movimiento del mouse (en cuartos: 4 = sin cambio).
    pub fn mouse_factor(&self) -> i32 {
        [2, 3, 4, 6, 8][(self.mouse_speed.clamp(1, 5) - 1) as usize]
    }

    pub fn wallpaper_color(&self) -> Option<Color> {
        match self.wallpaper {
            Wallpaper::Solid(n) => SOLID_COLORS.get(n).map(|c| Color::hex(c.1)),
            _ => None,
        }
    }

    /// "UTC-03:00"
    pub fn zone_label(&self) -> String {
        let sign = if self.utc_offset < 0 { '-' } else { '+' };
        format!("UTC{sign}{:02}:00", self.utc_offset.unsigned_abs())
    }
}

fn font_size(v: &str) -> u8 {
    let n = v.parse::<u8>().unwrap_or(16);
    *FONT_SIZES
        .iter()
        .min_by_key(|s| s.abs_diff(n))
        .unwrap_or(&16)
}

const STAT_NAMES: [(&str, u8); 5] = [
    ("cpu", crate::look::STAT_CPU),
    ("mem", crate::look::STAT_MEM),
    ("disco", crate::look::STAT_DISK),
    ("red", crate::look::STAT_NET),
    ("temp", crate::look::STAT_TEMP),
];

/// `"cpu,red"` → bits. Vacío = ninguno.
fn parse_stats(v: &str) -> u8 {
    v.split(',')
        .filter_map(|n| STAT_NAMES.iter().find(|(k, _)| *k == n.trim()))
        .fold(0, |m, (_, b)| m | b)
}

fn stats_text(m: u8) -> String {
    STAT_NAMES
        .iter()
        .filter(|(_, b)| m & b != 0)
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(",")
}

/// `"10.0.2.2:8119"` → `("10.0.2.2", 8119)`. El puerto es obligatorio.
pub fn parse_server(s: &str) -> Option<(&str, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|&p| p != 0)?;
    let ok = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some((host, port))
}

/// Nombres de usuario y de equipo: letras, números y guiones (como en Linux).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 24
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        let c = Config {
            wallpaper: Wallpaper::Image("/Imágenes/Fondos/aurora.bmp".into()),
            utc_offset: 5,
            clock_24h: false,
            mouse_speed: 5,
            search: SearchEngine::Wikipedia,
            pin: "1234".into(),
            lock_minutes: 5,
            language: Lang::Pt,
            brave_server: "brave.casa.lan:9000".into(),
            brave_token: "secreto123".into(),
            brave_default: false,
            sync_code: "ABCDE-FGHJK-LMNPQ-RSTUV".into(),
            sync_relay: "rele.ejemplo.com:443".into(),
            theme: ThemeKind::Light,
            accent: 3,
            ui_large: true,
            term_size: 24,
            anim_style: AnimStyle::Slide,
            anim_speed: 2,
            buttons_left: true,
            title_double: TitleDouble::Minimize,
            snap_edges: false,
            focus_follows: true,
            top_stats: crate::look::STAT_CPU | crate::look::STAT_TEMP,
            clock_seconds: true,
            cursor_big: true,
            display_mode: crate::display::Mode::Duplicate,
            display_vertical: true,
            display_primary: 1,
            ..Config::default()
        };
        let mut c = c;
        c.firewall
            .ufw(&["deny", "out", "to", "ejemplo.com", "port", "443"])
            .unwrap();
        assert_eq!(Config::parse(&c.serialize()), c);
    }

    #[test]
    fn valores_rotos_toman_el_defecto() {
        let c = Config::parse(
            "basura\nzona_utc=99\nmouse_velocidad=hola\npin=12ab\nusuario=con espacios\nfondo=color:40\nclave_nueva=1",
        );
        assert_eq!(c.utc_offset, 14);
        assert_eq!(c.mouse_speed, 3);
        assert_eq!(c.pin, "");
        assert_eq!(c.user, "roman");
        assert_eq!(c.wallpaper, Wallpaper::Solid(SOLID_COLORS.len() - 1));
        assert_eq!(Config::default().zone_label(), "UTC-03:00");
        let c = Config::parse("brave_servidor=sin-puerto\nbrave_token=con espacio");
        assert_eq!(c.brave_server, "10.0.2.2:8119");
        assert_eq!(c.brave_token, "");
        assert_eq!(parse_server("10.0.2.2:8119"), Some(("10.0.2.2", 8119)));
        assert_eq!(parse_server("x:0"), None);
        let c = Config::parse("tema=violeta\nletra_terminal=19\nestadisticas=\nacento=99");
        assert_eq!(c.theme, ThemeKind::Hud);
        assert_eq!(c.term_size, 20, "el tamaño más cercano");
        assert_eq!(c.top_stats, 0);
        assert_eq!(c.accent as usize, crate::look::ACCENTS.len() - 1);
        assert_eq!(Config::default().anim_ms(), 180);
    }
}
