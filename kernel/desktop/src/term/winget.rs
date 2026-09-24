//! `winget`: el gestor de paquetes de Windows, contra su repositorio oficial
//! (`github.com/microsoft/winget-pkgs`, el mismo que usa Windows).
//!
//! Cada programa tiene un Id (`7zip.7zip`, `Notepad++.Notepad++`) y, por versión, manifiestos
//! en YAML: `manifests/<letra>/<Editor>/<Programa>/<versión>/<Id>.installer.yaml` (dónde está el
//! instalador) y `<Id>.locale.en-US.yaml` (nombre y descripción).
//!
//! - `winget search` busca en una lista de programas conocidos
//!   (`http://paquetes.jarvis/winget.txt`): el índice completo de winget es una base SQLite que
//!   no vale la pena bajar. Con el Id exacto, `winget show` e `install` van al repositorio real.
//! - `winget install` baja el instalador (`.exe` o `.msi`) a `/Descargas`. **No lo ejecuta**:
//!   un programa de Windows necesita espacio de usuario y la API de Windows (ADR 0005). Se puede
//!   inspeccionar con `file`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;

use super::apt::{ensure_dirs, newer};
use super::{Out, ansi};
use crate::apps::Ctx;
use crate::files::format_size;
use crate::system::{FetchKind, HttpResponse};
use crate::web::json::{self, Json};

pub const LIST: &str = "http://paquetes.jarvis/winget.txt";
const API: &str = "https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests";
const RAW: &str = "https://raw.githubusercontent.com/microsoft/winget-pkgs/master/manifests";
const DOWNLOADS: &str = "/Descargas";
const DB: &str = "/Sistema/winget/descargados.txt";

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

/// Instaladores del manifiesto: (arquitectura, tipo, dirección).
fn installers(text: &str) -> Vec<(String, String, String)> {
    let default_type = yaml_field(text, "InstallerType").unwrap_or_default();
    let mut out = Vec::new();
    let mut arch = String::new();
    let mut ty = default_type.clone();
    let mut in_list = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("Installers:") {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        let t = t.trim_start_matches("- ");
        if let Some(v) = t.strip_prefix("Architecture:") {
            arch = v.trim().into();
            ty = default_type.clone();
        } else if let Some(v) = t.strip_prefix("InstallerType:") {
            ty = v.trim().into();
        } else if let Some(v) = t.strip_prefix("InstallerUrl:") {
            out.push((arch.clone(), ty.clone(), v.trim().to_string()));
        }
    }
    out
}

/// El instalador para una PC de 64 bits (o el que haya).
fn best_installer(list: &[(String, String, String)]) -> Option<&(String, String, String)> {
    list.iter()
        .find(|i| i.0 == "x64")
        .or_else(|| list.iter().find(|i| i.0 == "x86"))
        .or_else(|| list.iter().find(|i| i.0 == "neutral"))
        .or_else(|| list.first())
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
    url: String,
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
                let list = installers(&text);
                let Some((arch, ty, url)) = best_installer(&list).cloned() else {
                    out.err("El manifiesto no tiene instaladores.");
                    return Some((1, String::new()));
                };
                let mut info = core::mem::take(&mut self.info);
                info.push_str(&format!("Instalador: {ty} ({arch})\n  {url}"));
                out.info(&info);
                if !self.install {
                    return Some((0, String::new()));
                }
                self.url = url.clone();
                out.info("Descargando el instalador...");
                self.step = Step::File;
                self.get(ctx, &url, FetchKind::Download);
                None
            }
            Step::File => {
                let Some(r) = body else {
                    out.err(&format!(
                        "No se pudo bajar el instalador ({}). JARVIS-OS baja hasta 32 MB por vez.",
                        why()
                    ));
                    return Some((1, String::new()));
                };
                let now = ctx.timestamp();
                let fs = ctx.fs.as_deref_mut()?;
                let mut name = crate::web::url::percent_decode(
                    self.url
                        .split('?')
                        .next()
                        .unwrap_or("")
                        .rsplit('/')
                        .next()
                        .unwrap_or("instalador.exe"),
                );
                if !name.contains('.') {
                    name.push_str(".exe");
                }
                let path = format!("{DOWNLOADS}/{name}");
                let saved = ensure_dirs(fs, DOWNLOADS, now)
                    .and_then(|()| fs.write_file(&path, &r.body, now));
                if let Err(e) = saved {
                    out.err(&format!("No se pudo guardar {path}: {e}"));
                    return Some((1, String::new()));
                }
                let mut db = fs
                    .read_file(DB)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                db.push_str(&format!("{}|{}|{path}\n", self.id, self.version));
                let _ = ensure_dirs(fs, "/Sistema/winget", now)
                    .and_then(|()| fs.write_file(DB, db.as_bytes(), now));
                let what = super::binfmt::describe(&r.body[..r.body.len().min(256 * 1024)]);
                ctx.log
                    .push(format!("WINGET_DESCARGADO {} {path}", self.id));
                out.info(&format!(
                    "Descargado {path} ({}).\n{what}\n{}JARVIS-OS todavía no puede ejecutar instaladores de Windows (le falta el espacio de usuario y la API de Windows). Inspeccionalo con: file {path}{}",
                    format_size(r.body.len() as u64),
                    ansi::DIM,
                    ansi::RESET
                ));
                Some((0, String::new()))
            }
        }
    }
}

const HELP: &str = "Administrador de paquetes de Windows (winget) v1.9 · JARVIS-OS
Uso: winget <comando> [opciones]

  search TEXTO    busca en la lista de programas conocidos
  show ID         muestra un paquete del repositorio oficial
  install ID      baja el instalador a /Descargas (no se ejecuta todavía)
  list            instaladores bajados

Ejemplos:  winget search zip  ·  winget install 7zip.7zip";

pub(crate) fn run<D: BlockDevice>(
    args: &[String],
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
) -> Result<i32, Box<WingetJob>> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    // `--id X`, `-e`, `--exact`: se aceptan (como en Windows).
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter().skip(2);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--id" | "-q" | "--query" => {
                if let Some(v) = it.next() {
                    rest.push(v.clone());
                }
            }
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
        url: String::new(),
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
        "show" | "view" | "install" | "add" => {
            let Some(id) = rest.first() else {
                e.push_str(&format!(
                    "Falta el Id del paquete: winget {cmd} 7zip.7zip\n"
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
            let db = fs
                .read_file(DB)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            if db.trim().is_empty() {
                o.push_str(
                    "Todavía no se bajó ningún instalador. Probá: winget install 7zip.7zip\n",
                );
                return Ok(0);
            }
            o.push_str(&format!(
                "{:<30}{:<14}{}\n{}\n",
                "Id",
                "Versión",
                "Archivo",
                "-".repeat(70)
            ));
            for l in db.lines() {
                let f: Vec<&str> = l.split('|').collect();
                if f.len() >= 3 {
                    o.push_str(&format!("{:<30}{:<14}{}\n", f[0], f[1], f[2]));
                }
            }
            Ok(0)
        }
        "upgrade" | "update" | "uninstall" | "remove" => {
            e.push_str(&format!(
                "winget {cmd}: en JARVIS-OS los instaladores de Windows solo se bajan (no se instalan todavía).\n"
            ));
            Ok(1)
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
        assert_eq!(
            best_installer(&list).unwrap(),
            &(
                "x64".to_string(),
                "exe".to_string(),
                "https://7-zip.org/a/7z2408-x64.exe".to_string()
            )
        );
        assert_eq!(list[2].1, "msi");
        assert_eq!(
            yaml_field(
                "PackageName: \"7-Zip\"\nShortDescription: Archivador",
                "PackageName"
            ),
            Some("7-Zip".into())
        );
    }
}
