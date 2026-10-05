//! `apt`: el gestor de paquetes de JARVIS-OS, con los mismos comandos que en Debian/Ubuntu
//! (`apt update`, `apt install`, `apt remove`, `apt upgrade`, `apt list`, `apt search`,
//! `apt show`).
//!
//! **Repositorio**: `http://paquetes.jarvis/` (lo sirve el puente del anfitrión desde la carpeta
//! `kernel/paquetes/` del proyecto). Tiene un índice y, por cada paquete, un manifiesto:
//!
//! ```text
//! indice.txt:                  nombre|versión|descripción|dependencias|tamaño
//! <nombre>/manifiesto.txt:     archivo-en-el-repo -> /ruta/en/el/disco   [bmp]
//!                              usuario:programa -> /Programas/bin/programa
//! ```
//!
//! `usuario:` es un programa de Linux compilado desde `kernel/usuario/` (K11): el puente lo sirve
//! en `http://paquetes.jarvis/usuario/`.
//!
//! **Base de datos local** (en `/Sistema/paquetes/`): la copia del índice, la lista de paquetes
//! instalados (`instalados.txt`) y, por cada uno, qué archivos puso (`<nombre>.lista`), para
//! poder desinstalarlo. Desinstalar mueve esos archivos a la Papelera: nada se borra para
//! siempre sin preguntar.
//!
//! Los programas son scripts de `jsh` (van a `/Programas/bin`, que está en el `PATH`); también
//! hay paquetes de datos (fondos de pantalla, canciones, documentos).
//!
//! **Debian**: lo que no está en el repositorio de JARVIS-OS se busca en Debian estable
//! (`deb.debian.org`, ver [`super::debian`]): programas de Linux de verdad, con glibc. `apt
//! update` baja también su índice; `apt install hello` baja `hello` y sus dependencias (`libc6`…)
//! y los desarma en el disco (`/usr/bin`, `/usr/lib/x86_64-linux-gnu`…).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};

use super::debian::{self, DebPkg};
use super::{Out, ansi};
use crate::apps::Ctx;
use crate::files::{FilesApp, format_size, parent};
use crate::system::{FetchKind, HttpResponse};

pub const REPO: &str = "http://paquetes.jarvis";
pub const DB: &str = "/Sistema/paquetes";
pub const INDEX: &str = "/Sistema/paquetes/indice.txt";
pub const INSTALLED: &str = "/Sistema/paquetes/instalados.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,
    pub depends: Vec<String>,
    pub size: String,
}

pub fn parse_index(text: &str) -> Vec<Package> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            let name = f.first().filter(|n| !n.is_empty())?;
            Some(Package {
                name: name.to_string(),
                version: f.get(1).unwrap_or(&"0").to_string(),
                description: f.get(2).unwrap_or(&"").to_string(),
                depends: f
                    .get(3)
                    .unwrap_or(&"")
                    .split(',')
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(String::from)
                    .collect(),
                size: f.get(4).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}

/// Paquetes instalados: (nombre, versión).
pub fn installed<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<(String, String)> {
    let data = fs.read_file(INSTALLED).unwrap_or_default();
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|l| {
            let (n, v) = l.trim().split_once(' ')?;
            Some((n.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn save_installed<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    list: &[(String, String)],
    now: jarvis_fs::Timestamp,
) -> Result<(), jarvis_fs::FsError> {
    let text: String = list.iter().map(|(n, v)| format!("{n} {v}\n")).collect();
    ensure_dirs(fs, DB, now)?;
    fs.write_file(INSTALLED, text.as_bytes(), now)
}

pub fn local_index<D: BlockDevice>(fs: &mut FileSystem<D>) -> Option<Vec<Package>> {
    fs.read_file(INDEX)
        .ok()
        .map(|b| parse_index(&String::from_utf8_lossy(&b)))
}

/// "1.10" > "1.9" (se comparan los números, no el texto).
pub fn newer(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Vec<u32> {
        s.split(['.', '-'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (x, y) = (nums(a), nums(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if p != q {
            return p > q;
        }
    }
    false
}

/// Crea todas las carpetas de `path` que falten (como `mkdir -p`).
pub fn ensure_dirs<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    path: &str,
    now: jarvis_fs::Timestamp,
) -> Result<(), jarvis_fs::FsError> {
    let mut cur = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        cur.push('/');
        cur.push_str(seg);
        if !fs.exists(&cur) {
            fs.mkdir(&cur, now)?;
        }
    }
    Ok(())
}

// --- el trabajo en curso ----------------------------------------------------------------------

enum Step {
    /// Bajando el índice. Después: instalar/actualizar lo que haya en `queue`.
    Index {
        then_install: bool,
        upgrade: bool,
    },
    Manifest,
    File,
    /// Bajando el índice de Debian. Después: instalar lo que se pidió de Debian (o terminar).
    DebIndex {
        then_install: bool,
    },
    /// Bajando un `.deb`.
    DebPackage,
}

struct Current {
    pkg: Package,
    /// (origen en el repo, destino en el disco, convertir a BMP)
    files: Vec<(String, String, bool)>,
    next: usize,
    data: Vec<(String, Vec<u8>)>,
}

pub struct AptJob {
    step: Step,
    waiting: Option<u32>,
    /// Pedidos del usuario (para resolver después de bajar el índice).
    requested: Vec<String>,
    queue: Vec<Package>,
    current: Option<Current>,
    count: u32,
    bytes: u64,
    /// Lo pedido que no está en el repositorio de JARVIS-OS: se busca en Debian.
    debian: Vec<String>,
    deb_queue: Vec<DebPkg>,
    deb_current: Option<DebPkg>,
}

impl AptJob {
    pub fn waiting_id(&self) -> Option<u32> {
        self.waiting
    }

    fn fetch<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>, path: &str, bmp: bool) {
        let url = format!("{REPO}/{path}");
        let kind = if bmp {
            FetchKind::Image
        } else {
            FetchKind::Download
        };
        self.waiting = Some(ctx.out.fetch_as(&url, kind, "apt"));
    }

    fn fetch_url<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>, url: &str) {
        self.waiting = Some(ctx.out.fetch_as(url, FetchKind::Download, "apt"));
    }

    /// Llegó una respuesta. `Some((código, salida))` cuando terminó.
    pub(crate) fn on_response<D: BlockDevice>(
        &mut self,
        _id: u32,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        self.waiting = None;
        let resp = match result {
            Ok(r) if r.status == 200 => r,
            Ok(r) => {
                out.err(&format!(
                    "E: No se pudo descargar {} (HTTP {}).",
                    r.url, r.status
                ));
                return Some((100, String::new()));
            }
            Err(e) => {
                let debian = matches!(self.step, Step::DebIndex { .. } | Step::DebPackage);
                out.err(&if debian {
                    format!("E: No se pudo conectar con {}: {e}", debian::MIRROR)
                } else {
                    format!(
                        "E: No se pudo conectar con {REPO}: {e}\n   (el repositorio lo sirve `cargo xtask run` desde el anfitrión)"
                    )
                });
                return Some((100, String::new()));
            }
        };
        self.count += 1;
        self.bytes += resp.body.len() as u64;
        let now = ctx.timestamp();
        match self.step {
            Step::Index {
                then_install,
                upgrade,
            } => {
                out.info(&format!(
                    "Des:{} {REPO} indice.txt [{}]",
                    self.count,
                    format_size(resp.body.len() as u64)
                ));
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    out.err("E: no hay disco");
                    return Some((100, String::new()));
                };
                let saved =
                    ensure_dirs(fs, DB, now).and_then(|()| fs.write_file(INDEX, &resp.body, now));
                if let Err(e) = saved {
                    out.err(&format!("E: no se pudo guardar el índice: {e}"));
                    return Some((100, String::new()));
                }
                let index = parse_index(&String::from_utf8_lossy(&resp.body));
                let have = installed(fs);
                let upgradable: Vec<&Package> = index
                    .iter()
                    .filter(|p| {
                        have.iter()
                            .any(|(n, v)| *n == p.name && newer(&p.version, v))
                    })
                    .collect();
                out.info(&format!(
                    "Descargados {} en total.\nLeyendo lista de paquetes... Hecho\n{} paquetes disponibles.",
                    format_size(self.bytes),
                    index.len()
                ));
                if !then_install {
                    out.info(&match upgradable.len() {
                        0 => "Todos los paquetes están actualizados.".to_string(),
                        n => format!("Se pueden actualizar {n} paquetes. Ejecutá «apt list --upgradable» para verlos."),
                    });
                    // Y el índice de Debian.
                    out.info(&format!(
                        "Obj:2 {} stable/main amd64 Packages",
                        debian::MIRROR
                    ));
                    self.step = Step::DebIndex {
                        then_install: false,
                    };
                    self.fetch_url(ctx, debian::PACKAGES);
                    return None;
                }
                let names: Vec<String> = if upgrade {
                    upgradable.iter().map(|p| p.name.clone()).collect()
                } else {
                    // Lo que no es de JARVIS-OS se busca en Debian, después.
                    let (ours, others): (Vec<String>, Vec<String>) =
                        core::mem::take(&mut self.requested)
                            .into_iter()
                            .partition(|n| index.iter().any(|p| p.name == *n));
                    self.debian = others;
                    ours
                };
                if names.is_empty() && !self.debian.is_empty() {
                    return self.start_debian(ctx, out);
                }
                match plan(&index, &have, &names, upgrade, out) {
                    Err(code) => Some((code, String::new())),
                    Ok(queue) if queue.is_empty() && self.debian.is_empty() => {
                        Some((0, String::new()))
                    }
                    Ok(queue) => {
                        self.queue = queue;
                        self.next_package(ctx, out)
                    }
                }
            }
            Step::Manifest => {
                let cur = self.current.as_mut()?;
                cur.files = parse_manifest(&String::from_utf8_lossy(&resp.body));
                if cur.files.is_empty() {
                    out.err(&format!(
                        "E: el paquete {} no tiene archivos.",
                        cur.pkg.name
                    ));
                    return Some((100, String::new()));
                }
                self.step = Step::File;
                let (src, _, bmp) = cur.files[0].clone();
                let name = cur.pkg.name.clone();
                self.fetch(ctx, &repo_path(&name, &src), bmp);
                None
            }
            Step::File => {
                let cur = self.current.as_mut()?;
                let (src, dest, _) = cur.files[cur.next].clone();
                out.info(&format!(
                    "Des:{} {REPO}/{} {} [{}]",
                    self.count,
                    cur.pkg.name,
                    src,
                    format_size(resp.body.len() as u64)
                ));
                cur.data.push((dest, resp.body.clone()));
                cur.next += 1;
                if cur.next < cur.files.len() {
                    let (src, _, bmp) = cur.files[cur.next].clone();
                    let name = cur.pkg.name.clone();
                    self.fetch(ctx, &repo_path(&name, &src), bmp);
                    return None;
                }
                // Todo el paquete llegó: se instala.
                let cur = self.current.take()?;
                if let Err(e) = install_files(&cur, ctx, out) {
                    out.err(&format!("E: no se pudo instalar {}: {e}", cur.pkg.name));
                    return Some((100, String::new()));
                }
                self.next_package(ctx, out)
            }
            Step::DebIndex { then_install } => {
                out.info(&format!(
                    "Des:{} {} stable/main amd64 Packages [{}]\nLeyendo lista de paquetes de Debian...",
                    self.count,
                    debian::MIRROR,
                    format_size(resp.body.len() as u64)
                ));
                let mut b = debian::IndexBuilder::default();
                if let Err(e) = debian::xz_decode(&resp.body, debian::MAX_UNPACKED, |c| b.feed(c)) {
                    out.err(&format!("E: el índice de Debian vino dañado: {e}"));
                    return Some((100, String::new()));
                }
                let (text, n) = b.finish();
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    out.err("E: no hay disco");
                    return Some((100, String::new()));
                };
                let saved = ensure_dirs(fs, DB, now)
                    .and_then(|()| fs.write_file(debian::INDEX, text.as_bytes(), now));
                if let Err(e) = saved {
                    out.err(&format!("E: no se pudo guardar el índice de Debian: {e}"));
                    return Some((100, String::new()));
                }
                ctx.log.push(format!("APT_DEBIAN_INDICE {n}"));
                out.info(&format!("{n} paquetes de Debian disponibles."));
                if !then_install {
                    return Some((0, String::new()));
                }
                let index = debian::parse_compact(&text);
                self.plan_debian(&index, ctx, out)
            }
            Step::DebPackage => {
                let pkg = self.deb_current.take()?;
                out.info(&format!(
                    "Des:{} {} {} {} [{}]",
                    self.count,
                    debian::MIRROR,
                    pkg.name,
                    pkg.version,
                    format_size(resp.body.len() as u64)
                ));
                if let Err(e) = install_deb(&pkg, &resp.body, ctx, out) {
                    out.err(&format!("E: no se pudo instalar {}: {e}", pkg.name));
                    return Some((100, String::new()));
                }
                self.next_deb(ctx, out)
            }
        }
    }

    /// Lo pedido que no es de JARVIS-OS, de Debian: con el índice guardado, o bajándolo.
    fn start_debian<D: BlockDevice>(
        &mut self,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        let saved = ctx
            .fs
            .as_deref_mut()
            .and_then(|fs| fs.read_file(debian::INDEX).ok());
        match saved {
            Some(text) => {
                let index = debian::parse_compact(&String::from_utf8_lossy(&text));
                self.plan_debian(&index, ctx, out)
            }
            None => {
                out.info(&format!(
                    "Obj:2 {} stable/main amd64 Packages (la primera vez tarda un poco)",
                    debian::MIRROR
                ));
                self.step = Step::DebIndex { then_install: true };
                self.fetch_url(ctx, debian::PACKAGES);
                None
            }
        }
    }

    fn plan_debian<D: BlockDevice>(
        &mut self,
        index: &[DebPkg],
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        let have = ctx.fs.as_deref_mut().map(installed).unwrap_or_default();
        let names = core::mem::take(&mut self.debian);
        let queue = match debian::plan(index, &have, &names) {
            Ok(q) => q,
            Err(e) => {
                out.err(&format!("E: {e}"));
                return Some((100, String::new()));
            }
        };
        if queue.is_empty() {
            for n in &names {
                out.info(&format!("{n} ya está instalado."));
            }
            return Some((0, String::new()));
        }
        let list: Vec<&str> = queue.iter().map(|p| p.name.as_str()).collect();
        let size: u64 = queue.iter().map(|p| p.size).sum();
        out.info(&format!(
            "Se instalarán los siguientes paquetes NUEVOS (de Debian):\n  {}\nSe necesita descargar {}.",
            list.join(" "),
            format_size(size)
        ));
        self.deb_queue = queue;
        self.next_deb(ctx, out)
    }

    fn next_deb<D: BlockDevice>(
        &mut self,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        if self.deb_queue.is_empty() {
            out.info(&format!("{}Listo.{}", ansi::GREEN, ansi::RESET));
            return Some((0, String::new()));
        }
        let pkg = self.deb_queue.remove(0);
        let url = format!("{}/{}", debian::MIRROR, pkg.filename);
        self.deb_current = Some(pkg);
        self.step = Step::DebPackage;
        self.fetch_url(ctx, &url);
        None
    }

    fn next_package<D: BlockDevice>(
        &mut self,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        if self.queue.is_empty() {
            if !self.debian.is_empty() {
                return self.start_debian(ctx, out);
            }
            out.info(&format!("{}Listo.{}", ansi::GREEN, ansi::RESET));
            return Some((0, String::new()));
        }
        let pkg = self.queue.remove(0);
        out.info(&format!("Preparando {} ({})...", pkg.name, pkg.version));
        let path = format!("{}/manifiesto.txt", pkg.name);
        self.current = Some(Current {
            pkg,
            files: Vec::new(),
            next: 0,
            data: Vec::new(),
        });
        self.step = Step::Manifest;
        self.fetch(ctx, &path, false);
        None
    }
}

/// Dónde está en el repositorio un archivo del paquete `pkg`.
fn repo_path(pkg: &str, src: &str) -> String {
    match src.strip_prefix("usuario:") {
        Some(program) => format!("usuario/{program}"),
        None => format!("{pkg}/{src}"),
    }
}

fn parse_manifest(text: &str) -> Vec<(String, String, bool)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (src, rest) = l.split_once("->")?;
            let rest = rest.trim();
            let (dest, bmp) = match rest.strip_suffix("[bmp]") {
                Some(d) => (d.trim(), true),
                None => (rest, false),
            };
            let src = src.trim();
            // Nada de salirse del repositorio ni rutas relativas en el disco.
            if src.contains("..") || !dest.starts_with('/') || dest.contains("/../") {
                return None;
            }
            Some((src.to_string(), dest.to_string(), bmp))
        })
        .collect()
}

fn install_files<D: BlockDevice>(
    cur: &Current,
    ctx: &mut Ctx<'_, D>,
    out: &mut Out,
) -> Result<(), String> {
    let now = ctx.timestamp();
    let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
    out.info(&format!(
        "Desempaquetando {} ({})...",
        cur.pkg.name, cur.pkg.version
    ));
    let mut list = String::new();
    for (dest, data) in &cur.data {
        ensure_dirs(fs, &parent(dest), now).map_err(|e| e.to_string())?;
        fs.write_file(dest, data, now)
            .map_err(|e| format!("{dest}: {e}"))?;
        list.push_str(dest);
        list.push('\n');
    }
    ensure_dirs(fs, DB, now).map_err(|e| e.to_string())?;
    fs.write_file(
        &format!("{DB}/{}.lista", cur.pkg.name),
        list.as_bytes(),
        now,
    )
    .map_err(|e| e.to_string())?;
    let mut have = installed(fs);
    have.retain(|(n, _)| *n != cur.pkg.name);
    have.push((cur.pkg.name.clone(), cur.pkg.version.clone()));
    save_installed(fs, &have, now).map_err(|e| e.to_string())?;
    out.info(&format!(
        "Configurando {} ({})...",
        cur.pkg.name, cur.pkg.version
    ));
    ctx.log.push(format!(
        "APT_INSTALADO {} {}",
        cur.pkg.name, cur.pkg.version
    ));
    Ok(())
}

/// Desarma un `.deb` en el disco y lo anota como instalado.
fn install_deb<D: BlockDevice>(
    pkg: &DebPkg,
    deb: &[u8],
    ctx: &mut Ctx<'_, D>,
    out: &mut Out,
) -> Result<(), String> {
    // El antivirus revisa el paquete antes de desarmarlo.
    if let Some(fs) = ctx.fs.as_deref_mut() {
        ctx.antivirus.load(fs);
        if ctx.antivirus.enabled
            && let Some(t) = ctx.antivirus.scan(deb)
        {
            ctx.log.push(format!("ANTIVIRUS_BLOQUEO {}", pkg.name));
            return Err(format!("el antivirus lo bloqueó: {}", t.name()));
        }
    }
    out.info(&format!(
        "Desempaquetando {} ({})...",
        pkg.name, pkg.version
    ));
    let u = debian::unpack(deb)?;
    let now = ctx.timestamp();
    let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
    let mut list = String::new();
    for d in &u.dirs {
        ensure_dirs(fs, d, now).map_err(|e| format!("{d}: {e}"))?;
    }
    for (path, data) in &u.files {
        ensure_dirs(fs, &parent(path), now).map_err(|e| e.to_string())?;
        fs.write_file(path, data, now)
            .map_err(|e| format!("{path}: {e}"))?;
        list.push_str(path);
        list.push('\n');
    }
    // Enlaces a archivos de otro paquete (ya instalado): una copia, si está.
    for (path, target) in &u.links_out {
        if let Ok(data) = fs.read_file(target) {
            ensure_dirs(fs, &parent(path), now).map_err(|e| e.to_string())?;
            fs.write_file(path, &data, now)
                .map_err(|e| format!("{path}: {e}"))?;
            list.push_str(path);
            list.push('\n');
        }
    }
    ensure_dirs(fs, DB, now).map_err(|e| e.to_string())?;
    fs.write_file(&format!("{DB}/{}.lista", pkg.name), list.as_bytes(), now)
        .map_err(|e| e.to_string())?;
    let mut have = installed(fs);
    have.retain(|(n, _)| *n != pkg.name);
    have.push((pkg.name.clone(), pkg.version.clone()));
    save_installed(fs, &have, now).map_err(|e| e.to_string())?;
    out.info(&format!("Configurando {} ({})...", pkg.name, pkg.version));
    ctx.log
        .push(format!("APT_INSTALADO {} {}", pkg.name, pkg.version));
    Ok(())
}

/// El índice de Debian guardado (lo arma `apt update`).
fn debian_index<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<DebPkg> {
    fs.read_file(debian::INDEX)
        .map(|b| debian::parse_compact(&String::from_utf8_lossy(&b)))
        .unwrap_or_default()
}

/// Qué hay que bajar para instalar `names` (con sus dependencias, en orden).
fn plan(
    index: &[Package],
    have: &[(String, String)],
    names: &[String],
    upgrade: bool,
    out: &mut Out,
) -> Result<Vec<Package>, i32> {
    out.info("Leyendo lista de paquetes... Hecho\nCreando árbol de dependencias... Hecho");
    let mut queue: Vec<Package> = Vec::new();
    fn add(
        index: &[Package],
        have: &[(String, String)],
        name: &str,
        explicit: bool,
        queue: &mut Vec<Package>,
        out: &mut Out,
        depth: u32,
    ) -> Result<(), i32> {
        if depth > 16 || queue.iter().any(|p| p.name == name) {
            return Ok(());
        }
        let Some(p) = index.iter().find(|p| p.name == name) else {
            out.err(&format!("E: No se ha podido localizar el paquete {name}"));
            return Err(100);
        };
        for d in &p.depends {
            add(index, have, d, false, queue, out, depth + 1)?;
        }
        match have.iter().find(|(n, _)| n == name) {
            Some((_, v)) if !newer(&p.version, v) => {
                if explicit {
                    out.info(&format!("{name} ya está en su versión más reciente ({v})."));
                }
            }
            _ => queue.push(p.clone()),
        }
        Ok(())
    }
    for n in names {
        add(index, have, n, !upgrade, &mut queue, out, 0)?;
    }
    let (fresh, upgrades): (Vec<&Package>, Vec<&Package>) = queue
        .iter()
        .partition(|p| !have.iter().any(|(n, _)| *n == p.name));
    if !fresh.is_empty() {
        let names: Vec<&str> = fresh.iter().map(|p| p.name.as_str()).collect();
        out.info(&format!(
            "Se instalarán los siguientes paquetes NUEVOS:\n  {}",
            names.join(" ")
        ));
    }
    if !upgrades.is_empty() {
        let names: Vec<&str> = upgrades.iter().map(|p| p.name.as_str()).collect();
        out.info(&format!(
            "Se actualizarán los siguientes paquetes:\n  {}",
            names.join(" ")
        ));
    }
    out.info(&format!(
        "{} actualizados, {} nuevos se instalarán, 0 para eliminar.",
        upgrades.len(),
        fresh.len()
    ));
    Ok(queue)
}

const HELP: &str = "apt 1.0 (JARVIS-OS)
Uso: apt [orden] [paquetes]

Órdenes:
  update          baja la lista de paquetes del repositorio
  upgrade         actualiza los paquetes instalados
  install PAQ...  instala paquetes (y lo que necesiten)
  remove PAQ...   desinstala (los archivos van a la Papelera)
  list            lista los paquetes (--installed, --upgradable)
  search TEXTO    busca en los nombres y descripciones
  show PAQ        muestra los datos de un paquete

Repositorio: http://paquetes.jarvis (lo sirve `cargo xtask run`).";

/// `apt …` (y `dpkg -l`). `Ok` = terminó; `Err(job)` = tiene que bajar algo.
pub(crate) fn run<D: BlockDevice>(
    args: &[String],
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
    out: &mut Out,
) -> Result<i32, Box<AptJob>> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args
        .iter()
        .skip(2)
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();
    let flags: Vec<&str> = args
        .iter()
        .skip(2)
        .map(String::as_str)
        .filter(|a| a.starts_with('-'))
        .collect();
    let new_job = |step: Step, requested: Vec<String>| AptJob {
        step,
        waiting: None,
        requested,
        queue: Vec::new(),
        current: None,
        count: 0,
        bytes: 0,
        debian: Vec::new(),
        deb_queue: Vec::new(),
        deb_current: None,
    };
    let Some(fs) = ctx.fs.as_deref_mut() else {
        e.push_str("E: no hay disco\n");
        return Ok(100);
    };
    match cmd {
        "" | "help" | "--help" | "-h" => {
            o.push_str(HELP);
            o.push('\n');
            Ok(0)
        }
        "update" => {
            let mut job = new_job(
                Step::Index {
                    then_install: false,
                    upgrade: false,
                },
                Vec::new(),
            );
            out.info(&format!("Obj:1 {REPO} JARVIS-OS InRelease"));
            job.fetch(ctx, "indice.txt", false);
            Err(Box::new(job))
        }
        "install" | "reinstall" | "upgrade" | "full-upgrade" => {
            let upgrade = cmd.ends_with("upgrade");
            if !upgrade && rest.is_empty() {
                e.push_str("E: Decime qué paquete instalar. Ejemplo: apt install neofetch\n");
                return Ok(100);
            }
            let mut job = new_job(
                Step::Index {
                    then_install: true,
                    upgrade,
                },
                rest,
            );
            // Siempre se baja el índice primero: así nunca se instala algo viejo.
            job.fetch(ctx, "indice.txt", false);
            Err(Box::new(job))
        }
        "remove" | "purge" | "uninstall" => {
            if rest.is_empty() {
                e.push_str("E: Decime qué paquete desinstalar.\n");
                return Ok(100);
            }
            let now = crate::apps::timestamp(ctx.clock);
            let mut have = installed(fs);
            let mut code = 0;
            for name in &rest {
                if !have.iter().any(|(n, _)| n == name) {
                    e.push_str(&format!(
                        "El paquete «{name}» no está instalado, no se eliminará\n"
                    ));
                    code = 100;
                    continue;
                }
                o.push_str(&format!("Desinstalando {name}...\n"));
                let list = fs
                    .read_file(&format!("{DB}/{name}.lista"))
                    .unwrap_or_default();
                for path in String::from_utf8_lossy(&list)
                    .lines()
                    .filter(|l| !l.is_empty())
                {
                    if fs.exists(path)
                        && let Err(err) = FilesApp::move_to_trash(fs, path, now)
                    {
                        e.push_str(&format!("W: {path}: {err}\n"));
                    }
                }
                have.retain(|(n, _)| n != name);
                ctx.log.push(format!("APT_DESINSTALADO {name}"));
            }
            if let Err(err) = save_installed(fs, &have, now) {
                e.push_str(&format!("E: {err}\n"));
                return Ok(100);
            }
            o.push_str("Los archivos quedaron en la Papelera.\n");
            Ok(code)
        }
        "list" => {
            let have = installed(fs);
            let Some(index) = local_index(fs) else {
                e.push_str("E: No hay lista de paquetes. Ejecutá primero: apt update\n");
                return Ok(100);
            };
            o.push_str("Listando... Hecho\n");
            let only_installed = flags.contains(&"--installed");
            let only_upgradable = flags.contains(&"--upgradable");
            for p in &index {
                let inst = have.iter().find(|(n, _)| *n == p.name);
                let upgradable = inst.is_some_and(|(_, v)| newer(&p.version, v));
                if (only_installed && inst.is_none()) || (only_upgradable && !upgradable) {
                    continue;
                }
                let tag = match inst {
                    Some((_, v)) if upgradable => format!(" [instalado: {v}, actualizable]"),
                    Some(_) => " [instalado]".into(),
                    None => String::new(),
                };
                o.push_str(&format!(
                    "{}{}{}/jarvis {} all{}\n  {}\n",
                    ansi::GREEN,
                    p.name,
                    ansi::RESET,
                    p.version,
                    tag,
                    p.description
                ));
            }
            // Los de Debian instalados (el índice de Debian entero no se lista: son miles).
            if !only_upgradable {
                for (n, v) in have
                    .iter()
                    .filter(|(n, _)| !index.iter().any(|p| p.name == *n))
                {
                    o.push_str(&format!(
                        "{}{n}{}/debian {v} amd64 [instalado]\n",
                        ansi::GREEN,
                        ansi::RESET
                    ));
                }
            }
            Ok(0)
        }
        "search" => {
            let Some(index) = local_index(fs) else {
                e.push_str("E: No hay lista de paquetes. Ejecutá primero: apt update\n");
                return Ok(100);
            };
            let q = rest.join(" ").to_lowercase();
            for p in index.iter().filter(|p| {
                p.name.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q)
            }) {
                o.push_str(&format!(
                    "{}{}{} {}\n  {}\n",
                    ansi::GREEN,
                    p.name,
                    ansi::RESET,
                    p.version,
                    p.description
                ));
            }
            // Y en Debian (los primeros 50, por nombre primero).
            let deb = debian_index(fs);
            let mut hits: Vec<&DebPkg> = deb
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&q))
                .collect();
            if hits.len() < 50 {
                hits.extend(deb.iter().filter(|p| {
                    !p.name.to_lowercase().contains(&q) && p.description.to_lowercase().contains(&q)
                }));
            }
            let total = hits.len();
            for p in hits.into_iter().take(50) {
                o.push_str(&format!(
                    "{}{}{}/debian {}\n  {}\n",
                    ansi::GREEN,
                    p.name,
                    ansi::RESET,
                    p.version,
                    p.description
                ));
            }
            if total > 50 {
                o.push_str(&format!("... y {} más de Debian.\n", total - 50));
            }
            Ok(0)
        }
        "show" => {
            let Some(index) = local_index(fs) else {
                e.push_str("E: No hay lista de paquetes. Ejecutá primero: apt update\n");
                return Ok(100);
            };
            let Some(p) = rest
                .first()
                .and_then(|n| index.iter().find(|p| p.name == *n))
            else {
                let deb = debian_index(fs);
                let Some(p) = rest.first().and_then(|n| deb.iter().find(|p| p.name == *n)) else {
                    e.push_str("E: No se encontró el paquete\n");
                    return Ok(100);
                };
                o.push_str(&format!(
                    "Package: {}\nVersion: {}\nDepends: {}\nDownload-Size: {}\nAPT-Sources: {} stable/main\nDescription: {}\n",
                    p.name,
                    p.version,
                    if p.depends.is_empty() { "-".into() } else { p.depends.join(", ") },
                    format_size(p.size),
                    debian::MIRROR,
                    p.description
                ));
                return Ok(0);
            };
            o.push_str(&format!(
                "Package: {}\nVersion: {}\nDepends: {}\nDownload-Size: {}\nAPT-Sources: {REPO}\nDescription: {}\n",
                p.name,
                p.version,
                if p.depends.is_empty() { "-".into() } else { p.depends.join(", ") },
                p.size,
                p.description
            ));
            Ok(0)
        }
        other => {
            e.push_str(&format!("E: Orden no válida: {other} (probá «apt help»)\n"));
            Ok(100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indice_versiones_y_manifiesto() {
        let idx =
            parse_index("# comentario\nneofetch|1.2|Datos del sistema|figlet, x|3 KB\n\nsolo\n");
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].depends, ["figlet", "x"]);
        assert_eq!(idx[1].version, "0");
        assert!(newer("1.10", "1.9") && !newer("1.0", "1.0") && newer("2", "1.9.9"));
        let m = parse_manifest(
            "neofetch.sh -> /Programas/bin/neofetch\naurora.png -> /Imágenes/Fondos/aurora.bmp [bmp]\n../x -> /y\nz -> relativo",
        );
        assert_eq!(
            m,
            [
                (
                    "neofetch.sh".into(),
                    "/Programas/bin/neofetch".into(),
                    false
                ),
                (
                    "aurora.png".into(),
                    "/Imágenes/Fondos/aurora.bmp".into(),
                    true
                )
            ]
        );
    }
}
