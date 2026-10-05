//! Aplicaciones predeterminadas (Configuración → Aplicaciones predeterminadas): con qué app se
//! abre cada tipo de archivo, como en Windows.
//!
//! Es el único lugar que lo decide: lo usan Archivos (doble clic), las acciones de JARVIS
//! (`abrir_archivo`) y `open` en la terminal. Para cada tipo hay solo las apps que de verdad lo
//! pueden abrir (Brave corre en el anfitrión y no ve el disco de JARVIS-OS: no abre archivos).

use alloc::format;
use alloc::string::String;

use crate::files::parent;
use crate::i18n::tr;
use crate::system::Launch;

/// Los tipos que se pueden configurar, en el orden de la Configuración.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Folder,
    Text,
    Code,
    /// Páginas web guardadas (.html).
    Html,
    Image,
    Video,
    Audio,
}

impl FileType {
    pub const ALL: [FileType; 7] = [
        FileType::Folder,
        FileType::Text,
        FileType::Code,
        FileType::Html,
        FileType::Image,
        FileType::Video,
        FileType::Audio,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FileType::Folder => tr("Carpetas"),
            FileType::Text => tr("Texto (.txt, .md, .log, .csv)"),
            FileType::Code => tr("Código (.rs, .py, .js, .json...)"),
            FileType::Html => tr("Páginas web guardadas (.html)"),
            FileType::Image => tr("Imágenes (.png, .jpg, .bmp...)"),
            FileType::Video => tr("Videos (.avi)"),
            FileType::Audio => tr("Audio (.wav)"),
        }
    }

    /// Las apps que lo pueden abrir; la primera es la de fábrica.
    pub fn handlers(self) -> &'static [Handler] {
        match self {
            FileType::Folder => &[Handler::Files, Handler::Terminal],
            FileType::Text | FileType::Code | FileType::Html => {
                &[Handler::Editor, Handler::Terminal]
            }
            FileType::Image | FileType::Video => &[Handler::Viewer, Handler::Files],
            FileType::Audio => &[Handler::Music, Handler::Viewer, Handler::Files],
        }
    }

    fn index(self) -> usize {
        FileType::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// El nombre en `/Sistema/config.ini` (`app_<código>=...`).
    pub fn code(self) -> &'static str {
        match self {
            FileType::Folder => "carpetas",
            FileType::Text => "texto",
            FileType::Code => "codigo",
            FileType::Html => "html",
            FileType::Image => "imagenes",
            FileType::Video => "videos",
            FileType::Audio => "audio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    Files,
    Editor,
    Viewer,
    Music,
    /// La terminal: `ls` de la carpeta o `cat` del archivo.
    Terminal,
}

impl Handler {
    pub fn name(self) -> &'static str {
        match self {
            Handler::Files => tr("Archivos"),
            Handler::Editor => tr("Editor de texto"),
            Handler::Viewer => tr("Visor"),
            Handler::Music => tr("Música"),
            Handler::Terminal => "Terminal",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Handler::Files => "archivos",
            Handler::Editor => "editor",
            Handler::Viewer => "visor",
            Handler::Music => "musica",
            Handler::Terminal => "terminal",
        }
    }

    fn from_code(code: &str) -> Option<Handler> {
        [
            Handler::Files,
            Handler::Editor,
            Handler::Viewer,
            Handler::Music,
            Handler::Terminal,
        ]
        .into_iter()
        .find(|h| h.code() == code)
    }
}

/// La app elegida para cada tipo (índice en `FileType::handlers`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefaultApps([u8; 7]);

impl DefaultApps {
    pub fn get(&self, t: FileType) -> Handler {
        let options = t.handlers();
        options[(self.0[t.index()] as usize).min(options.len() - 1)]
    }

    /// Pasa a la app siguiente (o anterior, con `delta` -1) de las que pueden abrir `t`.
    pub fn cycle(&mut self, t: FileType, delta: i32) {
        let n = t.handlers().len() as i32;
        let i = &mut self.0[t.index()];
        *i = (i32::from(*i) + delta).rem_euclid(n) as u8;
    }

    /// `app_<tipo>=<app>` de `/Sistema/config.ini`. `false` si la clave no era de acá.
    pub fn parse_key(&mut self, key: &str, value: &str) -> bool {
        let Some(t) = key
            .strip_prefix("app_")
            .and_then(|k| FileType::ALL.into_iter().find(|t| t.code() == k))
        else {
            return false;
        };
        if let Some(i) =
            Handler::from_code(value.trim()).and_then(|h| t.handlers().iter().position(|x| *x == h))
        {
            self.0[t.index()] = i as u8;
        }
        true
    }

    pub fn serialize(&self) -> impl Iterator<Item = String> + '_ {
        FileType::ALL
            .into_iter()
            .map(|t| format!("app_{}={}", t.code(), self.get(t).code()))
    }
}

/// El tipo de un archivo por su extensión (`None`: no es de los configurables).
pub fn file_type(path: &str, is_dir: bool) -> Option<FileType> {
    if is_dir {
        return Some(FileType::Folder);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    Some(match ext.as_str() {
        "txt" | "md" | "log" | "csv" => FileType::Text,
        "rs" | "py" | "c" | "h" | "js" | "ts" | "json" | "toml" | "css" | "ini" | "yaml"
        | "yml" => FileType::Code,
        "html" | "htm" => FileType::Html,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" => FileType::Image,
        "avi" => FileType::Video,
        "wav" => FileType::Audio,
        _ => return None,
    })
}

/// Con qué se abre `path` y el nombre de la app (para los mensajes: "Abrí X en ...").
pub fn launch_for(apps: &DefaultApps, path: &str, is_dir: bool) -> (Launch, &'static str) {
    let lower = path.to_ascii_lowercase();
    // Lo que no se configura: los scripts se ejecutan, los programas de Windows se explican.
    if lower.ends_with(".sh") {
        return (Launch::Terminal(Some(format!("sh '{path}'"))), "Terminal");
    }
    if [".exe", ".msi", ".dll", ".com"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        let cmd = format!("file '{path}' && wine '{path}'");
        return (Launch::Terminal(Some(cmd)), "Terminal");
    }
    if [".zip", ".tar", ".gz", ".7z", ".rar"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return (Launch::Terminal(Some(format!("file '{path}'"))), "Terminal");
    }
    // Sin extensión conocida (README, Makefile...): el editor, que avisa si es binario.
    let Some(t) = file_type(path, is_dir) else {
        return (Launch::Edit(path.into()), Handler::Editor.name());
    };
    let h = apps.get(t);
    let launch = match (h, t) {
        (Handler::Files, FileType::Folder) => Launch::Folder(path.into()),
        // Un archivo "en Archivos": su carpeta.
        (Handler::Files, _) => Launch::Folder(parent(path)),
        (Handler::Terminal, FileType::Folder) => {
            Launch::Terminal(Some(format!("cd '{path}' && ls")))
        }
        (Handler::Terminal, _) => Launch::Terminal(Some(format!("cat '{path}'"))),
        (Handler::Editor, _) => Launch::Edit(path.into()),
        (Handler::Viewer, _) => Launch::View(path.into()),
        (Handler::Music, _) => Launch::Play(path.into()),
    };
    (launch, h.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn de_fabrica_cada_tipo_con_su_app() {
        let d = DefaultApps::default();
        let open = |p: &str, dir: bool| launch_for(&d, p, dir).0;
        assert_eq!(
            open("/Documentos", true),
            Launch::Folder("/Documentos".into())
        );
        assert_eq!(open("/a.txt", false), Launch::Edit("/a.txt".into()));
        assert_eq!(
            open("/p/index.html", false),
            Launch::Edit("/p/index.html".into())
        );
        assert_eq!(open("/i/f.PNG", false), Launch::View("/i/f.PNG".into()));
        assert_eq!(open("/m/t.wav", false), Launch::Play("/m/t.wav".into()));
        assert_eq!(open("/README", false), Launch::Edit("/README".into()));
        assert_eq!(
            open("/s/x.sh", false),
            Launch::Terminal(Some("sh '/s/x.sh'".into()))
        );
    }

    #[test]
    fn se_elige_otra_app_y_se_guarda() {
        let mut d = DefaultApps::default();
        d.cycle(FileType::Text, 1);
        d.cycle(FileType::Image, 1);
        assert_eq!(d.get(FileType::Text), Handler::Terminal);
        assert_eq!(
            launch_for(&d, "/a.txt", false).0,
            Launch::Terminal(Some("cat '/a.txt'".into()))
        );
        assert_eq!(
            launch_for(&d, "/Imágenes/f.png", false),
            (Launch::Folder("/Imágenes".into()), "Archivos")
        );
        // Ida y vuelta por config.ini.
        let mut back = DefaultApps::default();
        for line in d.serialize() {
            let (k, v) = line.split_once('=').unwrap();
            assert!(back.parse_key(k, v));
        }
        assert_eq!(back, d);
        // Una app que no puede abrir ese tipo no se acepta.
        let mut bad = DefaultApps::default();
        assert!(bad.parse_key("app_imagenes", "editor"));
        assert_eq!(bad.get(FileType::Image), Handler::Viewer);
        assert!(!bad.parse_key("otra_cosa", "x"));
        // Dar la vuelta hacia atrás.
        let mut d = DefaultApps::default();
        d.cycle(FileType::Audio, -1);
        assert_eq!(d.get(FileType::Audio), Handler::Files);
    }
}
