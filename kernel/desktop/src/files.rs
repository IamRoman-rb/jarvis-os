//! La app Archivos: estado y operaciones (sin dibujo; eso está en `files_view`).
//!
//! Regla de seguridad (la misma que el cerebro de JARVIS): **borrar = mover a la Papelera**. El
//! borrado definitivo solo existe dentro de la Papelera y siempre pide confirmación.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, DirEntry, FileSystem, FsError, Timestamp};

use crate::i18n::{tr, trf};
use crate::input::{Key, Mods};
use crate::text_input::TextInput;

pub const TRASH: &str = "/Papelera";
/// Adentro de la Papelera: de dónde vino cada cosa ("nombre<TAB>ruta original" por línea), para
/// poder restaurarla. Los nombres que empiezan con "." no se muestran.
pub const TRASH_INDEX: &str = "/Papelera/.origen";
/// Accesos rápidos del panel lateral: (nombre, ruta).
pub const SHORTCUTS: [(&str, &str); 7] = [
    ("Inicio", "/"),
    ("Documentos", "/Documentos"),
    ("Descargas", "/Descargas"),
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
    /// WAV (K12).
    Audio,
    /// AVI (K12).
    Video,
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
            "wav" => Kind::Audio,
            "avi" => Kind::Video,
            _ => Kind::Binary,
        }
    }

    pub fn description(self, entry: &DirEntry) -> &'static str {
        match self {
            Kind::Folder => tr("carpeta"),
            Kind::Trash => tr("papelera"),
            Kind::Text if entry.name.to_ascii_lowercase().ends_with(".md") => "markdown",
            Kind::Text => tr("texto"),
            Kind::Code => tr("código"),
            Kind::Image => tr("imagen"),
            Kind::Archive => tr("archivo comprimido"),
            Kind::Audio => tr("audio"),
            Kind::Video => tr("video"),
            Kind::Binary => tr("archivo"),
        }
    }
}

/// Columna por la que se ordena la lista (clic en el encabezado).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Modified,
}

pub enum Dialog {
    NewFolder(TextInput),
    NewFile(TextInput),
    Rename {
        original: String,
        input: TextInput,
    },
    /// Los nombres (uno o varios) a mover a la Papelera.
    ConfirmTrash(Vec<String>),
    ConfirmDelete(Vec<String>),
    ConfirmEmptyTrash,
}

impl Dialog {
    pub fn title(&self) -> &'static str {
        match self {
            Dialog::NewFolder(_) => tr("NUEVA CARPETA"),
            Dialog::NewFile(_) => tr("NUEVO ARCHIVO DE TEXTO"),
            Dialog::Rename { .. } => tr("RENOMBRAR"),
            Dialog::ConfirmTrash(_) => tr("MOVER A LA PAPELERA"),
            Dialog::ConfirmDelete(_) => tr("BORRAR DEFINITIVAMENTE"),
            Dialog::ConfirmEmptyTrash => tr("VACIAR LA PAPELERA"),
        }
    }

    pub fn accept_label(&self) -> &'static str {
        match self {
            Dialog::NewFolder(_) | Dialog::NewFile(_) => tr("CREAR"),
            Dialog::Rename { .. } => tr("RENOMBRAR"),
            Dialog::ConfirmTrash(_) => tr("MOVER"),
            Dialog::ConfirmDelete(_) | Dialog::ConfirmEmptyTrash => tr("BORRAR"),
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
            Dialog::NewFolder(_) | Dialog::NewFile(_) | Dialog::Rename { .. } => {
                tr("Nombre:").into()
            }
            Dialog::ConfirmTrash(n) if n.len() == 1 => {
                trf("¿Mover \"{}\" a la Papelera?", &[&n[0]])
            }
            Dialog::ConfirmTrash(n) => trf(
                "¿Mover {} elementos a la Papelera?",
                &[&n.len().to_string()],
            ),
            Dialog::ConfirmDelete(n) if n.len() == 1 => {
                trf("\"{}\" se borra para siempre.", &[&n[0]])
            }
            Dialog::ConfirmDelete(n) => trf(
                "{} elementos se borran para siempre.",
                &[&n.len().to_string()],
            ),
            Dialog::ConfirmEmptyTrash => {
                tr("Todo lo que está en la Papelera se borra para siempre.").into()
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
    /// La fila con el cursor.
    pub selected: usize,
    /// Las filas elegidas cuando hay más de una (Shift+flechas, Ctrl+clic, Ctrl+E). Vacío =
    /// solo cuenta `selected`.
    pub marked: BTreeSet<usize>,
    /// Desde dónde se extiende la selección con Shift.
    anchor: Option<usize>,
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
    /// Archivo que se pidió abrir (Enter o doble clic): la ventana decide con qué app.
    pub open_request: Option<String>,
    /// Lo copiado o cortado (Ctrl+C / Ctrl+X): (rutas, es_cortar).
    pub clipboard: Option<(Vec<String>, bool)>,
    pub sort: (Column, bool),
    /// Búsqueda por tipeo: las letras escritas hace poco (como en el Explorador de Windows).
    typed: String,
    typed_ms: u64,
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

pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn lower(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

/// Carpetas primero; después por la columna elegida.
fn sort_entries(entries: &mut [DirEntry], (column, ascending): (Column, bool)) {
    entries.sort_by(|a, b| {
        let by = match column {
            Column::Name => lower(&a.name).cmp(&lower(&b.name)),
            Column::Size => a.size.cmp(&b.size),
            Column::Modified => a.modified.cmp(&b.modified),
        };
        let by = if ascending { by } else { by.reverse() };
        b.is_dir.cmp(&a.is_dir).then(by)
    });
}

/// Los archivos ocultos (atributo de FAT o nombre con "." adelante) no se muestran.
fn visible(e: &DirEntry) -> bool {
    !e.hidden && !e.name.starts_with('.')
}

/// "notas.txt" + 2 → "notas (2).txt"
pub fn with_suffix(name: &str, n: u32) -> String {
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
            marked: BTreeSet::new(),
            anchor: None,
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
            open_request: None,
            clipboard: None,
            sort: (Column::Name, true),
            typed: String::new(),
            typed_ms: 0,
        }
    }

    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.entries.get(self.selected)
    }

    pub fn selected_path(&self) -> Option<String> {
        self.selected_entry().map(|e| join(&self.cwd, &e.name))
    }

    /// Los nombres sobre los que actúa una operación: los marcados o, si no hay, el de la
    /// fila con el cursor.
    pub fn targets(&self) -> Vec<String> {
        if self.marked.is_empty() {
            return self
                .selected_entry()
                .map(|e| e.name.clone())
                .into_iter()
                .collect();
        }
        self.marked
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|e| e.name.clone())
            .collect()
    }

    pub fn is_marked(&self, i: usize) -> bool {
        self.marked.contains(&i)
    }

    pub fn clear_marks(&mut self) {
        if !self.marked.is_empty() {
            self.marked.clear();
            self.dirty = true;
        }
        self.anchor = None;
    }

    /// Ctrl+E (o Ctrl+A): todo.
    pub fn mark_all(&mut self, log: &mut Vec<String>) {
        self.marked = (0..self.entries.len()).collect();
        self.anchor = Some(0);
        self.dirty = true;
        log.push(format!("ARCHIVOS_SELECCION_TODO {}", self.marked.len()));
    }

    /// Un clic en una fila: solo, con Ctrl (suma o saca) o con Shift (un rango).
    pub fn click_row<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        i: usize,
        ctrl: bool,
        shift: bool,
        log: &mut Vec<String>,
    ) {
        if i >= self.entries.len() {
            return;
        }
        if ctrl {
            if self.marked.is_empty() {
                self.marked.insert(self.selected);
            }
            if !self.marked.remove(&i) {
                self.marked.insert(i);
            }
            self.anchor = Some(i);
        } else if shift {
            let a = self.anchor.unwrap_or(self.selected);
            self.anchor = Some(a);
            self.marked = (a.min(i)..=a.max(i)).collect();
        } else {
            self.marked.clear();
            self.anchor = Some(i);
        }
        self.dirty = true;
        self.select(fs, i, log);
        if self.marked.len() > 1 {
            log.push(format!("ARCHIVOS_MARCADOS {}", self.marked.len()));
        }
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
                entries.retain(visible);
                sort_entries(&mut entries, self.sort);
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
        self.marked.clear();
        self.anchor = None;
        let keep = select
            .map(String::from)
            .or_else(|| self.selected_entry().map(|e| e.name.clone()));
        if let Ok(mut entries) = fs.list(&self.cwd) {
            entries.retain(visible);
            sort_entries(&mut entries, self.sort);
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
        } else if let Some(path) = self.selected_path() {
            self.open_request = Some(path);
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
        let names = self.targets();
        if names.is_empty() {
            return;
        }
        if self.cwd == "/" && names.iter().any(|n| n.eq_ignore_ascii_case(tr("Papelera"))) {
            self.notify(
                tr("La Papelera no se puede mover a la Papelera."),
                true,
                now_ms,
                log,
            );
            return;
        }
        self.dialog = Some(if self.in_trash() {
            Dialog::ConfirmDelete(names)
        } else {
            Dialog::ConfirmTrash(names)
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
            Dialog::ConfirmTrash(names) => {
                let mut result = Ok(());
                for name in names {
                    let path = join(&self.cwd, name);
                    match Self::move_to_trash(fs, &path, now) {
                        Ok(dest) => log.push(format!("ARCHIVOS_PAPELERA {path} -> {dest}")),
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                result.map(|()| {
                    let msg = if names.len() == 1 {
                        format!("\"{}\" está en la Papelera.", names[0])
                    } else {
                        trf(
                            "{} elementos están en la Papelera.",
                            &[&names.len().to_string()],
                        )
                    };
                    (msg, None)
                })
            }
            Dialog::ConfirmDelete(names) => {
                let mut result = Ok(());
                for name in names {
                    let path = join(&self.cwd, name);
                    match fs.remove(&path) {
                        Ok(()) => {
                            if self.cwd == TRASH {
                                Self::index_remove(fs, Some(name), now);
                            }
                            log.push(format!("ARCHIVOS_BORRADO {path}"));
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                result.map(|()| {
                    let msg = if names.len() == 1 {
                        format!("\"{}\" se borró.", names[0])
                    } else {
                        trf("{} elementos se borraron.", &[&names.len().to_string()])
                    };
                    (msg, None)
                })
            }
            Dialog::ConfirmEmptyTrash => {
                let result = fs.list(TRASH).and_then(|items| {
                    items
                        .iter()
                        .try_for_each(|e| fs.remove(&join(TRASH, &e.name)))
                });
                result.map(|()| {
                    Self::index_remove(fs, None, now);
                    log.push("ARCHIVOS_PAPELERA_VACIA".into());
                    (tr("La Papelera está vacía.").into(), None)
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
    pub fn move_to_trash<D: BlockDevice>(
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
        // Si no se puede anotar el origen, igual queda en la Papelera (solo no se podrá
        // restaurar a su lugar automáticamente).
        let _ = Self::index_add(fs, &target, path, now);
        Ok(join(TRASH, &target))
    }

    fn read_index<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<(String, String)> {
        let data = fs.read_file(TRASH_INDEX).unwrap_or_default();
        String::from_utf8_lossy(&data)
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(n, p)| (n.to_string(), p.to_string()))
            .collect()
    }

    fn write_index<D: BlockDevice>(
        fs: &mut FileSystem<D>,
        index: &[(String, String)],
        now: Timestamp,
    ) -> Result<(), FsError> {
        let text: String = index.iter().map(|(n, p)| format!("{n}\t{p}\n")).collect();
        fs.write_file(TRASH_INDEX, text.as_bytes(), now)
    }

    fn index_add<D: BlockDevice>(
        fs: &mut FileSystem<D>,
        name: &str,
        origin: &str,
        now: Timestamp,
    ) -> Result<(), FsError> {
        let mut index = Self::read_index(fs);
        index.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        index.push((name.to_string(), origin.to_string()));
        Self::write_index(fs, &index, now)
    }

    fn index_remove<D: BlockDevice>(fs: &mut FileSystem<D>, name: Option<&str>, now: Timestamp) {
        let mut index = Self::read_index(fs);
        match name {
            Some(name) => index.retain(|(n, _)| !n.eq_ignore_ascii_case(name)),
            None => index.clear(),
        }
        let _ = Self::write_index(fs, &index, now);
    }

    /// Restaura lo seleccionado de la Papelera a la carpeta de donde vino (si esa carpeta ya
    /// no existe, se vuelve a crear). Si ahí ya hay algo con ese nombre, se agrega " (2)".
    pub fn restore_selected<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now: Timestamp,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        if self.cwd != TRASH {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let name = entry.name.clone();
        let origin = Self::read_index(fs)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(&name))
            .map(|(_, p)| p)
            .unwrap_or_else(|| join("/", &name));
        let dir = parent(&origin);
        let result = restore_to(fs, &name, &origin, now);
        match result {
            Ok(dest) => {
                Self::index_remove(fs, Some(&name), now);
                log.push(format!("ARCHIVOS_RESTAURADO {dest}"));
                self.reload(fs, None);
                self.notify(format!("\"{name}\" volvió a {dir}."), false, now_ms, log);
            }
            Err(e) => self.notify(error_message(e), true, now_ms, log),
        }
    }

    // --- copiar, cortar y pegar ----------------------------------------------------------------

    pub fn copy_selected(&mut self, cut: bool, now_ms: u64, log: &mut Vec<String>) {
        let names = self.targets();
        if names.is_empty() {
            return;
        }
        let paths: Vec<String> = names.iter().map(|n| join(&self.cwd, n)).collect();
        for path in &paths {
            log.push(format!(
                "ARCHIVOS_{} {path}",
                if cut { "CORTADO" } else { "COPIADO" }
            ));
        }
        let msg = match (names.len(), cut) {
            (1, false) => format!("\"{}\" copiado: Ctrl+V lo pega en otra carpeta.", names[0]),
            (1, true) => format!("\"{}\" cortado: Ctrl+V lo pega en otra carpeta.", names[0]),
            (n, false) => trf(
                "{} elementos copiados: Ctrl+V los pega en otra carpeta.",
                &[&n.to_string()],
            ),
            (n, true) => trf(
                "{} elementos cortados: Ctrl+V los pega en otra carpeta.",
                &[&n.to_string()],
            ),
        };
        self.clipboard = Some((paths, cut));
        self.notify(msg, false, now_ms, log);
    }

    pub fn paste<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now: Timestamp,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        let Some((sources, cut)) = self.clipboard.clone() else {
            self.notify(tr("No hay nada copiado."), false, now_ms, log);
            return;
        };
        let mut pasted: Vec<String> = Vec::new();
        let mut failed = None;
        for src in &sources {
            let name = basename(src).to_string();
            let same_dir = parent(src).eq_ignore_ascii_case(&self.cwd);
            let result = if cut && same_dir {
                Ok(name.clone())
            } else if cut {
                move_unique(fs, src, &self.cwd)
            } else {
                let target = unique_name(fs, &self.cwd, &name);
                fs.copy(src, &join(&self.cwd, &target), now)
                    .map(|()| target)
            };
            match result {
                Ok(target) => {
                    log.push(format!("ARCHIVOS_PEGADO {}", join(&self.cwd, &target)));
                    pasted.push(target);
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if cut {
            self.clipboard = None;
        }
        self.reload(fs, pasted.last().map(String::as_str));
        match failed {
            Some(e) => self.notify(error_message(e), true, now_ms, log),
            None if pasted.len() == 1 => {
                self.notify(format!("\"{}\" pegado.", pasted[0]), false, now_ms, log)
            }
            None => self.notify(
                trf("{} elementos pegados.", &[&pasted.len().to_string()]),
                false,
                now_ms,
                log,
            ),
        }
    }

    pub fn sort_by(&mut self, column: Column) {
        self.sort = if self.sort.0 == column {
            (column, !self.sort.1)
        } else {
            // Como en el Explorador: la fecha arranca por lo más nuevo.
            (column, column != Column::Modified)
        };
        let keep = self.selected_entry().map(|e| e.name.clone());
        sort_entries(&mut self.entries, self.sort);
        if let Some(name) = keep
            && let Some(i) = self.entries.iter().position(|e| e.name == name)
        {
            self.selected = i;
        }
        self.dirty = true;
    }

    /// Rueda del mouse: mueve la lista sin cambiar la selección.
    pub fn scroll_by(&mut self, delta: i32, visible: usize) {
        let max = self.entries.len().saturating_sub(visible.max(1));
        let next = (self.scroll as i64 + delta as i64).clamp(0, max as i64) as usize;
        if next != self.scroll {
            self.scroll = next;
            self.dirty = true;
        }
    }

    /// Búsqueda por tipeo: selecciona el primer elemento cuyo nombre empieza con lo escrito
    /// (si pasa más de 1 segundo entre teclas, empieza una búsqueda nueva).
    fn type_ahead<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        c: char,
        now_ms: u64,
        log: &mut Vec<String>,
    ) {
        if now_ms.saturating_sub(self.typed_ms) > 1000 {
            self.typed.clear();
        }
        self.typed_ms = now_ms;
        self.typed.extend(c.to_lowercase());
        let found = self
            .entries
            .iter()
            .position(|e| lower(&e.name).starts_with(&self.typed));
        if let Some(i) = found {
            self.select(fs, i, log);
        }
    }

    /// Teclas de la app. Devuelve `false` si la tecla no se usó (Esc sin diálogo: la usa el
    /// escritorio para volver a JARVIS).
    pub fn handle_key<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        key: Key,
        mods: Mods,
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
        // Atajos del Explorador de Windows.
        if mods.ctrl {
            match key {
                Key::Char('e' | 'E' | 'a' | 'A') => self.mark_all(log),
                Key::Char('c' | 'C') => self.copy_selected(false, now_ms, log),
                Key::Char('x' | 'X') => self.copy_selected(true, now_ms, log),
                Key::Char('v' | 'V') => self.paste(fs, now, now_ms, log),
                Key::Char('n' | 'N') if mods.shift => self.start_new_folder(),
                _ => return false,
            }
            return true;
        }
        if mods.alt {
            match key {
                Key::Up => self.go_up(fs, now_ms, log),
                Key::Left => self.go_back(fs, now_ms, log),
                _ => return false,
            }
            return true;
        }
        match key {
            Key::F(5) => {
                self.reload(fs, None);
                return true;
            }
            Key::Char(c) if !c.is_control() && c != ' ' => {
                self.type_ahead(fs, c, now_ms, log);
                return true;
            }
            _ => {}
        }
        let moves = matches!(
            key,
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End
        );
        if moves && mods.shift {
            // Shift + mover: la selección va desde el ancla hasta el cursor.
            let a = self.anchor.unwrap_or(self.selected);
            self.anchor = Some(a);
            let delta = match key {
                Key::Up => -1,
                Key::Down => 1,
                Key::PageUp => -10,
                Key::PageDown => 10,
                Key::Home => isize::MIN / 2,
                _ => isize::MAX / 2,
            };
            self.move_selection(fs, delta, log);
            self.marked = (a.min(self.selected)..=a.max(self.selected)).collect();
            self.dirty = true;
            return true;
        }
        if moves {
            self.clear_marks();
        }
        if key == Key::Escape && !self.marked.is_empty() {
            self.clear_marks();
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
            Key::F(7) => self.start_new_folder(),
            Key::F(6) => self.start_new_file(),
            Key::F(2) => self.start_rename(),
            Key::Delete => self.start_delete(now_ms, log),
            _ => return false,
        }
        true
    }
}

/// Crea la carpeta `dir` y las que falten en el camino.
fn ensure_dir<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    dir: &str,
    now: Timestamp,
) -> Result<(), FsError> {
    if dir == "/" || fs.exists(dir) {
        return Ok(());
    }
    ensure_dir(fs, &parent(dir), now)?;
    fs.mkdir(dir, now)
}

/// `name`, o "name (2)", "name (3)"… el primero que no exista en `dir`.
pub(crate) fn unique_name<D: BlockDevice>(fs: &mut FileSystem<D>, dir: &str, name: &str) -> String {
    let mut target = name.to_string();
    let mut n = 2;
    while fs.exists(&join(dir, &target)) {
        target = with_suffix(name, n);
        n += 1;
    }
    target
}

/// Mueve `src` a la carpeta `dir`; si ahí ya hay algo con ese nombre, le agrega " (2)".
/// Devuelve el nombre final.
fn move_unique<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    src: &str,
    dir: &str,
) -> Result<String, FsError> {
    if !fs.exists(src) {
        return Err(FsError::NotFound);
    }
    let name = basename(src).to_string();
    let target = unique_name(fs, dir, &name);
    if target != name {
        // Se renombra en el origen para que no choque en el destino.
        fs.rename(src, &target)?;
    }
    let moving = join(&parent(src), &target);
    if let Err(e) = fs.move_to(&moving, dir) {
        if target != name {
            let _ = fs.rename(&moving, &name);
        }
        return Err(e);
    }
    Ok(target)
}

/// Saca `name` de la Papelera y lo devuelve a `origin` (su ruta original).
fn restore_to<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    name: &str,
    origin: &str,
    now: Timestamp,
) -> Result<String, FsError> {
    let dir = parent(origin);
    ensure_dir(fs, &dir, now)?;
    let here = join(TRASH, name);
    let wanted = basename(origin);
    if wanted != name && !fs.exists(&join(TRASH, wanted)) {
        // Vuelve a su nombre original (en la Papelera pudo tener " (2)").
        fs.rename(&here, wanted)?;
        let target = move_unique(fs, &join(TRASH, wanted), &dir)?;
        return Ok(join(&dir, &target));
    }
    let target = move_unique(fs, &here, &dir)?;
    Ok(join(&dir, &target))
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
