//! La app Archivos: estado y operaciones (sin dibujo; eso está en `files_view`).
//!
//! Regla de seguridad (la misma que el cerebro de JARVIS): **borrar = mover a la Papelera**. El
//! borrado definitivo solo existe dentro de la Papelera y siempre pide confirmación.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, DirEntry, FileSystem, FsError, Timestamp};

use crate::input::Key;
use crate::text_input::TextInput;

pub const TRASH: &str = "/Papelera";
/// Accesos rápidos del panel lateral: (nombre, ruta).
pub const SHORTCUTS: [(&str, &str); 6] = [
    ("Inicio", "/"),
    ("Documentos", "/Documentos"),
    ("Proyectos", "/Proyectos"),
    ("Facultad", "/Facultad"),
    ("Imágenes", "/Imágenes"),
    ("Papelera", TRASH),
];
const MAX_NAME: usize = 64;
const PREVIEW_BYTES: usize = 2048;
pub const PREVIEW_LINES: usize = 14;
const TOAST_MS: u64 = 4000;

/// Tipo de archivo, para el ícono y la descripción.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Trash,
    Text,
    Code,
    Image,
    Archive,
    Binary,
}

impl Kind {
    pub fn of(entry: &DirEntry, path: &str) -> Kind {
        if entry.is_dir {
            return if path == TRASH {
                Kind::Trash
            } else {
                Kind::Folder
            };
        }
        let ext = entry
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "txt" | "md" | "log" | "csv" => Kind::Text,
            "rs" | "py" | "c" | "h" | "js" | "ts" | "json" | "toml" | "sh" | "html" | "css" => {
                Kind::Code
            }
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" => Kind::Image,
            "zip" | "tar" | "gz" | "7z" | "rar" => Kind::Archive,
            _ => Kind::Binary,
        }
    }

    pub fn description(self, entry: &DirEntry) -> &'static str {
        match self {
            Kind::Folder => "carpeta",
            Kind::Trash => "papelera",
            Kind::Text if entry.name.to_ascii_lowercase().ends_with(".md") => "markdown",
            Kind::Text => "texto",
            Kind::Code => "código",
            Kind::Image => "imagen",
            Kind::Archive => "archivo comprimido",
            Kind::Binary => "archivo",
        }
    }
}

pub enum Dialog {
    NewFolder(TextInput),
    NewFile(TextInput),
    Rename { original: String, input: TextInput },
    ConfirmTrash(String),
    ConfirmDelete(String),
    ConfirmEmptyTrash,
}

impl Dialog {
    pub fn title(&self) -> &'static str {
        match self {
            Dialog::NewFolder(_) => "NUEVA CARPETA",
            Dialog::NewFile(_) => "NUEVO ARCHIVO DE TEXTO",
            Dialog::Rename { .. } => "RENOMBRAR",
            Dialog::ConfirmTrash(_) => "MOVER A LA PAPELERA",
            Dialog::ConfirmDelete(_) => "BORRAR DEFINITIVAMENTE",
            Dialog::ConfirmEmptyTrash => "VACIAR LA PAPELERA",
        }
    }

    pub fn accept_label(&self) -> &'static str {
        match self {
            Dialog::NewFolder(_) | Dialog::NewFile(_) => "CREAR",
            Dialog::Rename { .. } => "RENOMBRAR",
            Dialog::ConfirmTrash(_) => "MOVER",
            Dialog::ConfirmDelete(_) | Dialog::ConfirmEmptyTrash => "BORRAR",
        }
    }

    /// Las que borran definitivamente se muestran en rojo.
    pub fn is_destructive(&self) -> bool {
        matches!(self, Dialog::ConfirmDelete(_) | Dialog::ConfirmEmptyTrash)
    }

    pub fn input(&self) -> Option<&TextInput> {
        match self {
            Dialog::NewFolder(i) | Dialog::NewFile(i) | Dialog::Rename { input: i, .. } => Some(i),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Dialog::NewFolder(_) | Dialog::NewFile(_) | Dialog::Rename { .. } => "Nombre:".into(),
            Dialog::ConfirmTrash(n) => format!("¿Mover \"{n}\" a la Papelera?"),
            Dialog::ConfirmDelete(n) => format!("\"{n}\" se borra para siempre."),
            Dialog::ConfirmEmptyTrash => {
                "Todo lo que está en la Papelera se borra para siempre.".into()
            }
        }
    }
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    until_ms: u64,
}

/// Vista previa del archivo seleccionado.
pub enum Preview {
    Text(Vec<String>),
    Binary,
}

pub struct FilesApp {
    pub cwd: String,
    pub entries: Vec<DirEntry>,
    pub selected: usize,
    pub scroll: usize,
    history: Vec<String>,
    pub dialog: Option<Dialog>,
    pub preview: Option<Preview>,
    pub toast: Option<Toast>,
    pub label: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
    /// La ventana cambió y hay que redibujarla.
    pub dirty: bool,
    pub opened: bool,
}

pub fn join(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((p, _)) => p.into(),
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn sort_key(e: &DirEntry) -> (bool, String) {
    (
        !e.is_dir,
        e.name.chars().flat_map(char::to_lowercase).collect(),
    )
}

/// "notas.txt" + 2 → "notas (2).txt"
fn with_suffix(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => format!("{base} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

impl Default for FilesApp {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesApp {
    pub fn new() -> Self {
        FilesApp {
            cwd: "/".into(),
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            history: Vec::new(),
            dialog: None,
            preview: None,
            toast: None,
            label: String::new(),
            free_bytes: 0,
            total_bytes: 0,
            dirty: true,
            opened: false,
        }
    }

    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.entries.get(self.selected)
    }

    pub fn selected_path(&self) -> Option<String> {
        self.selected_entry().map(|e| join(&self.cwd, &e.name))
    }

    pub fn in_trash(&self) -> bool {
        self.cwd == TRASH || self.cwd.starts_with("/Papelera/")
    }

    fn notify(&mut self, text: impl Into<String>, error: bool, now_ms: u64, log: &mut Vec<String>) {
        let text = text.into();
        if error {
            log.push(format!("ARCHIVOS_ERROR {text}"));
        }
        self.toast = Some(Toast {
            text,
            error,
            until_ms: now_ms + TOAST_MS,
        });
        self.dirty = true;
    }

    /// Borra el aviso cuando vence. Devuelve `true` si hay que redibujar.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        if self.toast.as_ref().is_some_and(|t| now_ms >= t.until_ms) {
            self.toast = None;
            self.dirty = true;
        }
        self.dirty
    }

    /// Abre una carpeta (sin tocar el historial).
    pub fn open<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        path: &str,
        now_ms: u64,
        log: &mut Vec<String>,
    ) -> bool {
        match fs.list(path) {
            Ok(mut entries) => {
                entries.sort_by_key(sort_key);
                self.cwd = path.into();
                self.entries = entries;
                self.selected = 0;
                self.scroll = 0;
                self.opened = true;
                self.refresh_disk_info(fs);
                self.update_preview(fs);
                self.dirty = true;
                log.push(format!("ARCHIVOS_ABIERTO {path}"));
                true
            }
            Err(e) => {
                self.notify(format!("No se pudo abrir {path}: {e}"), true, now_ms, log);
                false
            }
        }
    }

    /// Navega a una carpeta guardando la actual en el historial.
    pub fn navigate<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        path: &str,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        let from = self.cwd.clone();
        if from != path && self.open(fs, path, now_ms, log) {
            self.history.push(from);
        }
    }

    /// Vuelve a leer la carpeta actual, conservando la selección por nombre si sigue existiendo.
    pub fn reload<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>, select: Option<&str>) {
        let keep = select
            .map(String::from)
            .or_else(|| self.selected_entry().map(|e| e.name.clone()));
        if let Ok(mut entries) = fs.list(&self.cwd) {
            entries.sort_by_key(sort_key);
            self.entries = entries;
        }
        if let Some(name) = keep
            && let Some(i) = self.entries.iter().position(|e| e.name == name)
        {
            self.selected = i;
        }
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.refresh_disk_info(fs);
        self.update_preview(fs);
        self.dirty = true;
    }

    fn refresh_disk_info<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>) {
        self.label = fs.label().to_string();
        self.free_bytes = fs.free_bytes();
        self.total_bytes = fs.total_bytes();
    }

    fn update_preview<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>) {
        self.preview = None;
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.is_dir {
            return;
        }
        let Some(path) = self.selected_path() else {
            return;
        };
        let Ok(bytes) = fs.read_prefix(&path, PREVIEW_BYTES) else {
            return;
        };
        let text = String::from_utf8_lossy(&bytes);
        let control = text
            .chars()
            .filter(|&c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            .count();
        if control > text.len() / 20 || text.contains('\u{FFFD}') && bytes.len() < PREVIEW_BYTES {
            self.preview = Some(Preview::Binary);
            return;
        }
        let lines = text
            .lines()
            .take(PREVIEW_LINES)
            .map(|l| l.replace('\t', "    ").trim_end_matches('\r').into())
            .collect();
        self.preview = Some(Preview::Text(lines));
    }

    pub fn select<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        index: usize,
        log: &mut Vec<String>,
    ) {
        if index < self.entries.len() && index != self.selected {
            self.selected = index;
            self.update_preview(fs);
            self.dirty = true;
            log.push(format!("ARCHIVOS_SELECCION {}", self.entries[index].name));
        }
    }

    fn move_selection<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        delta: isize,
        log: &mut Vec<String>,
    ) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len() as isize - 1;
        let target = (self.selected as isize + delta).clamp(0, max) as usize;
        self.select(fs, target, log);
    }

    /// Mantiene la fila seleccionada visible cuando entran `visible` filas.
    pub fn ensure_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    /// Abre la carpeta seleccionada.
    pub fn enter_selected<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.is_dir {
            if let Some(path) = self.selected_path() {
                self.navigate(fs, &path, now_ms, log);
            }
        } else {
            self.notify(
                "La vista previa está en el inspector (todavía no hay editor).",
                false,
                now_ms,
                log,
            );
        }
    }

    pub fn go_up<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        if self.cwd != "/" {
            let child = basename(&self.cwd).to_string();
            let up = parent(&self.cwd);
            self.navigate(fs, &up, now_ms, log);
            self.reload(fs, Some(&child));
        }
    }

    pub fn go_back<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        if let Some(prev) = self.history.pop() {
            self.open(fs, &prev, now_ms, log);
        }
    }

    // --- diálogos -----------------------------------------------------------------------------

    pub fn start_new_folder(&mut self) {
        self.dialog = Some(Dialog::NewFolder(TextInput::new("", MAX_NAME)));
        self.dirty = true;
    }

    pub fn start_new_file(&mut self) {
        self.dialog = Some(Dialog::NewFile(TextInput::new("nota.txt", MAX_NAME)));
        self.dirty = true;
    }

    pub fn start_rename(&mut self) {
        if let Some(e) = self.selected_entry() {
            let name = e.name.clone();
            self.dialog = Some(Dialog::Rename {
                input: TextInput::new(&name, MAX_NAME),
                original: name,
            });
            self.dirty = true;
        }
    }

    /// Supr: a la Papelera, o borrado definitivo si ya está en la Papelera.
    pub fn start_delete(&mut self, now_ms: u64, log: &mut Vec<String>) {
        let Some(e) = self.selected_entry() else {
            return;
        };
        let name = e.name.clone();
        if self.cwd == "/" && name.eq_ignore_ascii_case("Papelera") {
            self.notify(
                "La Papelera no se puede mover a la Papelera.",
                true,
                now_ms,
                log,
            );
            return;
        }
        self.dialog = Some(if self.in_trash() {
            Dialog::ConfirmDelete(name)
        } else {
            Dialog::ConfirmTrash(name)
        });
        self.dirty = true;
    }

    pub fn start_empty_trash(&mut self) {
        if self.in_trash() {
            self.dialog = Some(Dialog::ConfirmEmptyTrash);
            self.dirty = true;
        }
    }

    pub fn cancel_dialog(&mut self) {
        if self.dialog.take().is_some() {
            self.dirty = true;
        }
    }

    /// Ejecuta la acción del diálogo abierto.
    pub fn accept_dialog<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now: Timestamp,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        self.dirty = true;
        let result = match &dialog {
            Dialog::NewFolder(input) => {
                let path = join(&self.cwd, input.text.trim());
                fs.mkdir(&path, now).map(|()| {
                    log.push(format!("ARCHIVOS_CREADO {path}"));
                    (
                        format!("Carpeta \"{}\" creada.", input.text.trim()),
                        Some(input.text.trim().to_string()),
                    )
                })
            }
            Dialog::NewFile(input) => {
                let path = join(&self.cwd, input.text.trim());
                let content = format!(
                    "Archivo creado desde JARVIS-OS el {:02}/{:02}/{} a las {:02}:{:02}.\n",
                    now.day, now.month, now.year, now.hour, now.minute
                );
                fs.create_file(&path, content.as_bytes(), now).map(|()| {
                    log.push(format!("ARCHIVOS_CREADO {path}"));
                    (
                        format!("Archivo \"{}\" creado.", input.text.trim()),
                        Some(input.text.trim().to_string()),
                    )
                })
            }
            Dialog::Rename { original, input } => {
                let path = join(&self.cwd, original);
                fs.rename(&path, input.text.trim()).map(|()| {
                    log.push(format!(
                        "ARCHIVOS_RENOMBRADO {path} -> {}",
                        input.text.trim()
                    ));
                    (
                        format!("Renombrado a \"{}\".", input.text.trim()),
                        Some(input.text.trim().to_string()),
                    )
                })
            }
            Dialog::ConfirmTrash(name) => {
                let path = join(&self.cwd, name);
                self.move_to_trash(fs, &path, now).map(|dest| {
                    log.push(format!("ARCHIVOS_PAPELERA {path} -> {dest}"));
                    (format!("\"{name}\" está en la Papelera."), None)
                })
            }
            Dialog::ConfirmDelete(name) => {
                let path = join(&self.cwd, name);
                fs.remove(&path).map(|()| {
                    log.push(format!("ARCHIVOS_BORRADO {path}"));
                    (format!("\"{name}\" se borró."), None)
                })
            }
            Dialog::ConfirmEmptyTrash => {
                let result = fs.list(TRASH).and_then(|items| {
                    items
                        .iter()
                        .try_for_each(|e| fs.remove(&join(TRASH, &e.name)))
                });
                result.map(|()| {
                    log.push("ARCHIVOS_PAPELERA_VACIA".into());
                    ("La Papelera está vacía.".into(), None)
                })
            }
        };
        match result {
            Ok((msg, select)) => {
                self.reload(fs, select.as_deref());
                self.notify(msg, false, now_ms, log);
            }
            Err(e) => {
                // Si el nombre no sirvió, el diálogo queda abierto para corregirlo.
                if matches!(e, FsError::AlreadyExists | FsError::InvalidName)
                    && dialog.input().is_some()
                {
                    self.dialog = Some(dialog);
                }
                self.notify(error_message(e), true, now_ms, log);
            }
        }
    }

    /// Mueve a /Papelera. Si ya hay algo con ese nombre, agrega " (2)", " (3)"…
    fn move_to_trash<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        path: &str,
        now: Timestamp,
    ) -> Result<String, FsError> {
        if !fs.exists(TRASH) {
            fs.mkdir(TRASH, now)?;
        }
        let name = basename(path).to_string();
        let dir = parent(path);
        let mut target = name.clone();
        let mut n = 2;
        while fs.exists(&join(TRASH, &target))
            || (target != name && fs.exists(&join(&dir, &target)))
        {
            target = with_suffix(&name, n);
            n += 1;
        }
        let mut current = path.to_string();
        if target != name {
            fs.rename(path, &target)?;
            current = join(&dir, &target);
        }
        fs.move_to(&current, TRASH)?;
        Ok(join(TRASH, &target))
    }

    /// Teclas de la app. Devuelve `false` si la tecla no se usó (Esc sin diálogo: la usa el
    /// escritorio para volver a JARVIS).
    pub fn handle_key<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        key: Key,
        now: Timestamp,
        now_ms: u64,
        log: &mut Vec<String>,
    ) -> bool {
        if let Some(dialog) = self.dialog.as_mut() {
            match key {
                Key::Escape => self.cancel_dialog(),
                Key::Enter => self.accept_dialog(fs, now, now_ms, log),
                other => {
                    let changed = match dialog {
                        Dialog::NewFolder(i)
                        | Dialog::NewFile(i)
                        | Dialog::Rename { input: i, .. } => i.handle(other),
                        _ => false,
                    };
                    self.dirty |= changed;
                }
            }
            return true;
        }
        match key {
            Key::Up => self.move_selection(fs, -1, log),
            Key::Down => self.move_selection(fs, 1, log),
            Key::PageUp => self.move_selection(fs, -10, log),
            Key::PageDown => self.move_selection(fs, 10, log),
            Key::Home => self.move_selection(fs, isize::MIN / 2, log),
            Key::End => self.move_selection(fs, isize::MAX / 2, log),
            Key::Enter | Key::Right => self.enter_selected(fs, now_ms, log),
            Key::Backspace => self.go_up(fs, now_ms, log),
            Key::Left => self.go_back(fs, now_ms, log),
            Key::F7 => self.start_new_folder(),
            Key::F6 => self.start_new_file(),
            Key::F2 => self.start_rename(),
            Key::Delete => self.start_delete(now_ms, log),
            _ => return false,
        }
        true
    }
}

/// Mensaje para el usuario, con mayúscula inicial.
pub fn error_message(e: FsError) -> String {
    let msg = e.to_string();
    let mut chars = msg.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>() + ".",
        None => msg,
    }
}

/// "4 B", "12.3 KiB", "1.5 MiB".
pub fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{}.{} KiB", bytes / KIB, bytes % KIB * 10 / KIB)
    } else {
        format!("{}.{} MiB", bytes / MIB, bytes % MIB * 10 / MIB)
    }
}

/// "23/09/2026 22:41"
pub fn format_date(t: Timestamp) -> String {
    format!(
        "{:02}/{:02}/{} {:02}:{:02}",
        t.day, t.month, t.year, t.hour, t.minute
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(with_suffix("notas.txt", 2), "notas (2).txt");
        assert_eq!(with_suffix("Carpeta", 3), "Carpeta (3)");
        assert_eq!(with_suffix(".config", 2), ".config (2)");
    }

    #[test]
    fn formatos() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(64 * 1024 * 1024), "64.0 MiB");
        let t = Timestamp {
            year: 2026,
            month: 9,
            day: 3,
            hour: 7,
            minute: 5,
            second: 0,
        };
        assert_eq!(format_date(t), "03/09/2026 07:05");
        assert_eq!(
            error_message(FsError::NoSpace),
            "No queda espacio en el disco."
        );
    }
}
