//! `winget`: el gestor de paquetes de Windows, contra su repositorio oficial
//! (`github.com/microsoft/winget-pkgs`, el mismo que usa Windows). ADR 0021.
//!
//! Cada programa tiene un Id (`7zip.7zip`, `BurntSushi.ripgrep.MSVC`) y, por versión,
//! manifiestos en YAML: `manifests/<letra>/<Editor>/<Programa>/<versión>/<Id>.installer.yaml`
//! (dónde está el instalador, su SHA-256 y de qué tipo es) y `<Id>.locale.en-US.yaml` (nombre y
//! descripción).
//!
//! - `winget search` busca en una lista de programas conocidos
//!   (`http://paquetes.jarvis/winget.txt`): el índice completo de winget es una base SQLite que
//!   no vale la pena bajar. Con el Id exacto, `winget show` e `install` van al repositorio real.
//! - `winget install` baja el instalador de 64 bits y **verifica su SHA-256** contra el del
//!   manifiesto (como winget en Windows: si no coincide, no instala). Según `InstallerType`:
//!   - `portable` (un `.exe` suelto, como jq) y `zip` con `NestedInstallerType: portable` (un
//!     ZIP con el programa adentro, como ripgrep): se **instalan** en
//!     `/Programas/windows/<Id>/` y queda un comando en `/Programas/bin` (`PortableCommandAlias`,
//!     o `Commands`, o el nombre del `.exe`) que lo corre con `vino`.
//!   - Los instaladores (`exe`, `msi`, `nullsoft`, `inno`, `wix`, `burn`): son programas con
//!     ventanas y casi siempre de 32 bits, que `vino` todavía no corre: se bajan a /Descargas.
//! - `winget install --manifest <ruta o URL>` instala desde un manifiesto propio (como en
//!   Windows), sin pasar por GitHub.
//! - `list`, `uninstall` (a la Papelera, regla 17) y `upgrade`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;

use super::apt::{ensure_dirs, newer};
use super::{Out, ansi, zip};
use crate::apps::Ctx;
use crate::files::{FilesApp, format_size};
use crate::system::{FetchKind, HttpResponse};
use crate::web::json::{self, Json};

pub const LIST: &str = "http://paquetes.jarvis/winget.txt";
const API: &str = "https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests";
const RAW: &str = "https://raw.githubusercontent.com/microsoft/winget-pkgs/master/manifests";
const DOWNLOADS: &str = "/Descargas";
/// Los instaladores bajados (los que no se pueden instalar).
const DB: &str = "/Sistema/winget/descargados.txt";
/// Los programas instalados: `Id|versión|tipo|carpeta|comando1,comando2`.
pub const INSTALLED: &str = "/Sistema/winget/instalados.txt";
/// Dónde quedan los programas de Windows instalados.
pub const PROGRAMS: &str = "/Programas/windows";

/// Un programa de la lista conocida: (Id, nombre, descripción).
fn parse_list(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            (f.len() >= 3).then(|| (f[0].into(), f[1].into(), f[2].into()))
        })
        .collect()
}

/// `7zip.7zip` → `7/7zip/7zip` (la carpeta del repositorio).
fn id_path(id: &str) -> String {
    let first = id
        .chars()
        .next()
        .map(|c| c.to_ascii_lowercase())
        .unwrap_or('_');
    format!("{first}/{}", id.replace('.', "/"))
}

/// Datos de un YAML plano: el valor de `clave:` (el primero que aparece).
fn yaml_field(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim_start().trim_start_matches("- ");
        if let Some(v) = t.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let v = v.trim().trim_matches(['"', '\'']);
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Un archivo de adentro del ZIP que es el programa, y el comando con el que se lo llama.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nested {
    pub path: String,
    pub alias: Option<String>,
}

/// Un instalador del manifiesto (lo de arriba de `Installers:` vale para todos).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installer {
    pub arch: String,
    pub kind: String,
    pub nested_kind: String,
    pub url: String,
    pub sha256: String,
    pub nested: Vec<Nested>,
    pub commands: Vec<String>,
}

fn clean(v: &str) -> String {
    v.trim().trim_matches(['"', '\'']).to_string()
}

/// Los instaladores de un manifiesto `*.installer.yaml`. Es YAML de verdad (listas y mapas
/// anidados), pero con una forma fija: alcanza con seguir la sangría.
pub fn installers(text: &str) -> Vec<Installer> {
    #[derive(PartialEq)]
    enum List {
        None,
        Nested,
        Commands,
    }
    let mut top = Installer::default();
    let mut out: Vec<Installer> = Vec::new();
    let mut in_installers = false;
    let mut list = List::None;
    let mut list_indent = 0usize;
    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = line.len() - t.len();
        let item = t.strip_prefix("- ");
        // Un elemento de la lista de instaladores (`- Architecture: x64`) empieza uno nuevo.
        if in_installers && indent == 0 && item.is_some() {
            let mut inst = top.clone();
            inst.url.clear();
            inst.sha256.clear();
            inst.arch.clear();
            out.push(inst);
            list = List::None;
        } else if indent == 0 && item.is_none() {
            in_installers = t.starts_with("Installers:");
            list = List::None;
        }
        // Una línea es de la lista si es un elemento (`- …`) con al menos su sangría, o la
        // continuación de un elemento (más sangría). Si no, la lista terminó.
        let in_list = list != List::None
            && !(in_installers && indent == 0)
            && ((item.is_some() && indent >= list_indent) || indent > list_indent);
        if !in_list {
            list = List::None;
        }
        let body = if in_installers && indent == 0 {
            item.unwrap_or(t)
        } else {
            t
        };
        let target: &mut Installer = if in_installers {
            match out.last_mut() {
                Some(i) => i,
                None => continue,
            }
        } else {
            &mut top
        };
        // Dentro de una lista (archivos del ZIP o comandos).
        if in_list {
            match list {
                List::Commands => {
                    if let Some(c) = item {
                        target.commands.push(clean(c));
                    }
                }
                List::Nested => {
                    let (new, kv) = match item {
                        Some(rest) => (true, rest),
                        None => (false, t),
                    };
                    if new {
                        target.nested.push(Nested::default());
                    }
                    if let Some((k, v)) = kv.split_once(':')
                        && let Some(n) = target.nested.last_mut()
                    {
                        match k.trim() {
                            "RelativeFilePath" => n.path = clean(v).replace('\\', "/"),
                            "PortableCommandAlias" => n.alias = Some(clean(v)),
                            _ => {}
                        }
                    }
                }
                List::None => {}
            }
            continue;
        }
        let Some((key, value)) = body.split_once(':') else {
            continue;
        };
        let value = clean(value);
        match key.trim() {
            "Architecture" => target.arch = value,
            "InstallerType" => target.kind = value.to_lowercase(),
            "NestedInstallerType" => target.nested_kind = value.to_lowercase(),
            "InstallerUrl" => target.url = value,
            "InstallerSha256" => target.sha256 = value.to_lowercase(),
            "NestedInstallerFiles" => {
                target.nested.clear();
                list = List::Nested;
                list_indent = indent;
            }
            "Commands" => {
                target.commands.clear();
                list = List::Commands;
                list_indent = indent;
            }
            _ => {}
        }
    }
    out
}

/// El instalador para una PC de 64 bits (o el que haya).
pub fn best_installer(list: &[Installer]) -> Option<&Installer> {
    list.iter()
        .find(|i| i.arch == "x64")
        .or_else(|| list.iter().find(|i| i.arch == "neutral"))
        .or_else(|| list.iter().find(|i| i.arch == "x86"))
        .or_else(|| list.first())
}

/// ¿`vino` puede correrlo instalado? (Un `.exe` portable, o un ZIP con uno adentro.)
pub fn installable(i: &Installer) -> bool {
    i.kind == "portable" || (i.kind == "zip" && i.nested_kind == "portable")
}

/// Un nombre de comando seguro (letras, números, `-`, `_`, `.`).
fn safe_command(s: &str) -> Option<String> {
    let c = s.trim().to_lowercase();
    let c = c.strip_suffix(".exe").unwrap_or(&c).to_string();
    (!c.is_empty()
        && !c.starts_with('.')
        && c.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    .then_some(c)
}

/// Un Id como nombre de carpeta (`BurntSushi.ripgrep.MSVC`).
fn safe_id(id: &str) -> Option<String> {
    (!id.is_empty()
        && id.len() <= 100
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')))
    .then(|| id.to_string())
}

fn file_name_of(url: &str) -> String {
    crate::web::url::percent_decode(
        url.split('?')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("programa.exe"),
    )
}

/// El texto del comando de `/Programas/bin` que corre el `.exe` con vino.
pub fn launcher(exe: &str) -> String {
    format!(
        "# Programa de Windows instalado con winget: lo corre vino (ADR 0018).\nwine {exe} $@\n"
    )
}

/// Si `text` es un lanzador (comentarios y una sola línea `wine <exe> $@` o `vino <exe> $@`),
/// el `.exe`. La terminal lo corre directo con vino, sin pasar por un script (los scripts no
/// pueden lanzar procesos).
pub fn launcher_target(text: &str) -> Option<String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let mut words = line.split_whitespace();
    if !matches!(words.next()?, "wine" | "vino") {
        return None;
    }
    let exe = words.next()?;
    let rest: Vec<&str> = words.collect();
    (exe.starts_with('/') && (rest.is_empty() || rest == ["$@"]) && !exe.contains("$"))
        .then(|| exe.to_string())
}

enum Step {
    Search(String),
    /// Resolviendo el Id en la lista (para corregir mayúsculas).
    List,
    Versions,
    Locale,
    Installer,
    File,
}

pub struct WingetJob {
    step: Step,
    waiting: Option<u32>,
    id: String,
    install: bool,
    version: String,
    info: String,
    chosen: Option<Installer>,
    /// `upgrade`: solo se instala si la versión del repositorio es más nueva.
    upgrade_from: Option<String>,
}

impl WingetJob {
    pub fn waiting_id(&self) -> Option<u32> {
        self.waiting
    }

    fn get<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>, url: &str, kind: FetchKind) {
        self.waiting = Some(ctx.out.fetch_as(url, kind, "winget"));
    }

    pub(crate) fn on_response<D: BlockDevice>(
        &mut self,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        self.waiting = None;
        let body = match result {
            Ok(r) if r.status == 200 => Some(r),
            _ => None,
        };
        let why = || match result {
            Ok(r) => format!("HTTP {}", r.status),
            Err(e) => e.clone(),
        };
        match &self.step {
            Step::Search(q) => {
                let q = q.to_lowercase();
                let Some(r) = body else {
                    out.err(&format!(
                        "No se pudo leer la lista de programas ({}).",
                        why()
                    ));
                    return Some((1, String::new()));
                };
                let list = parse_list(&String::from_utf8_lossy(&r.body));
                let hits: Vec<&(String, String, String)> = list
                    .iter()
                    .filter(|(id, n, d)| {
                        q.is_empty()
                            || id.to_lowercase().contains(&q)
                            || n.to_lowercase().contains(&q)
                            || d.to_lowercase().contains(&q)
                    })
                    .collect();
                if hits.is_empty() {
                    out.info(&format!(
                        "No se encontró ningún programa para \"{q}\" en la lista conocida.\n\
                         Si sabés el Id exacto (lo muestra winget.run o la web del programa): winget show <Id>"
                    ));
                    return Some((1, String::new()));
                }
                let mut s = format!(
                    "{:<28}{:<30}{}\n{}\n",
                    "Nombre",
                    "Id",
                    "Origen",
                    "-".repeat(66)
                );
                for (id, n, _) in hits {
                    s.push_str(&format!("{:<28}{:<30}winget\n", n, id));
                }
                out.info(s.trim_end());
                Some((0, String::new()))
            }
            Step::List => {
                // Si el Id está en la lista conocida (sin importar mayúsculas), se usa como ahí.
                if let Some(r) = body {
                    let list = parse_list(&String::from_utf8_lossy(&r.body));
                    if let Some((id, _, _)) = list
                        .iter()
                        .find(|(id, _, _)| id.eq_ignore_ascii_case(&self.id))
                    {
                        self.id = id.clone();
                    }
                }
                out.info(&format!(
                    "Buscando {} en el repositorio de winget...",
                    self.id
                ));
                self.step = Step::Versions;
                let url = format!("{API}/{}", id_path(&self.id));
                self.get(ctx, &url, FetchKind::Page);
                None
            }
            Step::Versions => {
                let Some(r) = body else {
                    out.err(&format!(
                        "No se encontró el paquete {} ({}). Los Id distinguen mayúsculas: winget search {}",
                        self.id,
                        why(),
                        self.id.split('.').next_back().unwrap_or("")
                    ));
                    return Some((1, String::new()));
                };
                let v = json::parse(&String::from_utf8_lossy(&r.body));
                let mut best: Option<String> = None;
                for item in v.as_ref().map(Json::arr).unwrap_or(&[]) {
                    let (Some(name), Some("dir")) = (
                        item.get("name").and_then(Json::str),
                        item.get("type").and_then(Json::str),
                    ) else {
                        continue;
                    };
                    if !name.starts_with(|c: char| c.is_ascii_digit()) {
                        continue; // subpaquetes (otra carpeta), no versiones
                    }
                    if best.as_deref().is_none_or(|b| newer(name, b)) {
                        best = Some(name.to_string());
                    }
                }
                let Some(version) = best else {
                    out.err(&format!(
                        "El paquete {} no tiene versiones publicadas.",
                        self.id
                    ));
                    return Some((1, String::new()));
                };
                if let Some(old) = &self.upgrade_from
                    && !newer(&version, old)
                {
                    out.info(&format!(
                        "{} ya está en su versión más reciente ({old}).",
                        self.id
                    ));
                    return Some((0, String::new()));
                }
                self.version = version;
                self.step = Step::Locale;
                let url = format!(
                    "{RAW}/{}/{}/{}.locale.en-US.yaml",
                    id_path(&self.id),
                    self.version,
                    self.id
                );
                self.get(ctx, &url, FetchKind::Page);
                None
            }
            Step::Locale => {
                let text = body
                    .map(|r| String::from_utf8_lossy(&r.body).into_owned())
                    .unwrap_or_default();
                let f = |k: &str| yaml_field(&text, k).unwrap_or_else(|| "-".into());
                self.info = format!(
                    "Encontrado {}{}{} [{}]\nVersión: {}\nEditor: {}\nDescripción: {}\nLicencia: {}\nPágina: {}\n",
                    ansi::BOLD,
                    f("PackageName"),
                    ansi::RESET,
                    self.id,
                    self.version,
                    f("Publisher"),
                    f("ShortDescription"),
                    f("License"),
                    f("PackageUrl"),
                );
                self.step = Step::Installer;
                let url = format!(
                    "{RAW}/{}/{}/{}.installer.yaml",
                    id_path(&self.id),
                    self.version,
                    self.id
                );
                self.get(ctx, &url, FetchKind::Page);
                None
            }
            Step::Installer => {
                let Some(r) = body else {
                    out.err(&format!(
                        "No se pudo leer el manifiesto del instalador ({}).",
                        why()
                    ));
                    return Some((1, String::new()));
                };
                let text = String::from_utf8_lossy(&r.body).into_owned();
                self.read_manifest(&text, ctx, out)
            }
            Step::File => {
                let Some(r) = body else {
                    out.err(&format!(
                        "No se pudo bajar el instalador ({}). JARVIS-OS baja hasta 32 MB por vez.",
                        why()
                    ));
                    return Some((1, String::new()));
                };
                let Some(inst) = self.chosen.clone() else {
                    return Some((1, String::new()));
                };
                // Como winget: si el SHA-256 no es el del manifiesto, no se instala nada.
                if !inst.sha256.is_empty() {
                    let got = crate::antivirus::hex(&crate::antivirus::sha256(&r.body));
                    if got != inst.sha256 {
                        out.err(&format!(
                            "El instalador no coincide con el SHA-256 del manifiesto (se esperaba {}, llegó {got}). No se instaló nada.",
                            inst.sha256
                        ));
                        ctx.log.push(format!("WINGET_HASH_MAL {}", self.id));
                        return Some((1, String::new()));
                    }
                    out.info("SHA-256 verificado.");
                }
                if installable(&inst) {
                    Some(self.install_portable(&inst, &r.body, ctx, out))
                } else {
                    Some(self.save_download(&inst, &r.body, ctx, out))
                }
            }
        }
    }

    /// Leyó el manifiesto del instalador: elige uno y lo baja (o solo lo muestra).
    fn read_manifest<D: BlockDevice>(
        &mut self,
        text: &str,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        if self.version.is_empty() {
            self.version = yaml_field(text, "PackageVersion").unwrap_or_else(|| "?".into());
        }
        if let Some(id) = yaml_field(text, "PackageIdentifier")
            && self.id.is_empty()
        {
            self.id = id;
        }
        let list = installers(text);
        let Some(inst) = best_installer(&list).cloned() else {
            out.err("El manifiesto no tiene instaladores.");
            return Some((1, String::new()));
        };
        let mut info = core::mem::take(&mut self.info);
        if info.is_empty() {
            info = format!("Paquete {} [{}]\n", self.id, self.version);
        }
        let kind = if inst.nested_kind.is_empty() {
            inst.kind.clone()
        } else {
            format!("{} ({})", inst.kind, inst.nested_kind)
        };
        info.push_str(&format!(
            "Instalador: {kind} ({})\n  {}",
            inst.arch, inst.url
        ));
        out.info(&info);
        if !self.install {
            return Some((0, String::new()));
        }
        if inst.url.is_empty() {
            out.err("El manifiesto no dice de dónde bajar el instalador.");
            return Some((1, String::new()));
        }
        out.info(&format!("Descargando {}...", file_name_of(&inst.url)));
        let url = inst.url.clone();
        self.chosen = Some(inst);
        self.step = Step::File;
        self.get(ctx, &url, FetchKind::Download);
        None
    }

    /// `portable` o `zip` con un portable adentro: se instala y queda el comando.
    fn install_portable<D: BlockDevice>(
        &mut self,
        inst: &Installer,
        data: &[u8],
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> (i32, String) {
        let Some(id) = safe_id(&self.id) else {
            out.err(&format!("Id de paquete inválido: {}", self.id));
            return (1, String::new());
        };
        let dir = format!("{PROGRAMS}/{id}");
        // Los archivos a instalar: (ruta adentro de la carpeta, contenido).
        let files: Vec<(String, Vec<u8>)> = if inst.kind == "zip" {
            match zip::extract_all(data) {
                Ok(list) => list.into_iter().map(|(e, d)| (e.name, d)).collect(),
                Err(e) => {
                    out.err(&format!("No se pudo abrir el ZIP: {e}"));
                    return (1, String::new());
                }
            }
        } else {
            let name = file_name_of(&inst.url);
            if !zip::safe_name(&name) {
                out.err(&format!("Nombre de archivo inválido: {name}"));
                return (1, String::new());
            }
            alloc::vec![(name, data.to_vec())]
        };
        // Los comandos: (nombre, el .exe adentro de la carpeta).
        let mut commands: Vec<(String, String)> = Vec::new();
        if inst.kind == "zip" {
            for n in &inst.nested {
                let alias = n
                    .alias
                    .as_deref()
                    .and_then(safe_command)
                    .or_else(|| safe_command(n.path.rsplit('/').next().unwrap_or("")));
                if let Some(a) = alias
                    && files.iter().any(|(p, _)| p.eq_ignore_ascii_case(&n.path))
                {
                    commands.push((a, n.path.clone()));
                }
            }
        } else if let Some((name, _)) = files.first() {
            let alias = inst
                .commands
                .first()
                .and_then(|c| safe_command(c))
                .or_else(|| safe_command(name));
            if let Some(a) = alias {
                commands.push((a, name.clone()));
            }
        }
        if commands.is_empty() {
            out.err(
                "El paquete no dice cuál es el programa (NestedInstallerFiles): no se instaló.",
            );
            return (1, String::new());
        }
        let now = ctx.timestamp();
        let Some(fs) = ctx.fs.as_deref_mut() else {
            out.err("no hay disco");
            return (1, String::new());
        };
        // Una versión anterior va a la Papelera (regla 17).
        if fs.exists(&dir) {
            let _ = FilesApp::move_to_trash(fs, &dir, now);
        }
        let mut total = 0usize;
        for (rel, bytes) in &files {
            let path = format!("{dir}/{rel}");
            let parent = crate::files::parent(&path);
            if let Err(e) =
                ensure_dirs(fs, &parent, now).and_then(|()| fs.write_file(&path, bytes, now))
            {
                out.err(&format!("No se pudo guardar {path}: {e}"));
                return (1, String::new());
            }
            total += bytes.len();
        }
        // El antivirus revisa cada archivo instalado.
        for (rel, bytes) in &files {
            let path = format!("{dir}/{rel}");
            if let Some(msg) = crate::antivirus::guard(ctx, &path, bytes) {
                out.err(&msg);
                return (1, String::new());
            }
        }
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return (1, String::new());
        };
        let _ = ensure_dirs(fs, super::BIN, now);
        let mut names = Vec::new();
        for (alias, rel) in &commands {
            let bin = format!("{}/{alias}", super::BIN);
            if fs.exists(&bin) {
                let _ = FilesApp::move_to_trash(fs, &bin, now);
            }
            if let Err(e) = fs.write_file(&bin, launcher(&format!("{dir}/{rel}")).as_bytes(), now) {
                out.err(&format!("No se pudo crear el comando {alias}: {e}"));
                return (1, String::new());
            }
            names.push(alias.clone());
        }
        let mut db = read_installed(fs);
        db.retain(|r| !r.id.eq_ignore_ascii_case(&id));
        db.push(InstalledRow {
            id: id.clone(),
            version: self.version.clone(),
            kind: inst.kind.clone(),
            dir: dir.clone(),
            commands: names.clone(),
        });
        let _ = write_installed(fs, &db, now);
        ctx.log
            .push(format!("WINGET_INSTALADO {id} {}", names.join(",")));
        let has_vino = fs.exists("/Programas/bin/vino");
        out.info(&format!(
            "Instalado {id} {} ({}, {} archivo(s)) en {dir}.\nComando: {}{}{} (corre con vino){}",
            self.version,
            format_size(total as u64),
            files.len(),
            ansi::BOLD,
            names.join(", "),
            ansi::RESET,
            if has_vino {
                format!(
                    "\n{}Si le falta algo de Windows, vino lo dice: vino --imports {dir}/{}{}",
                    ansi::DIM,
                    commands[0].1,
                    ansi::RESET
                )
            } else {
                "\nPara correrlo hace falta vino: sudo apt install vino".to_string()
            }
        ));
        (0, String::new())
    }

    /// Un instalador que `vino` no puede correr: se baja a /Descargas.
    fn save_download<D: BlockDevice>(
        &mut self,
        inst: &Installer,
        data: &[u8],
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> (i32, String) {
        let now = ctx.timestamp();
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return (1, String::new());
        };
        let mut name = file_name_of(&inst.url);
        if !name.contains('.') {
            name.push_str(".exe");
        }
        if !zip::safe_name(&name) {
            out.err(&format!("Nombre de archivo inválido: {name}"));
            return (1, String::new());
        }
        let path = format!("{DOWNLOADS}/{name}");
        let saved = ensure_dirs(fs, DOWNLOADS, now).and_then(|()| fs.write_file(&path, data, now));
        if let Err(e) = saved {
            out.err(&format!("No se pudo guardar {path}: {e}"));
            return (1, String::new());
        }
        let mut db = fs
            .read_file(DB)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        db.push_str(&format!("{}|{}|{path}\n", self.id, self.version));
        let _ = ensure_dirs(fs, "/Sistema/winget", now)
            .and_then(|()| fs.write_file(DB, db.as_bytes(), now));
        if let Some(msg) = crate::antivirus::guard(ctx, &path, data) {
            out.err(&msg);
            return (1, String::new());
        }
        let what = super::binfmt::describe(&data[..data.len().min(256 * 1024)]);
        ctx.log
            .push(format!("WINGET_DESCARGADO {} {path}", self.id));
        out.info(&format!(
            "Descargado {path} ({}).\n{what}\n{}Es un instalador ({}): un programa con ventanas que vino todavía no corre. Los paquetes \"portable\" y \"zip\" sí se instalan.{}",
            format_size(data.len() as u64),
            ansi::DIM,
            inst.kind,
            ansi::RESET
        ));
        (0, String::new())
    }
}

/// Un programa instalado (una línea de [`INSTALLED`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledRow {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub dir: String,
    pub commands: Vec<String>,
}

pub fn read_installed<D: BlockDevice>(fs: &mut jarvis_fs::FileSystem<D>) -> Vec<InstalledRow> {
    let text = fs
        .read_file(INSTALLED)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            (f.len() >= 5).then(|| InstalledRow {
                id: f[0].into(),
                version: f[1].into(),
                kind: f[2].into(),
                dir: f[3].into(),
                commands: f[4]
                    .split(',')
                    .filter(|c| !c.is_empty())
                    .map(String::from)
                    .collect(),
            })
        })
        .collect()
}

fn write_installed<D: BlockDevice>(
    fs: &mut jarvis_fs::FileSystem<D>,
    rows: &[InstalledRow],
    now: jarvis_fs::Timestamp,
) -> Result<(), jarvis_fs::FsError> {
    let mut s = String::new();
    for r in rows {
        s.push_str(&format!(
            "{}|{}|{}|{}|{}\n",
            r.id,
            r.version,
            r.kind,
            r.dir,
            r.commands.join(",")
        ));
    }
    ensure_dirs(fs, "/Sistema/winget", now)?;
    fs.write_file(INSTALLED, s.as_bytes(), now)
}

const HELP: &str = "Administrador de paquetes de Windows (winget) v1.9 · JARVIS-OS
Uso: winget <comando> [opciones]

  search TEXTO              busca en la lista de programas conocidos
  show ID                   muestra un paquete del repositorio oficial
  install ID                instala (portable y zip; los instaladores se bajan a /Descargas)
  install --manifest RUTA   instala desde un manifiesto propio (ruta o http://)
  list                      lo instalado y lo bajado
  upgrade ID                actualiza si hay una versión más nueva
  uninstall ID              desinstala (a la Papelera)

Ejemplos:  winget search ripgrep  ·  sudo winget install BurntSushi.ripgrep.MSVC";

pub(crate) fn run<D: BlockDevice>(
    args: &[String],
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
) -> Result<i32, Box<WingetJob>> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    // `--id X`, `-e`, `--exact`: se aceptan (como en Windows).
    let mut rest: Vec<String> = Vec::new();
    let mut manifest: Option<String> = None;
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--id" | "-q" | "--query" => {
                if let Some(v) = it.next() {
                    rest.push(v.clone());
                }
            }
            "--manifest" | "-m" => manifest = it.next().cloned(),
            x if x.starts_with('-') => {}
            _ => rest.push(a.clone()),
        }
    }
    let new = |step: Step, id: String, install: bool| WingetJob {
        step,
        waiting: None,
        id,
        install,
        version: String::new(),
        info: String::new(),
        chosen: None,
        upgrade_from: None,
    };
    match cmd {
        "" | "--help" | "-?" | "help" => {
            o.push_str(HELP);
            o.push('\n');
            Ok(0)
        }
        "--version" | "-v" => {
            o.push_str("v1.9.25200 (JARVIS-OS)\n");
            Ok(0)
        }
        "search" | "find" => {
            let mut j = new(Step::Search(rest.join(" ")), String::new(), false);
            j.get(ctx, LIST, FetchKind::Page);
            Err(Box::new(j))
        }
        "show" | "view" | "install" | "add" if manifest.is_some() => {
            let m = manifest.unwrap_or_default();
            let mut j = new(
                Step::Installer,
                String::new(),
                matches!(cmd, "install" | "add"),
            );
            if m.starts_with("http://") || m.starts_with("https://") {
                j.get(ctx, &m, FetchKind::Page);
                return Err(Box::new(j));
            }
            // Un manifiesto del disco.
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let text = match fs.read_file(&m) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(err) => {
                    e.push_str(&format!("winget: {m}: {err}\n"));
                    return Ok(1);
                }
            };
            let mut out = Out::new();
            let r = j.read_manifest(&text, ctx, &mut out);
            o.push_str(&out.term);
            match r {
                Some((code, _)) => Ok(code),
                None => Err(Box::new(j)),
            }
        }
        "show" | "view" | "install" | "add" => {
            let Some(id) = rest.first() else {
                e.push_str(&format!(
                    "Falta el Id del paquete: winget {cmd} BurntSushi.ripgrep.MSVC\n"
                ));
                return Ok(1);
            };
            let mut j = new(Step::List, id.clone(), matches!(cmd, "install" | "add"));
            j.get(ctx, LIST, FetchKind::Page);
            Err(Box::new(j))
        }
        "list" | "ls" => {
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let installed = read_installed(fs);
            let db = fs
                .read_file(DB)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            if installed.is_empty() && db.trim().is_empty() {
                o.push_str(
                    "Todavía no hay nada. Probá: sudo winget install BurntSushi.ripgrep.MSVC\n",
                );
                return Ok(0);
            }
            if !installed.is_empty() {
                o.push_str(&format!(
                    "{:<30}{:<12}{:<10}{}\n{}\n",
                    "Id",
                    "Versión",
                    "Tipo",
                    "Comandos",
                    "-".repeat(70)
                ));
                for r in &installed {
                    o.push_str(&format!(
                        "{:<30}{:<12}{:<10}{}\n",
                        r.id,
                        r.version,
                        r.kind,
                        r.commands.join(", ")
                    ));
                }
            }
            if !db.trim().is_empty() {
                o.push_str("\nInstaladores bajados (no se ejecutan todavía):\n");
                for l in db.lines() {
                    let f: Vec<&str> = l.split('|').collect();
                    if f.len() >= 3 {
                        o.push_str(&format!("  {:<28}{:<12}{}\n", f[0], f[1], f[2]));
                    }
                }
            }
            Ok(0)
        }
        "uninstall" | "remove" | "rm" => {
            let Some(id) = rest.first() else {
                e.push_str("Falta el Id: winget uninstall BurntSushi.ripgrep.MSVC\n");
                return Ok(1);
            };
            let now = ctx.timestamp();
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let mut db = read_installed(fs);
            let Some(pos) = db.iter().position(|r| r.id.eq_ignore_ascii_case(id)) else {
                e.push_str(&format!(
                    "No se encontró ningún paquete instalado que coincida con {id}.\n"
                ));
                return Ok(1);
            };
            let row = db.remove(pos);
            let _ = FilesApp::move_to_trash(fs, &row.dir, now);
            for c in &row.commands {
                let _ = FilesApp::move_to_trash(fs, &format!("{}/{c}", super::BIN), now);
            }
            let _ = write_installed(fs, &db, now);
            ctx.log.push(format!("WINGET_DESINSTALADO {}", row.id));
            o.push_str(&format!(
                "Desinstalado {} {}: los archivos están en la Papelera.\n",
                row.id, row.version
            ));
            Ok(0)
        }
        "upgrade" | "update" => {
            let Some(id) = rest.first() else {
                e.push_str("Falta el Id: winget upgrade BurntSushi.ripgrep.MSVC\n");
                return Ok(1);
            };
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let Some(row) = read_installed(fs)
                .into_iter()
                .find(|r| r.id.eq_ignore_ascii_case(id))
            else {
                e.push_str(&format!("{id} no está instalado (winget install {id}).\n"));
                return Ok(1);
            };
            let mut j = new(Step::List, row.id.clone(), true);
            j.upgrade_from = Some(row.version);
            j.get(ctx, LIST, FetchKind::Page);
            Err(Box::new(j))
        }
        other => {
            e.push_str(&format!(
                "Comando desconocido: {other} (ver: winget --help)\n"
            ));
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifiestos_de_winget() {
        assert_eq!(id_path("7zip.7zip"), "7/7zip/7zip");
        assert_eq!(id_path("Notepad++.Notepad++"), "n/Notepad++/Notepad++");
        let yaml = "PackageIdentifier: 7zip.7zip\nPackageVersion: 24.08\nInstallerType: exe\n\
                    Installers:\n- Architecture: x86\n  InstallerUrl: https://7-zip.org/a/7z2408.exe\n\
                    - Architecture: x64\n  InstallerUrl: https://7-zip.org/a/7z2408-x64.exe\n\
                    - Architecture: arm64\n  InstallerType: msi\n  InstallerUrl: https://7-zip.org/a/7z-arm64.msi\n";
        let list = installers(yaml);
        assert_eq!(list.len(), 3);
        let best = best_installer(&list).unwrap();
        assert_eq!(best.arch, "x64");
        assert_eq!(best.kind, "exe");
        assert_eq!(best.url, "https://7-zip.org/a/7z2408-x64.exe");
        assert!(!installable(best));
        assert_eq!(list[2].kind, "msi");
        assert_eq!(
            yaml_field(
                "PackageName: \"7-Zip\"\nShortDescription: Archivador",
                "PackageName"
            ),
            Some("7-Zip".into())
        );
    }

    /// Los manifiestos reales de ripgrep y jq (winget-pkgs, noviembre de 2026).
    #[test]
    fn manifiestos_reales_zip_y_portable() {
        let rg = r"PackageIdentifier: BurntSushi.ripgrep.MSVC
PackageVersion: 15.2.0
InstallerType: zip
NestedInstallerType: portable
ReleaseDate: 2026-07-15
Installers:
- Architecture: x86
  NestedInstallerFiles:
  - RelativeFilePath: ripgrep-15.2.0-i686-pc-windows-msvc/rg.exe
    PortableCommandAlias: rg
  InstallerUrl: https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-i686-pc-windows-msvc.zip
  InstallerSha256: 9BF73BDB3FDA9AD4B0235E1295B02C717031C986AFA4D7C05DD0AF8B74010A95
- Architecture: x64
  NestedInstallerFiles:
  - RelativeFilePath: ripgrep-15.2.0-x86_64-pc-windows-msvc/rg.exe
    PortableCommandAlias: rg
  InstallerUrl: https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-x86_64-pc-windows-msvc.zip
  InstallerSha256: 71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5
ManifestType: installer
ManifestVersion: 1.12.0
";
        let list = installers(rg);
        assert_eq!(list.len(), 2);
        let x64 = best_installer(&list).unwrap();
        assert_eq!(x64.kind, "zip");
        assert_eq!(x64.nested_kind, "portable");
        assert!(installable(x64));
        assert_eq!(
            x64.nested,
            alloc::vec![Nested {
                path: "ripgrep-15.2.0-x86_64-pc-windows-msvc/rg.exe".into(),
                alias: Some("rg".into()),
            }]
        );
        assert_eq!(
            x64.sha256,
            "71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5"
        );

        let jq = r"PackageIdentifier: jqlang.jq
PackageVersion: 1.8.2
InstallerType: portable
Commands:
- jq
ReleaseDate: 2026-06-20
Installers:
- Architecture: x86
  InstallerUrl: https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-windows-i386.exe
- Architecture: x64
  InstallerUrl: https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-windows-amd64.exe
  InstallerSha256: A6FC67FEDAF9128A3309A1E2EBB8B986AECCF70122EE46D2CB4849E423F0C627
ManifestType: installer
";
        let list = installers(jq);
        let x64 = best_installer(&list).unwrap();
        assert_eq!(x64.kind, "portable");
        assert_eq!(x64.commands, alloc::vec!["jq".to_string()]);
        assert!(x64.url.ends_with("jq-windows-amd64.exe"));
    }

    #[test]
    fn lanzadores() {
        let l = launcher("/Programas/windows/X/rg.exe");
        assert_eq!(
            launcher_target(&l),
            Some("/Programas/windows/X/rg.exe".into())
        );
        assert_eq!(launcher_target("vino /a/b.exe"), Some("/a/b.exe".into()));
        // Un script de verdad (más de un comando, o con otros argumentos) no es un lanzador.
        assert_eq!(launcher_target("echo hola\nwine /a.exe $@"), None);
        assert_eq!(launcher_target("wine /a.exe --rm"), None);
        assert_eq!(launcher_target("wine relativo.exe"), None);
        assert_eq!(launcher_target("wine $X"), None);
    }

    #[test]
    fn nombres_seguros() {
        assert_eq!(safe_command("rg.exe"), Some("rg".into()));
        assert_eq!(safe_command("Jq"), Some("jq".into()));
        assert_eq!(safe_command("../x"), None);
        assert_eq!(safe_command("a b"), None);
        assert_eq!(
            safe_id("BurntSushi.ripgrep.MSVC"),
            Some("BurntSushi.ripgrep.MSVC".into())
        );
        assert_eq!(safe_id("../../Sistema"), None);
        assert!(
            launcher("/Programas/windows/x/rg.exe").contains("wine /Programas/windows/x/rg.exe $@")
        );
    }
}
