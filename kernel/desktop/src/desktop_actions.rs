//! Las acciones que pide el cerebro (ADR 0008): el escritorio las ejecuta y devuelve un texto
//! para Claude. Los permisos ya se resolvieron del otro lado (las de nivel 2 y 3 llegan después de
//! que Roman las aprobó en el diálogo de confirmación).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem, Timestamp};

use super::Desktop;
use crate::apps::{app_by_name, name_of, timestamp};
use crate::files::{FilesApp, basename, format_size, join};
use crate::system::Launch;

/// Lo más que se le devuelve a Claude de un archivo.
const MAX_READ: usize = 64 * 1024;
const MAX_FOUND: usize = 50;
/// Cuánto de la terminal se le devuelve a Claude (lo último).
const MAX_TERM: usize = 4 * 1024;

/// Las últimas `max` letras de `text` (sin cortar un carácter por la mitad).
fn tail(text: &str, max: usize) -> &str {
    let text = text.trim_end();
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Una ruta absoluta y sin `..` (las de Claude pueden venir sin la barra inicial).
fn clean(path: &str) -> Option<String> {
    let p = path.trim();
    if p.is_empty() || p.split('/').any(|s| s == "..") {
        return None;
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    Some(if p.len() > 1 {
        p.trim_end_matches('/').to_string()
    } else {
        p
    })
}

fn mkdir_all<D: BlockDevice>(fs: &mut FileSystem<D>, dir: &str, now: Timestamp) -> bool {
    let mut cur = String::new();
    for part in dir.split('/').filter(|s| !s.is_empty()) {
        cur.push('/');
        cur.push_str(part);
        if !fs.exists(&cur) && fs.mkdir(&cur, now).is_err() {
            return false;
        }
    }
    true
}

impl<D: BlockDevice> Desktop<D> {
    pub(super) fn run_action(
        &mut self,
        tool: &str,
        args: &[(String, String)],
        now_ms: u64,
    ) -> (bool, String) {
        let arg = |k: &str| {
            args.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let clock = self.last_clock;
        let now = timestamp(clock);
        let bad_path = |p: &str| (false, format!("Ruta inválida: {p}"));
        match tool {
            "estado_sistema" => {
                let s = &self.stats;
                (
                    true,
                    format!(
                        "CPU {} % ({}), memoria {} de {} MiB (RAM física libre {} MiB, {} tablas de páginas), {} ventanas abiertas, red: {}",
                        s.cpu_pct,
                        s.cpu_name,
                        s.heap_used >> 20,
                        s.heap_total >> 20,
                        s.ram_free >> 20,
                        s.page_tables,
                        self.slots.len(),
                        if s.net.ip.is_some() {
                            "conectada"
                        } else {
                            "sin conexión"
                        },
                    ),
                )
            }
            "ventanas_abiertas" => {
                let list: Vec<String> = self
                    .tasks()
                    .iter()
                    .map(|t| {
                        format!(
                            "{} ({}){}",
                            t.title,
                            name_of(t.kind),
                            if t.minimized { ", minimizada" } else { "" }
                        )
                    })
                    .collect();
                if list.is_empty() {
                    (true, "No hay ventanas abiertas.".into())
                } else {
                    (true, list.join("\n"))
                }
            }
            "abrir_app" => match app_by_name(&arg("app")) {
                Some(k) => {
                    self.open(Launch::App(k), now_ms, clock);
                    (true, format!("Abrí {}.", name_of(k)))
                }
                None => (false, format!("No conozco la app \"{}\".", arg("app"))),
            },
            "abrir_web" => {
                let url = arg("url");
                self.open(Launch::Browse(url.clone()), now_ms, clock);
                (true, format!("Abrí {url} en el navegador."))
            }
            "buscar_web" => {
                let q = arg("consulta");
                self.open(Launch::Browse(format!("? {q}")), now_ms, clock);
                (true, format!("Busqué \"{q}\" en el navegador."))
            }
            "cerrar_ventana" => {
                let Some(k) = app_by_name(&arg("app")) else {
                    return (false, format!("No conozco la app \"{}\".", arg("app")));
                };
                match self.window_of(k) {
                    Some(id) => {
                        self.close_window(id, now_ms, clock);
                        (true, format!("Cerré {}.", name_of(k)))
                    }
                    None => (false, format!("{} no está abierta.", name_of(k))),
                }
            }
            "ejecutar_comando" => {
                let cmd = arg("comando");
                self.open(Launch::Terminal(Some(cmd.clone())), now_ms, clock);
                // Los comandos locales terminan al momento: la salida va en la respuesta. Los que
                // esperan la red (apt, wget...) siguen: Claude la lee después con leer_terminal.
                match self.terminal_text() {
                    Some((text, false)) => (
                        true,
                        format!(
                            "Ejecuté: {cmd}
Lo último de la terminal:
{text}"
                        ),
                    ),
                    Some((text, true)) => (
                        true,
                        format!(
                            "Ejecuté: {cmd}
Todavía está corriendo (usá leer_terminal para ver cómo termina). Por ahora:
{text}"
                        ),
                    ),
                    None => (true, format!("Lo mandé a la terminal: {cmd}")),
                }
            }
            "leer_terminal" => match self.terminal_text() {
                Some((text, running)) => (
                    true,
                    format!(
                        "{}
{text}",
                        if running {
                            "(el comando sigue corriendo)"
                        } else {
                            "(la terminal espera otro comando)"
                        }
                    ),
                ),
                None => (false, "No hay ninguna terminal abierta.".into()),
            },
            "abrir_archivo" => self.open_file(&arg("ruta"), now_ms, clock),
            _ => self.file_action(tool, &arg, now, bad_path),
        }
    }

    /// Lo último que muestra la terminal, y si el comando sigue corriendo.
    fn terminal_text(&self) -> Option<(String, bool)> {
        self.slots.iter().rev().find_map(|s| match &s.app {
            crate::apps::App::Terminal(t) => {
                Some((tail(&t.text(), MAX_TERM).to_string(), t.shell.waiting()))
            }
            _ => None,
        })
    }

    /// Abre un archivo con su app (como el doble clic en Archivos): carpetas en Archivos,
    /// imágenes en el visor, páginas en el navegador y el resto en el editor.
    fn open_file(
        &mut self,
        path: &str,
        now_ms: u64,
        clock: Option<jarvis_gfx::clock::DateTime>,
    ) -> (bool, String) {
        let Some(p) = clean(path) else {
            return (false, format!("Ruta inválida: {path}"));
        };
        let Some(fs) = self.fs.as_mut() else {
            return (false, "JARVIS-OS no tiene disco.".into());
        };
        let st = match fs.stat(&p) {
            Ok(st) => st,
            Err(e) => return (false, format!("No pude abrir {p}: {e}")),
        };
        // Con la app elegida en Configuración → Aplicaciones predeterminadas.
        let is_dir = st.is_dir || p == "/";
        let (launch, app) = crate::defaults::launch_for(&self.config.default_apps, &p, is_dir);
        self.open(launch, now_ms, clock);
        (true, format!("Abrí {p} en {app}."))
    }

    fn file_action(
        &mut self,
        tool: &str,
        arg: &dyn Fn(&str) -> String,
        now: Timestamp,
        bad_path: impl Fn(&str) -> (bool, String),
    ) -> (bool, String) {
        let Some(fs) = self.fs.as_mut() else {
            return (false, "JARVIS-OS no tiene disco.".into());
        };
        let err = |e: jarvis_fs::FsError| (false, format!("No se pudo: {e}"));
        match tool {
            "listar_archivos" => {
                let Some(p) = clean(&arg("ruta")) else {
                    return bad_path(&arg("ruta"));
                };
                match fs.list(&p) {
                    Ok(list) => {
                        let lines: Vec<String> = list
                            .iter()
                            .filter(|e| e.name != "." && e.name != "..")
                            .map(|e| {
                                if e.is_dir {
                                    format!("{}/", e.name)
                                } else {
                                    format!("{} ({})", e.name, format_size(e.size as u64))
                                }
                            })
                            .collect();
                        if lines.is_empty() {
                            (true, format!("{p} está vacía."))
                        } else {
                            (true, lines.join("\n"))
                        }
                    }
                    Err(e) => err(e),
                }
            }
            "leer_archivo" => {
                let Some(p) = clean(&arg("ruta")) else {
                    return bad_path(&arg("ruta"));
                };
                match fs.read_prefix(&p, MAX_READ) {
                    Ok(d) => (true, String::from_utf8_lossy(&d).into_owned()),
                    Err(e) => err(e),
                }
            }
            "buscar_archivos" => {
                let needle = arg("nombre").to_lowercase();
                if needle.is_empty() {
                    return (false, "¿Qué nombre busco?".into());
                }
                let mut found = Vec::new();
                let mut dirs = Vec::from([String::from("/")]);
                let mut visited = 0;
                while let Some(d) = dirs.pop() {
                    visited += 1;
                    if visited > 2000 || found.len() >= MAX_FOUND {
                        break;
                    }
                    let Ok(list) = fs.list(&d) else { continue };
                    for e in list {
                        if e.name == "." || e.name == ".." {
                            continue;
                        }
                        let p = join(&d, &e.name);
                        if e.name.to_lowercase().contains(&needle) {
                            found.push(p.clone());
                        }
                        if e.is_dir {
                            dirs.push(p);
                        }
                    }
                }
                if found.is_empty() {
                    (true, format!("No encontré nada con \"{needle}\"."))
                } else {
                    (true, found.join("\n"))
                }
            }
            "escribir_archivo" => {
                let Some(p) = clean(&arg("ruta")) else {
                    return bad_path(&arg("ruta"));
                };
                let dir = crate::files::parent(&p);
                if !mkdir_all(fs, &dir, now) {
                    return (false, format!("No pude crear {dir}."));
                }
                match fs.write_file(&p, arg("contenido").as_bytes(), now) {
                    Ok(()) => (true, format!("Escribí {p}.")),
                    Err(e) => err(e),
                }
            }
            "crear_carpeta" => {
                let Some(p) = clean(&arg("ruta")) else {
                    return bad_path(&arg("ruta"));
                };
                if mkdir_all(fs, &p, now) {
                    (true, format!("Creé {p}."))
                } else {
                    (false, format!("No pude crear {p}."))
                }
            }
            "copiar" | "mover" => {
                let (Some(src), Some(dst)) = (clean(&arg("origen")), clean(&arg("destino"))) else {
                    return bad_path(&arg("origen"));
                };
                if tool == "mover" {
                    return match fs.move_to(&src, &dst) {
                        Ok(()) => (true, format!("Moví {src} a {dst}.")),
                        Err(e) => err(e),
                    };
                }
                let data = match fs.read_file(&src) {
                    Ok(d) => d,
                    Err(e) => return err(e),
                };
                let target = join(&dst, basename(&src));
                match fs.write_file(&target, &data, now) {
                    Ok(()) => (true, format!("Copié {src} a {target}.")),
                    Err(e) => err(e),
                }
            }
            "a_papelera" => {
                let Some(p) = clean(&arg("ruta")) else {
                    return bad_path(&arg("ruta"));
                };
                match FilesApp::move_to_trash(fs, &p, now) {
                    Ok(to) => (true, format!("Moví {p} a la Papelera ({to}).")),
                    Err(e) => err(e),
                }
            }
            other => (false, format!("JARVIS-OS no conoce la acción \"{other}\".")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn rutas() {
        assert_eq!(
            clean("Documentos/a.txt").as_deref(),
            Some("/Documentos/a.txt")
        );
        assert_eq!(clean("/Documentos/").as_deref(), Some("/Documentos"));
        assert_eq!(clean("/"), Some("/".into()));
        assert_eq!(clean("/a/../Sistema"), None);
        assert_eq!(clean("  "), None);
    }
}
