//! `lspci`, `lsusb` y `controladores` (ADR 0023): el inventario del hardware y el gestor de
//! controladores (ver [`crate::drivers`]).
//!
//! ```text
//! controladores                  qué hay, qué driver lo atiende y qué falta
//! sudo controladores instalar    baja el firmware que falta (linux-firmware, con SHA-256)
//! controladores reporte          /Documentos/hardware.txt con lo que no tiene driver
//! sudo controladores actualizar  baja el catálogo nuevo
//! ```

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;

use super::Out;
use super::apt::ensure_dirs;
use crate::apps::Ctx;
use crate::drivers::{self, Bus, Catalog, Firmware, State};
use crate::system::{FetchKind, HttpResponse};

/// El catálogo: el bajado si está y se entiende; si no, el que viene adentro.
pub fn catalog<D: BlockDevice>(ctx: &mut Ctx<'_, D>) -> Catalog {
    let downloaded = ctx
        .fs
        .as_deref_mut()
        .and_then(|fs| fs.read_file(drivers::CATALOG).ok())
        .map(|b| drivers::parse_catalog(&String::from_utf8_lossy(&b)))
        .filter(|c| !c.entries.is_empty());
    downloaded.unwrap_or_else(|| drivers::parse_catalog(drivers::BUILTIN_CATALOG))
}

fn rows<D: BlockDevice>(ctx: &mut Ctx<'_, D>) -> Vec<drivers::Row> {
    let cat = catalog(ctx);
    let hw = ctx.stats.hardware.clone();
    let fs = ctx.fs.as_deref_mut();
    match fs {
        Some(fs) => drivers::review(&hw, &cat, &mut |p| fs.exists(p)),
        None => drivers::review(&hw, &cat, &mut |_| false),
    }
}

pub(crate) fn lspci<D: BlockDevice>(ctx: &mut Ctx<'_, D>, o: &mut String) -> i32 {
    let cat = catalog(ctx);
    for d in ctx.stats.hardware.iter().filter(|d| d.bus == Bus::Pci) {
        let name = cat.find(d).map_or_else(
            || {
                let v = drivers::vendor_name(d.vendor);
                format!("{} {}", drivers::class_name(d.bus, d.class, d.subclass), v)
            },
            |e| e.name.clone(),
        );
        o.push_str(&format!(
            "{} {}: {} [{:04x}:{:04x}]\n",
            d.location,
            drivers::class_name(d.bus, d.class, d.subclass),
            name.trim(),
            d.vendor,
            d.device
        ));
    }
    0
}

pub(crate) fn lsusb<D: BlockDevice>(ctx: &mut Ctx<'_, D>, o: &mut String) -> i32 {
    let usb: Vec<_> = ctx
        .stats
        .hardware
        .iter()
        .filter(|d| d.bus == Bus::Usb)
        .collect();
    if usb.is_empty() {
        o.push_str("No hay dispositivos USB (o no hay controladora xHCI).\n");
    }
    for d in usb {
        let v = drivers::vendor_name(d.vendor);
        o.push_str(&format!(
            "Bus 001 {}: ID {:04x}:{:04x} {v} ({})\n",
            d.location,
            d.vendor,
            d.device,
            drivers::class_name(Bus::Usb, d.class, d.subclass)
        ));
    }
    0
}

/// Bajando firmware o el catálogo.
pub(crate) struct DriversJob {
    pub id: u32,
    /// El firmware que falta (o vacío si se baja el catálogo).
    pub queue: Vec<Firmware>,
    pub next: usize,
}

impl DriversJob {
    pub fn on_response<D: BlockDevice>(
        &mut self,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(i32, String)> {
        let now = ctx.timestamp();
        let body = match result {
            Ok(r) if r.status == 200 => r.body.clone(),
            Ok(r) => {
                out.err(&format!(
                    "controladores: el servidor respondió HTTP {}",
                    r.status
                ));
                return Some((1, String::new()));
            }
            Err(e) => {
                out.err(&format!("controladores: no se pudo bajar: {e}"));
                return Some((1, String::new()));
            }
        };
        let fs = ctx.fs.as_deref_mut()?;
        if self.queue.is_empty() {
            // El catálogo: solo si se entiende.
            let cat = drivers::parse_catalog(&String::from_utf8_lossy(&body));
            if cat.entries.is_empty() {
                out.err("controladores: el catálogo nuevo está vacío o roto: queda el que había");
                return Some((1, String::new()));
            }
            let saved = ensure_dirs(fs, "/Sistema/controladores", now)
                .and_then(|()| fs.write_file(drivers::CATALOG, &body, now));
            if let Err(e) = saved {
                out.err(&format!(
                    "controladores: no se pudo guardar el catálogo: {e}"
                ));
                return Some((1, String::new()));
            }
            ctx.log
                .push(format!("CONTROLADORES_CATALOGO {}", cat.entries.len()));
            out.info(&format!(
                "Catálogo al día: {} dispositivos y {} firmwares conocidos.",
                cat.entries.len(),
                cat.firmware.len()
            ));
            return Some((0, String::new()));
        }
        let fw = &self.queue[self.next];
        let got = crate::antivirus::hex(&crate::antivirus::sha256(&body));
        if got != fw.sha256 {
            out.err(&format!(
                "controladores: {} no coincide con el SHA-256 del catálogo: no se instaló",
                fw.path
            ));
            ctx.log.push(format!("CONTROLADORES_HASH_MAL {}", fw.path));
            return Some((1, String::new()));
        }
        let dir = crate::files::parent(&fw.path);
        let saved = ensure_dirs(fs, &dir, now).and_then(|()| fs.write_file(&fw.path, &body, now));
        if let Err(e) = saved {
            out.err(&format!(
                "controladores: no se pudo guardar {}: {e}",
                fw.path
            ));
            return Some((1, String::new()));
        }
        ctx.log.push(format!("CONTROLADORES_FIRMWARE {}", fw.path));
        out.info(&format!(
            "Firmware de {} instalado en {} ({} KiB, SHA-256 verificado).",
            fw.driver,
            fw.path,
            body.len() / 1024
        ));
        self.next += 1;
        if let Some(next) = self.queue.get(self.next) {
            out.info(&format!("Bajando {}...", next.path));
            self.id = ctx.out.fetch_as(&next.url, FetchKind::Download, "sistema");
            return None;
        }
        out.info("Listo. El firmware se usa desde el próximo arranque (reboot).");
        Some((0, String::new()))
    }
}

/// `controladores …`. `Err(job)` si quedó bajando algo.
pub(crate) fn run<D: BlockDevice>(
    args: &[String],
    root: bool,
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
) -> Result<i32, DriversJob> {
    match args.get(1).map(String::as_str).unwrap_or("escanear") {
        "escanear" | "scan" | "lista" => {
            let rows = rows(ctx);
            if rows.is_empty() {
                o.push_str("El kernel todavía no pasó el inventario del hardware.\n");
                return Ok(0);
            }
            o.push_str(&drivers::table(&rows));
            let missing = rows.iter().filter(|r| r.state == State::Missing).count();
            let fw = rows
                .iter()
                .filter(|r| matches!(r.state, State::NeedsFirmware(..)))
                .count();
            o.push_str(&format!(
                "\n{} dispositivos: {missing} sin driver, {fw} con firmware para instalar.\n",
                rows.len()
            ));
            if missing > 0 {
                o.push_str("Para pedir los que faltan: controladores reporte\n");
            }
            ctx.log
                .push(format!("CONTROLADORES {} {missing} {fw}", rows.len()));
            Ok(0)
        }
        "reporte" | "report" => {
            let rows = rows(ctx);
            let text = drivers::report(&rows, &ctx.config.hostname);
            let now = ctx.timestamp();
            let Some(fs) = ctx.fs.as_deref_mut() else {
                return Ok(1);
            };
            let path = "/Documentos/hardware.txt";
            match ensure_dirs(fs, "/Documentos", now)
                .and_then(|()| fs.write_file(path, text.as_bytes(), now))
            {
                Ok(()) => {
                    o.push_str(&format!("Reporte guardado en {path}.\n"));
                    Ok(0)
                }
                Err(err) => {
                    e.push_str(&format!("controladores: {path}: {err}\n"));
                    Ok(1)
                }
            }
        }
        "instalar" | "install" | "actualizar" | "update" if !root => {
            e.push_str("controladores: hace falta ser root (probá con sudo)\n");
            Ok(1)
        }
        "instalar" | "install" => {
            let mut queue: Vec<Firmware> = Vec::new();
            for r in rows(ctx) {
                if let State::NeedsFirmware(_, fw) = r.state {
                    for f in fw {
                        if !queue.iter().any(|q| q.path == f.path) {
                            queue.push(f);
                        }
                    }
                }
            }
            if queue.is_empty() {
                o.push_str("No falta ningún firmware.\n");
                return Ok(0);
            }
            o.push_str(&format!("Bajando {} de linux-firmware...\n", queue[0].path));
            let id = ctx
                .out
                .fetch_as(&queue[0].url, FetchKind::Download, "sistema");
            Err(DriversJob { id, queue, next: 0 })
        }
        "actualizar" | "update" => {
            o.push_str("Bajando el catálogo de controladores...\n");
            let id = ctx
                .out
                .fetch_as(drivers::CATALOG_URL, FetchKind::Download, "sistema");
            Err(DriversJob {
                id,
                queue: Vec::new(),
                next: 0,
            })
        }
        other => {
            e.push_str(&format!(
                "controladores: «{other}» no es una orden (escanear, instalar, reporte, actualizar)\n"
            ));
            let _ = other.to_string();
            Ok(1)
        }
    }
}
