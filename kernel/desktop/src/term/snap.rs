//! `snap`: programas empaquetados como en Ubuntu, con los mismos comandos (`snap find`,
//! `snap install`, `snap list`, `snap refresh`, `snap revert`, `snap remove`, `snap info`,
//! `snap download`, `snap run`).
//!
//! A diferencia de `apt`, un snap trae **todo** lo que necesita (no tiene dependencias), viene en
//! **canales** (`stable`, `beta`…) y cada versión es una **revisión** numerada que se guarda
//! aparte: `/snap/<nombre>/<revisión>/`. `/snap/<nombre>/current` dice cuál se usa, así
//! `snap refresh` instala la nueva sin borrar la anterior y `snap revert` vuelve atrás. El
//! comando queda en `/snap/bin` (que está en el `PATH`).
//!
//! Hay dos tiendas:
//! - **La de JARVIS-OS** (`http://paquetes.jarvis/snaps/`, en `kernel/paquetes/snaps/`): snaps
//!   que corren en JARVIS-OS (scripts de `jsh`).
//! - **La de Snapcraft** (`api.snapcraft.io`, la real de Ubuntu): se busca y se ve la
//!   información, y se pueden bajar (`snap download`). Son programas de Linux: JARVIS-OS
//!   todavía no puede ejecutarlos (ver ADR 0005).
//!
//! Índice de la tienda de JARVIS-OS:
//!
//! ```text
//! nombre|versión|revisión|canal|editor|resumen|tamaño|comando
//! ```
//!
//! y un `manifiesto.txt` por revisión (`<nombre>/<revisión>/manifiesto.txt`) con líneas
//! `archivo -> ruta/dentro/del/snap`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};

use super::apt::ensure_dirs;
use super::{Out, ansi};
use crate::apps::Ctx;
use crate::files::{FilesApp, format_size};
use crate::system::{FetchKind, HttpResponse};
use crate::web::json::{self, Json};

pub const STORE: &str = "http://paquetes.jarvis/snaps";
pub const API: &str = "https://api.snapcraft.io/v2/snaps";
pub const ROOT: &str = "/snap";
pub const BIN: &str = "/snap/bin";
pub const DB: &str = "/Sistema/snaps/instalados.txt";
const DOWNLOADS: &str = "/Descargas";
/// Lo más grande que se puede bajar (el tope de las descargas del kernel).
const MAX_DOWNLOAD: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapEntry {
    pub name: String,
    pub version: String,
    pub rev: u32,
    pub channel: String,
    pub publisher: String,
    pub summary: String,
    pub size: String,
    pub command: String,
}

pub fn parse_index(text: &str) -> Vec<SnapEntry> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            if f.len() < 8 || f[0].is_empty() {
                return None;
            }
            Some(SnapEntry {
                name: f[0].into(),
                version: f[1].into(),
                rev: f[2].parse().ok()?,
                channel: f[3].into(),
                publisher: f[4].into(),
                summary: f[5].into(),
                size: f[6].into(),
                command: f[7].into(),
            })
        })
        .collect()
}

/// Un snap instalado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub rev: u32,
    pub channel: String,
    pub publisher: String,
}

pub fn installed<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<Installed> {
    let data = fs.read_file(DB).unwrap_or_default();
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            (f.len() >= 5).then(|| Installed {
                name: f[0].into(),
                version: f[1].into(),
                rev: f[2].parse().unwrap_or(0),
                channel: f[3].into(),
                publisher: f[4].into(),
            })
        })
        .collect()
}

fn save_installed<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    list: &[Installed],
    now: jarvis_fs::Timestamp,
) -> Result<(), jarvis_fs::FsError> {
    let text: String = list
        .iter()
        .map(|s| {
            format!(
                "{}|{}|{}|{}|{}\n",
                s.name, s.version, s.rev, s.channel, s.publisher
            )
        })
        .collect();
    ensure_dirs(fs, "/Sistema/snaps", now)?;
    fs.write_file(DB, text.as_bytes(), now)
}

/// La mejor entrada del índice para `name` en `channel` (si no hay en ese canal, `stable`).
fn pick<'a>(index: &'a [SnapEntry], name: &str, channel: &str) -> Option<&'a SnapEntry> {
    let best = |ch: &str| {
        index
            .iter()
            .filter(|e| e.name == name && e.channel == ch)
            .max_by_key(|e| e.rev)
    };
    best(channel).or_else(|| best("stable"))
}

fn channels(index: &[SnapEntry], name: &str) -> String {
    let mut s = String::new();
    for ch in ["stable", "candidate", "beta", "edge"] {
        let line = match index
            .iter()
            .filter(|e| e.name == name && e.channel == ch)
            .max_by_key(|e| e.rev)
        {
            Some(e) => format!(
                "  latest/{ch:<10} {:<10} ({}) {}\n",
                e.version, e.rev, e.size
            ),
            None => format!("  latest/{ch:<10} ^\n"),
        };
        s.push_str(&line);
    }
    s
}

fn launcher(name: &str, rev: u32, version: &str, command: &str) -> String {
    format!(
        "#!/bin/jsh\n# snap {name} {version} (revisión {rev}): lo generó `snap install`.\n\
         sh /snap/{name}/{rev}/{command} $@\n"
    )
}

// --- trabajos que esperan la red --------------------------------------------------------------

enum Step {
    /// `snap find`: índice propio, después la tienda de Snapcraft.
    Find { query: String, stage: u8 },
    /// `snap info`: índice propio; si no está, Snapcraft.
    Info { name: String, stage: u8 },
    /// `snap install` / `refresh`: índice, después manifiesto y archivos de cada uno.
    Install {
        queue: Vec<(String, String)>,
        refresh: bool,
        stage: InstallStage,
    },
    /// `snap download`: información en Snapcraft, después el archivo.
    Download { name: String, stage: u8 },
}

enum InstallStage {
    Index,
    /// Preguntando a Snapcraft si existe (para explicar por qué no se puede instalar).
    Probe(String),
    Manifest(SnapEntry),
    Files {
        entry: SnapEntry,
        files: Vec<(String, String)>,
        next: usize,
        data: Vec<(String, Vec<u8>)>,
    },
}

pub struct SnapJob {
    step: Step,
    waiting: Option<u32>,
    index: Vec<SnapEntry>,
    done: u32,
}

impl SnapJob {
    pub fn waiting_id(&self) -> Option<u32> {
        self.waiting
    }

    fn get<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>, url: &str, kind: FetchKind) {
        self.waiting = Some(ctx.out.fetch_as(url, kind, "snap"));
    }

    /// Llegó una respuesta. `Some((código, salida))` cuando terminó.
    pub(crate) fn on_response<D: BlockDevice>(
        &mut self,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        self.waiting = None;
        let ok = match result {
            Ok(r) if r.status == 200 => Some(r),
            _ => None,
        };
        let fail = |out: &mut Out, what: &str| {
            let why = match result {
                Ok(r) => format!("HTTP {}", r.status),
                Err(e) => e.clone(),
            };
            out.err(&format!("error: no se pudo {what}: {why}"));
            Some((1, String::new()))
        };
        match &mut self.step {
            Step::Find { query, stage } => {
                let query = query.clone();
                if *stage == 0 {
                    let Some(r) = ok else {
                        return fail(out, "leer la tienda de JARVIS-OS");
                    };
                    self.index = parse_index(&String::from_utf8_lossy(&r.body));
                    let q = query.to_lowercase();
                    let mut seen: Vec<&str> = Vec::new();
                    let mut rows = String::new();
                    for e in self.index.iter().filter(|e| e.channel == "stable") {
                        if seen.contains(&e.name.as_str()) {
                            continue;
                        }
                        if q.is_empty()
                            || e.name.contains(&q)
                            || e.summary.to_lowercase().contains(&q)
                        {
                            seen.push(&e.name);
                            rows.push_str(&format!(
                                "{:<14}{:<10}{:<14}{:<8}{}\n",
                                e.name, e.version, e.publisher, "-", e.summary
                            ));
                        }
                    }
                    if rows.is_empty() {
                        out.info(&format!("No hay snaps de JARVIS-OS para \"{query}\"."));
                    } else {
                        out.info(&format!(
                            "{}Tienda de JARVIS-OS{}\n{:<14}{:<10}{:<14}{:<8}{}\n{}",
                            ansi::BOLD,
                            ansi::RESET,
                            "Nombre",
                            "Versión",
                            "Editor",
                            "Notas",
                            "Resumen",
                            rows.trim_end()
                        ));
                    }
                    if query.is_empty() {
                        return Some((0, String::new()));
                    }
                    *stage = 1;
                    let url = format!(
                        "{API}/find?q={}&fields=title,summary,version,publisher",
                        crate::web::url::percent_encode(&query)
                    );
                    self.get(ctx, &url, FetchKind::Page);
                    return None;
                }
                let Some(r) = ok else {
                    out.info("(La tienda de Snapcraft no respondió.)");
                    return Some((0, String::new()));
                };
                let v = json::parse(&String::from_utf8_lossy(&r.body))?;
                let results = v.get("results").map(Json::arr).unwrap_or(&[]);
                if results.is_empty() {
                    return Some((0, String::new()));
                }
                let mut s = format!(
                    "\n{}Tienda de Snapcraft{} {}(programas de Linux: se pueden bajar con snap download, todavía no ejecutar){}\n",
                    ansi::BOLD,
                    ansi::RESET,
                    ansi::DIM,
                    ansi::RESET
                );
                for x in results.iter().take(12) {
                    let name = x.get("name").and_then(Json::str).unwrap_or("?");
                    let ver = x
                        .path(&["revision", "version"])
                        .and_then(Json::str)
                        .unwrap_or("-");
                    let publisher = x
                        .path(&["snap", "publisher", "username"])
                        .and_then(Json::str)
                        .unwrap_or("-");
                    let verified = x
                        .path(&["snap", "publisher", "validation"])
                        .and_then(Json::str)
                        == Some("verified");
                    let summary = x
                        .path(&["snap", "summary"])
                        .and_then(Json::str)
                        .unwrap_or("");
                    s.push_str(&format!(
                        "{name:<22}{:<14}{:<16}{summary}\n",
                        short(ver, 13),
                        format!("{publisher}{}", if verified { "*" } else { "" })
                    ));
                }
                out.info(s.trim_end());
                Some((0, String::new()))
            }
            Step::Info { name, stage } => {
                let name = name.clone();
                if *stage == 0 {
                    if let Some(r) = ok {
                        self.index = parse_index(&String::from_utf8_lossy(&r.body));
                    }
                    if let Some(e) = pick(&self.index, &name, "stable") {
                        let have = ctx
                            .fs
                            .as_deref_mut()
                            .map(installed)
                            .unwrap_or_default()
                            .into_iter()
                            .find(|s| s.name == name);
                        out.info(&format!(
                            "name:      {}\nsummary:   {}\npublisher: {} (tienda de JARVIS-OS)\nlicense:   MIT\ncommands:\n  - {}\n{}channels:\n{}",
                            e.name,
                            e.summary,
                            e.publisher,
                            e.name,
                            match have {
                                Some(h) => format!(
                                    "tracking:  latest/{}\ninstalled: {} ({})\n",
                                    h.channel, h.version, h.rev
                                ),
                                None => String::new(),
                            },
                            channels(&self.index, &name).trim_end()
                        ));
                        return Some((0, String::new()));
                    }
                    *stage = 1;
                    let url = format!(
                        "{API}/info/{name}?fields=title,summary,description,version,publisher,download,license"
                    );
                    self.get(ctx, &url, FetchKind::Page);
                    return None;
                }
                let Some(r) = ok else {
                    out.err(&format!("error: no se encontró el snap \"{name}\""));
                    return Some((1, String::new()));
                };
                let v = json::parse(&String::from_utf8_lossy(&r.body))?;
                let snap = v.get("snap");
                let text = |k: &str| {
                    snap.and_then(|s| s.get(k))
                        .and_then(Json::str)
                        .unwrap_or("")
                        .to_string()
                };
                let mut s = format!(
                    "name:      {name}\nsummary:   {}\npublisher: {}\nlicense:   {}\ndescription: |\n  {}\n",
                    text("summary"),
                    snap.and_then(|s| s.path(&["publisher", "display-name"]))
                        .and_then(Json::str)
                        .unwrap_or("-"),
                    text("license"),
                    short(&text("description").replace('\n', "\n  "), 600)
                );
                s.push_str("channels:\n");
                for c in v.get("channel-map").map(Json::arr).unwrap_or(&[]) {
                    if c.path(&["channel", "architecture"]).and_then(Json::str) != Some("amd64") {
                        continue;
                    }
                    let ch = c
                        .path(&["channel", "name"])
                        .and_then(Json::str)
                        .unwrap_or("?");
                    let track = c
                        .path(&["channel", "track"])
                        .and_then(Json::str)
                        .unwrap_or("latest");
                    let ver = c.get("version").and_then(Json::str).unwrap_or("?");
                    let size = c
                        .path(&["download", "size"])
                        .and_then(Json::str)
                        .and_then(|x| x.parse::<u64>().ok())
                        .map(format_size)
                        .unwrap_or_default();
                    s.push_str(&format!("  {track}/{ch:<10} {ver:<16} {size}\n"));
                }
                s.push_str(&format!(
                    "{}Es un programa de Linux: JARVIS-OS lo puede bajar (snap download {name}) pero todavía no ejecutarlo.{}",
                    ansi::DIM,
                    ansi::RESET
                ));
                out.info(&s);
                Some((0, String::new()))
            }
            Step::Download { name, stage } => {
                let name = name.clone();
                if *stage == 0 {
                    let Some(r) = ok else {
                        out.err(&format!(
                            "error: snap \"{name}\" no encontrado en Snapcraft"
                        ));
                        return Some((1, String::new()));
                    };
                    let v = json::parse(&String::from_utf8_lossy(&r.body))?;
                    let chosen = v
                        .get("channel-map")
                        .map(Json::arr)
                        .unwrap_or(&[])
                        .iter()
                        .find(|c| {
                            c.path(&["channel", "architecture"]).and_then(Json::str)
                                == Some("amd64")
                                && c.path(&["channel", "risk"]).and_then(Json::str)
                                    == Some("stable")
                        })?
                        .clone();
                    let url = chosen
                        .path(&["download", "url"])
                        .and_then(Json::str)?
                        .to_string();
                    let size: u64 = chosen
                        .path(&["download", "size"])
                        .and_then(Json::str)
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(0);
                    if size > MAX_DOWNLOAD {
                        out.err(&format!(
                            "error: {name} pesa {}: JARVIS-OS baja hasta {} por vez.",
                            format_size(size),
                            format_size(MAX_DOWNLOAD)
                        ));
                        return Some((1, String::new()));
                    }
                    out.info(&format!(
                        "Bajando snap \"{name}\" ({}) de Snapcraft...",
                        format_size(size)
                    ));
                    *stage = 1;
                    self.get(ctx, &url, FetchKind::Download);
                    return None;
                }
                let Some(r) = ok else {
                    return fail(out, "bajar el snap");
                };
                let now = ctx.timestamp();
                let fs = ctx.fs.as_deref_mut()?;
                let path = format!("{DOWNLOADS}/{name}.snap");
                let saved = ensure_dirs(fs, DOWNLOADS, now)
                    .and_then(|()| fs.write_file(&path, &r.body, now));
                if let Err(e) = saved {
                    out.err(&format!("error: no se pudo guardar {path}: {e}"));
                    return Some((1, String::new()));
                }
                ctx.log.push(format!("SNAP_DESCARGADO {name}"));
                out.info(&format!(
                    "Guardado {path} ({}).\nEs un paquete snap de Linux (squashfs): miralo con `file {path}`. \
                     Ejecutarlo necesita el espacio de usuario de Linux (hito K9).",
                    format_size(r.body.len() as u64)
                ));
                Some((0, String::new()))
            }
            Step::Install { .. } => self.on_install(ok, result, ctx, out),
        }
    }

    fn on_install<D: BlockDevice>(
        &mut self,
        ok: Option<&HttpResponse>,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        let Step::Install {
            queue,
            refresh,
            stage,
        } = &mut self.step
        else {
            return None;
        };
        let refresh = *refresh;
        match stage {
            InstallStage::Index => {
                let Some(r) = ok else {
                    let why = match result {
                        Ok(r) => format!("HTTP {}", r.status),
                        Err(e) => e.clone(),
                    };
                    out.err(&format!(
                        "error: no se pudo leer la tienda ({why})\n(la sirve `cargo xtask run` desde el anfitrión)"
                    ));
                    return Some((1, String::new()));
                };
                self.index = parse_index(&String::from_utf8_lossy(&r.body));
                if refresh {
                    // Cuáles tienen otra revisión en su canal (o en el canal nuevo que se pidió).
                    let have = ctx.fs.as_deref_mut().map(installed).unwrap_or_default();
                    let mut next = Vec::new();
                    for h in &have {
                        let wanted = queue.iter().find(|q| q.0 == h.name);
                        if !queue.is_empty() && wanted.is_none() {
                            continue;
                        }
                        let ch = match wanted {
                            Some((_, c)) if !c.is_empty() => c.clone(),
                            _ => h.channel.clone(),
                        };
                        if let Some(e) = pick(&self.index, &h.name, &ch)
                            && (e.rev > h.rev || (ch != h.channel && e.rev != h.rev))
                        {
                            next.push((h.name.clone(), ch));
                        }
                    }
                    if next.is_empty() {
                        out.info("Todos los snaps están actualizados.");
                        return Some((0, String::new()));
                    }
                    *queue = next;
                }
            }
            InstallStage::Probe(name) => {
                let name = name.clone();
                if ok.is_some() {
                    out.err(&format!(
                        "error: \"{name}\" es un snap de Linux (de la tienda de Snapcraft): JARVIS-OS \
                         todavía no puede ejecutarlo.\nSe puede bajar para inspeccionarlo: snap download {name}"
                    ));
                } else {
                    out.err(&format!("error: snap \"{name}\" no encontrado"));
                }
                return Some((1, String::new()));
            }
            InstallStage::Manifest(entry) => {
                let entry = entry.clone();
                let Some(r) = ok else {
                    out.err(&format!(
                        "error: el snap {} no tiene manifiesto",
                        entry.name
                    ));
                    return Some((1, String::new()));
                };
                let files: Vec<(String, String)> = String::from_utf8_lossy(&r.body)
                    .lines()
                    .filter_map(|l| {
                        let (a, b) = l.split_once("->")?;
                        let (a, b) = (a.trim(), b.trim().trim_start_matches('/'));
                        (!a.is_empty() && !a.contains("..") && !b.contains("..") && !b.is_empty())
                            .then(|| (a.to_string(), b.to_string()))
                    })
                    .collect();
                if files.is_empty() {
                    out.err(&format!("error: el snap {} está vacío", entry.name));
                    return Some((1, String::new()));
                }
                let url = format!("{STORE}/{}/{}/{}", entry.name, entry.rev, files[0].0);
                *stage = InstallStage::Files {
                    entry,
                    files,
                    next: 0,
                    data: Vec::new(),
                };
                self.get(ctx, &url, FetchKind::Download);
                return None;
            }
            InstallStage::Files {
                entry,
                files,
                next,
                data,
            } => {
                let Some(r) = ok else {
                    out.err(&format!("error: falta un archivo del snap {}", entry.name));
                    return Some((1, String::new()));
                };
                data.push((files[*next].1.clone(), r.body.clone()));
                *next += 1;
                if *next < files.len() {
                    let url = format!("{STORE}/{}/{}/{}", entry.name, entry.rev, files[*next].0);
                    self.get(ctx, &url, FetchKind::Download);
                    return None;
                }
                let entry = entry.clone();
                let data = core::mem::take(data);
                if let Err(e) = install(&entry, &data, ctx) {
                    out.err(&format!("error: no se pudo instalar {}: {e}", entry.name));
                    return Some((1, String::new()));
                }
                self.done += 1;
                out.info(&format!(
                    "{}{} {} de {} {}{}",
                    ansi::GREEN,
                    entry.name,
                    entry.version,
                    entry.publisher,
                    if refresh {
                        format!("actualizado (revisión {})", entry.rev)
                    } else if entry.channel != "stable" {
                        format!("({}) instalado", entry.channel)
                    } else {
                        "instalado".to_string()
                    },
                    ansi::RESET
                ));
            }
        }
        // Siguiente de la cola.
        let Some((name, channel)) = (if let Step::Install { queue, .. } = &mut self.step {
            (!queue.is_empty()).then(|| queue.remove(0))
        } else {
            None
        }) else {
            return Some((0, String::new()));
        };
        let entry = pick(&self.index, &name, &channel).cloned();
        let Step::Install { stage, .. } = &mut self.step else {
            return None;
        };
        match entry {
            Some(e) => {
                let url = format!("{STORE}/{}/{}/manifiesto.txt", e.name, e.rev);
                *stage = InstallStage::Manifest(e);
                self.get(ctx, &url, FetchKind::Download);
            }
            None => {
                // No es de la tienda de JARVIS-OS: ¿existe en Snapcraft?
                let url = format!("{API}/info/{name}?fields=title");
                *stage = InstallStage::Probe(name);
                self.get(ctx, &url, FetchKind::Page);
            }
        }
        None
    }
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Escribe los archivos de una revisión, cambia `current` y el lanzador de `/snap/bin`.
fn install<D: BlockDevice>(
    e: &SnapEntry,
    data: &[(String, Vec<u8>)],
    ctx: &mut Ctx<'_, D>,
) -> Result<(), String> {
    let now = ctx.timestamp();
    let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
    let base = format!("{ROOT}/{}/{}", e.name, e.rev);
    ensure_dirs(fs, &base, now).map_err(|x| x.to_string())?;
    fs.write_file(&format!("{base}/.snap"), revision_meta(e).as_bytes(), now)
        .map_err(|x| x.to_string())?;
    for (rel, bytes) in data {
        let dest = format!("{base}/{rel}");
        ensure_dirs(fs, &crate::files::parent(&dest), now).map_err(|x| x.to_string())?;
        fs.write_file(&dest, bytes, now)
            .map_err(|x| format!("{dest}: {x}"))?;
    }
    set_current(fs, e, now)?;
    let mut have = installed(fs);
    have.retain(|s| s.name != e.name);
    have.push(Installed {
        name: e.name.clone(),
        version: e.version.clone(),
        rev: e.rev,
        channel: e.channel.clone(),
        publisher: e.publisher.clone(),
    });
    save_installed(fs, &have, now).map_err(|x| x.to_string())?;
    ctx.log
        .push(format!("SNAP_INSTALADO {} {} {}", e.name, e.version, e.rev));
    Ok(())
}

fn set_current<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    e: &SnapEntry,
    now: jarvis_fs::Timestamp,
) -> Result<(), String> {
    let dir = format!("{ROOT}/{}", e.name);
    fs.write_file(
        &format!("{dir}/current"),
        format!("{}|{}|{}|{}\n", e.rev, e.version, e.channel, e.command).as_bytes(),
        now,
    )
    .map_err(|x| x.to_string())?;
    ensure_dirs(fs, BIN, now).map_err(|x| x.to_string())?;
    fs.write_file(
        &format!("{BIN}/{}", e.name),
        launcher(&e.name, e.rev, &e.version, &e.command).as_bytes(),
        now,
    )
    .map_err(|x| x.to_string())
}

const HELP: &str = "snap 2.61 (JARVIS-OS)
Uso: snap <comando> [opciones]

  find [TEXTO]        busca snaps (tienda de JARVIS-OS y de Snapcraft)
  info NOMBRE         información y canales
  install NOMBRE      instala (--beta, --edge, --candidate o --channel=beta)
  list                snaps instalados
  refresh [NOMBRE]    actualiza a la última revisión de su canal
  revert NOMBRE       vuelve a la revisión anterior
  remove NOMBRE       desinstala (va a la Papelera)
  download NOMBRE     baja un snap de Snapcraft a /Descargas (no se ejecuta todavía)
  run NOMBRE [ARGS]   ejecuta un snap
  version             versiones";

/// `snap …`. `Ok` = terminó; `Err(job)` = tiene que bajar algo.
pub(crate) fn run<D: BlockDevice>(
    args: &[String],
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
) -> Result<i32, Box<SnapJob>> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args
        .iter()
        .skip(2)
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();
    // El canal pedido (`--beta`, `--channel=latest/edge`); vacío = el de siempre.
    let mut channel = String::new();
    for a in args.iter().skip(2) {
        match a.as_str() {
            "--beta" => channel = "beta".into(),
            "--edge" => channel = "edge".into(),
            "--candidate" => channel = "candidate".into(),
            "--stable" => channel = "stable".into(),
            x if x.starts_with("--channel=") => {
                channel = x["--channel=".len()..]
                    .rsplit('/')
                    .next()
                    .unwrap_or("stable")
                    .to_string()
            }
            _ => {}
        }
    }
    let job = |step: Step| SnapJob {
        step,
        waiting: None,
        index: Vec::new(),
        done: 0,
    };
    let index_url = format!("{STORE}/indice.txt");
    match cmd {
        "" | "help" | "--help" | "-h" => {
            o.push_str(HELP);
            o.push('\n');
            Ok(0)
        }
        "version" | "--version" => {
            o.push_str(
                "snap    2.61 (JARVIS-OS)\nsnapd   2.61\nseries  16\njarvis  0.1\nkernel  0.1-k5\n",
            );
            Ok(0)
        }
        "find" | "search" => {
            let mut j = job(Step::Find {
                query: rest.join(" "),
                stage: 0,
            });
            j.get(ctx, &index_url, FetchKind::Page);
            Err(Box::new(j))
        }
        "info" => {
            let Some(name) = rest.first() else {
                e.push_str("error: decime qué snap (snap info clima)\n");
                return Ok(1);
            };
            let mut j = job(Step::Info {
                name: name.clone(),
                stage: 0,
            });
            j.get(ctx, &index_url, FetchKind::Page);
            Err(Box::new(j))
        }
        "install" | "refresh" => {
            let refresh = cmd == "refresh";
            if !refresh && rest.is_empty() {
                e.push_str("error: decime qué snap instalar (snap install clima)\n");
                return Ok(1);
            }
            if !refresh && let Some(fs) = ctx.fs.as_deref_mut() {
                let have = installed(fs);
                if let Some(h) = rest.iter().find_map(|n| have.iter().find(|h| h.name == *n))
                    && rest.len() == 1
                {
                    o.push_str(&format!(
                        "snap \"{}\" ya está instalado; para actualizarlo: snap refresh {}\n",
                        h.name, h.name
                    ));
                    return Ok(0);
                }
            }
            let queue = rest
                .iter()
                .map(|n| {
                    let ch = if channel.is_empty() && !refresh {
                        "stable".to_string()
                    } else {
                        channel.clone()
                    };
                    (n.clone(), ch)
                })
                .collect();
            let mut j = job(Step::Install {
                queue,
                refresh,
                stage: InstallStage::Index,
            });
            j.get(ctx, &index_url, FetchKind::Page);
            Err(Box::new(j))
        }
        "download" => {
            let Some(name) = rest.first() else {
                e.push_str("error: decime qué snap bajar\n");
                return Ok(1);
            };
            let mut j = job(Step::Download {
                name: name.clone(),
                stage: 0,
            });
            j.get(
                ctx,
                &format!("{API}/info/{name}?fields=download,version"),
                FetchKind::Page,
            );
            Err(Box::new(j))
        }
        "list" => {
            let Some(fs) = ctx.fs.as_deref_mut() else {
                e.push_str("error: no hay disco\n");
                return Ok(1);
            };
            let have = installed(fs);
            if have.is_empty() {
                o.push_str(
                    "Todavía no hay snaps instalados. Probá: snap find, snap install clima\n",
                );
                return Ok(0);
            }
            o.push_str("Nombre        Versión   Rev  Canal            Editor\n");
            for s in have {
                o.push_str(&format!(
                    "{:<14}{:<10}{:<5}{:<17}{}\n",
                    s.name,
                    s.version,
                    s.rev,
                    format!("latest/{}", s.channel),
                    s.publisher
                ));
            }
            Ok(0)
        }
        "remove" => {
            let Some(fs) = ctx.fs.as_deref_mut() else {
                e.push_str("error: no hay disco\n");
                return Ok(1);
            };
            let now = crate::apps::timestamp(ctx.clock);
            let mut have = installed(fs);
            let mut code = 0;
            for name in &rest {
                if !have.iter().any(|s| s.name == *name) {
                    e.push_str(&format!("snap \"{name}\" no está instalado\n"));
                    code = 1;
                    continue;
                }
                for path in [format!("{ROOT}/{name}"), format!("{BIN}/{name}")] {
                    if fs.exists(&path) {
                        let _ = FilesApp::move_to_trash(fs, &path, now);
                    }
                }
                have.retain(|s| s.name != *name);
                ctx.log.push(format!("SNAP_DESINSTALADO {name}"));
                o.push_str(&format!("{name} desinstalado (a la Papelera)\n"));
            }
            let _ = save_installed(fs, &have, now);
            Ok(code)
        }
        "revert" => {
            let Some(name) = rest.first() else {
                e.push_str("error: decime qué snap\n");
                return Ok(1);
            };
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let now = crate::apps::timestamp(ctx.clock);
            let mut have = installed(fs);
            let Some(cur) = have.iter().position(|s| s.name == *name) else {
                e.push_str(&format!("snap \"{name}\" no está instalado\n"));
                return Ok(1);
            };
            // Revisiones que quedaron en el disco, menores que la actual.
            let current = have[cur].rev;
            let prev = fs
                .list(&format!("{ROOT}/{name}"))
                .unwrap_or_default()
                .iter()
                .filter(|x| x.is_dir)
                .filter_map(|x| x.name.parse::<u32>().ok())
                .filter(|r| *r < current)
                .max();
            let Some(prev) = prev else {
                e.push_str(&format!(
                    "error: no hay una revisión anterior de \"{name}\" en el disco\n"
                ));
                return Ok(1);
            };
            // La versión y el comando de esa revisión: los del índice guardado en su carpeta.
            let meta = fs
                .read_file(&format!("{ROOT}/{name}/{prev}/.snap"))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let mut f = meta.trim().split('|');
            let entry = SnapEntry {
                name: name.clone(),
                version: f.next().unwrap_or("?").to_string(),
                rev: prev,
                channel: f.next().unwrap_or("stable").to_string(),
                publisher: have[cur].publisher.clone(),
                summary: String::new(),
                size: String::new(),
                command: f.next().unwrap_or(name).to_string(),
            };
            if let Err(x) = set_current(fs, &entry, now) {
                e.push_str(&format!("error: {x}\n"));
                return Ok(1);
            }
            have[cur].rev = prev;
            have[cur].version = entry.version.clone();
            let _ = save_installed(fs, &have, now);
            ctx.log.push(format!("SNAP_REVERT {name} {prev}"));
            o.push_str(&format!(
                "{name} volvió a la versión {} (revisión {prev})\n",
                entry.version
            ));
            Ok(0)
        }
        other => {
            e.push_str(&format!(
                "error: comando desconocido \"{other}\" (ver: snap help)\n"
            ));
            Ok(1)
        }
    }
}

/// Lo que se guarda en cada revisión para poder volver a ella (`snap revert`).
pub fn revision_meta(e: &SnapEntry) -> String {
    format!("{}|{}|{}\n", e.version, e.channel, e.command)
}

impl SnapEntry {
    #[cfg(test)]
    fn sample(rev: u32) -> SnapEntry {
        SnapEntry {
            name: "clima".into(),
            version: format!("1.{rev}"),
            rev,
            channel: "stable".into(),
            publisher: "jarvis".into(),
            summary: String::new(),
            size: String::new(),
            command: "clima.sh".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indice_canales_y_lanzador() {
        let idx = parse_index(
            "# comentario\nclima|1.0|3|stable|jarvis|El clima|2 KB|clima.sh\n\
             clima|1.1|5|beta|jarvis|El clima|2 KB|clima.sh\nroto|1\n",
        );
        assert_eq!(idx.len(), 2);
        assert_eq!(pick(&idx, "clima", "beta").unwrap().rev, 5);
        assert_eq!(
            pick(&idx, "clima", "edge").unwrap().rev,
            3,
            "si no hay, stable"
        );
        assert!(channels(&idx, "clima").contains("latest/beta       1.1 "));
        assert!(launcher("clima", 3, "1.0", "clima.sh").contains("sh /snap/clima/3/clima.sh $@"));
        assert!(crate::term::apt::newer("1.10", "1.9"));
        assert_eq!(
            revision_meta(&SnapEntry::sample(3)),
            "1.3|stable|clima.sh\n"
        );
    }
}
