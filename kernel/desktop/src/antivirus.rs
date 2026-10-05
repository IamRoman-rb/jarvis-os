//! El antivirus de JARVIS-OS: revisa lo que entra al disco y lo que se va a ejecutar.
//!
//! - **Firmas**: los SHA-256 de malware conocido que publica MalwareBazaar (abuse.ch, de uso
//!   libre). `antivirus actualizar` (o Configuración → Privacidad y seguridad) baja los de las
//!   últimas 48 horas y los **suma** a la base local ([`DB`]: hashes de 32 bytes, ordenados),
//!   así que la base crece con cada actualización. Más la firma de prueba EICAR, el archivo que
//!   usan todos los antivirus para probar que andan.
//! - **Protección en tiempo real**: cada archivo que llega por Brave, `curl`/`wget`, `winget` o
//!   `apt` se revisa al guardarlo, y cada programa antes de ejecutarlo. Si está infectado va a
//!   **cuarentena** ([`QUARANTINE`]): se aparta, no se borra (regla 13), y desde ahí no se puede
//!   ejecutar. `antivirus restaurar` lo devuelve a donde estaba.
//! - **A pedido**: `antivirus escanear [ruta]` revisa una carpeta entera.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem, FsError, Timestamp};

use crate::apps::Ctx;
use crate::i18n::trf;
use sha2::{Digest, Sha256};

pub const DIR: &str = "/Sistema/antivirus";
pub const DB: &str = "/Sistema/antivirus/firmas.bin";
pub const INFO: &str = "/Sistema/antivirus/estado.txt";
pub const QUARANTINE: &str = "/Cuarentena";
/// De dónde salió cada archivo en cuarentena (nombre en la cuarentena|ruta original).
const ORIGINS: &str = "/Sistema/antivirus/cuarentena.txt";
/// Los SHA-256 de las muestras de malware de las últimas 48 horas.
pub const FEED: &str = "https://bazaar.abuse.ch/export/txt/sha256/recent/";
/// La base no crece más que esto (16 MiB de hashes): se quedan los más nuevos.
pub const MAX_SIGNATURES: usize = 500_000;
/// Los archivos más grandes que esto no se revisan al escanear una carpeta.
pub const MAX_SCAN: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Threat {
    /// El archivo de prueba EICAR (no hace nada: sirve para probar el antivirus).
    Eicar,
    /// Un malware conocido: su SHA-256.
    Known([u8; 32]),
}

impl Threat {
    pub fn name(&self) -> String {
        match self {
            Threat::Eicar => "EICAR-Test-File (archivo de prueba de antivirus)".into(),
            Threat::Known(h) => format!(
                "malware conocido (MalwareBazaar, SHA-256 {}…)",
                &hex(h)[..16]
            ),
        }
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hash(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// La firma de prueba EICAR. Se arma de a partes: escrita de corrido, los antivirus del
/// anfitrión marcarían el código fuente y el kernel compilado.
fn eicar() -> Vec<u8> {
    let parts = [
        "X5O!P%@AP[4\\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-",
        "ANTIVIRUS-TEST-",
        "FILE!$H+H*",
    ];
    parts.concat().into_bytes()
}

/// ¿Es el archivo de prueba EICAR? Como dice su especificación: empieza con la firma y no
/// tiene más que espacios después (128 bytes como mucho).
fn is_eicar(data: &[u8]) -> bool {
    let sig = eicar();
    data.len() <= 128
        && data.starts_with(&sig)
        && data[sig.len()..].iter().all(|b| b.is_ascii_whitespace())
}

#[derive(Debug, Default)]
pub struct Antivirus {
    /// Ordenadas, sin repetidos.
    sigs: Vec<[u8; 32]>,
    loaded: bool,
    /// Protección en tiempo real.
    pub enabled: bool,
    /// Cuándo se actualizaron las firmas (segundos desde 1970), si alguna vez.
    pub updated: Option<u64>,
    /// En esta sesión: archivos revisados y amenazas encontradas.
    pub scanned: u64,
    pub found: u64,
}

impl Antivirus {
    pub fn new() -> Antivirus {
        Antivirus {
            enabled: true,
            ..Antivirus::default()
        }
    }

    /// Lee las firmas y el estado del disco (una vez).
    pub fn load<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Ok(b) = fs.read_file(DB) {
            self.sigs = b.as_chunks::<32>().0.to_vec();
            self.sigs.sort_unstable();
            self.sigs.dedup();
        }
        if let Ok(b) = fs.read_file(INFO) {
            for line in String::from_utf8_lossy(&b).lines() {
                match line.split_once('=') {
                    Some(("activado", v)) => self.enabled = v.trim() != "no",
                    Some(("actualizado", v)) => self.updated = v.trim().parse().ok(),
                    _ => {}
                }
            }
        }
    }

    pub fn count(&self) -> usize {
        self.sigs.len()
    }

    /// Revisa `data` (sin tocar el disco).
    pub fn scan(&self, data: &[u8]) -> Option<Threat> {
        if is_eicar(data) {
            return Some(Threat::Eicar);
        }
        let h = sha256(data);
        self.sigs
            .binary_search(&h)
            .is_ok()
            .then_some(Threat::Known(h))
    }

    /// Suma las firmas de la lista de MalwareBazaar (un SHA-256 por línea; `#` es comentario).
    /// Devuelve cuántas son nuevas.
    pub fn merge_feed(&mut self, text: &str, now: u64) -> usize {
        let before = self.sigs.len();
        let mut new: Vec<[u8; 32]> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(parse_hash)
            .filter(|h| self.sigs.binary_search(h).is_err())
            .collect();
        new.sort_unstable();
        new.dedup();
        let added = new.len();
        // Si no entran, se hace lugar sacando de las que ya estaban.
        let overflow = (before + added).saturating_sub(MAX_SIGNATURES);
        self.sigs.drain(..overflow.min(before));
        self.sigs.extend(new);
        self.sigs.sort_unstable();
        self.sigs.truncate(MAX_SIGNATURES);
        self.updated = Some(now);
        added
    }

    pub fn save<D: BlockDevice>(
        &self,
        fs: &mut FileSystem<D>,
        now: Timestamp,
    ) -> Result<(), FsError> {
        crate::term::apt::ensure_dirs(fs, DIR, now)?;
        let bytes: Vec<u8> = self.sigs.iter().flatten().copied().collect();
        fs.write_file(DB, &bytes, now)?;
        self.save_state(fs, now)
    }

    pub fn save_state<D: BlockDevice>(
        &self,
        fs: &mut FileSystem<D>,
        now: Timestamp,
    ) -> Result<(), FsError> {
        crate::term::apt::ensure_dirs(fs, DIR, now)?;
        let text = format!(
            "activado={}\nactualizado={}\n",
            if self.enabled { "si" } else { "no" },
            self.updated.map_or(String::new(), |u| u.to_string())
        );
        fs.write_file(INFO, text.as_bytes(), now)
    }

    /// Un archivo que acaba de llegar al disco (`path`, con este contenido). Si está infectado
    /// lo manda a cuarentena y devuelve la amenaza y dónde quedó. Con la protección apagada no
    /// revisa nada.
    pub fn check_new<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        path: &str,
        data: &[u8],
        now: Timestamp,
    ) -> Option<(Threat, Result<String, FsError>)> {
        self.load(fs);
        if !self.enabled {
            return None;
        }
        self.scanned += 1;
        let threat = self.scan(data)?;
        self.found += 1;
        Some((threat, quarantine(fs, path, now)))
    }

    /// ¿Se puede ejecutar este programa? `Err` con el motivo si no.
    pub fn may_run<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        path: &str,
        data: &[u8],
        now: Timestamp,
    ) -> Result<(), String> {
        if path.starts_with(&format!("{QUARANTINE}/")) {
            return Err("está en cuarentena: el antivirus no deja ejecutarlo".into());
        }
        match self.check_new(fs, path, data, now) {
            None => Ok(()),
            Some((t, moved)) => Err(format!(
                "el antivirus lo bloqueó: {} ({})",
                t.name(),
                match moved {
                    Ok(q) => format!("quedó en {q}"),
                    Err(e) => format!("no se pudo mover a cuarentena: {e}"),
                }
            )),
        }
    }
}

/// Lo que muestra Configuración (se copia en `SystemStats`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub enabled: bool,
    pub signatures: usize,
    pub updated: Option<u64>,
    pub scanned: u64,
    pub found: u64,
}

impl Default for Summary {
    /// Como arranca el antivirus (antes de leer su estado del disco): con la protección activada.
    fn default() -> Summary {
        Antivirus::new().summary()
    }
}

impl Antivirus {
    pub fn summary(&self) -> Summary {
        Summary {
            enabled: self.enabled,
            signatures: self.count(),
            updated: self.updated,
            scanned: self.scanned,
            found: self.found,
        }
    }
}

/// Revisa un archivo que una app acaba de guardar. Si está infectado lo manda a cuarentena,
/// avisa (notificación y log) y devuelve el mensaje para mostrar.
pub fn guard<D: BlockDevice>(ctx: &mut Ctx<'_, D>, path: &str, data: &[u8]) -> Option<String> {
    let now = ctx.timestamp();
    let fs = ctx.fs.as_deref_mut()?;
    let (threat, moved) = ctx.antivirus.check_new(fs, path, data, now)?;
    let kind = match threat {
        Threat::Eicar => "EICAR",
        Threat::Known(_) => "MalwareBazaar",
    };
    ctx.log.push(format!("ANTIVIRUS_AMENAZA {path} {kind}"));
    let msg = match moved {
        Ok(q) => trf(
            "Amenaza bloqueada en {}: {}. Quedó en cuarentena ({}).",
            &[path, &threat.name(), &q],
        ),
        Err(e) => trf(
            "Amenaza en {}: {}. No se pudo mover a cuarentena: {}",
            &[path, &threat.name(), &e.to_string()],
        ),
    };
    ctx.out.notify(msg.clone(), true);
    Some(msg)
}

/// "2026-10-05" a partir de segundos desde 1970 (UTC).
pub fn date(secs: u64) -> String {
    // El algoritmo de días civiles de Howard Hinnant.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

const HELP: &str = "antivirus: revisa archivos contra malware conocido (firmas de MalwareBazaar)
Uso:
  antivirus                     estado: protección, firmas, cuarentena
  antivirus actualizar          baja las firmas nuevas y las suma a la base
  antivirus escanear [RUTA]     revisa una carpeta entera (por defecto, todo el disco)
  antivirus cuarentena          lo que está apartado y de dónde vino
  antivirus restaurar NOMBRE    devuelve un archivo de la cuarentena a su lugar
  antivirus activar|desactivar  la protección en tiempo real
Lo que llega por Brave, curl, wget, winget y apt se revisa solo, y los programas antes de
ejecutarse. Lo infectado va a /Cuarentena: no se borra.";

/// `antivirus ...` en la Terminal. `Err(id)`: quedó bajando las firmas (ver `on_update`).
pub(crate) fn command<D: BlockDevice>(
    args: &[String],
    root: Option<String>,
    ctx: &mut Ctx<'_, D>,
    o: &mut String,
    e: &mut String,
) -> Result<i32, u32> {
    let now = ctx.timestamp();
    let Some(fs) = ctx.fs.as_deref_mut() else {
        e.push_str("antivirus: no hay disco\n");
        return Ok(1);
    };
    let av = &mut *ctx.antivirus;
    av.load(fs);
    match args.get(1).map(String::as_str).unwrap_or("estado") {
        "estado" => {
            o.push_str(&format!(
                "Protección en tiempo real: {}\nFirmas: {} (MalwareBazaar) + EICAR\nÚltima actualización: {}\nEn esta sesión: {} archivos revisados, {} amenazas\nEn cuarentena: {}\n",
                if av.enabled { "activada" } else { "DESACTIVADA" },
                av.count(),
                av.updated
                    .map_or("nunca (ejecutá «antivirus actualizar»)".into(), date),
                av.scanned,
                av.found,
                quarantined(fs).len()
            ));
            Ok(0)
        }
        "actualizar" | "update" => {
            o.push_str("Bajando las firmas nuevas de MalwareBazaar (abuse.ch)...\n");
            Err(ctx
                .out
                .fetch_as(FEED, crate::system::FetchKind::Download, "antivirus"))
        }
        "escanear" | "scan" => {
            let root = root.unwrap_or_else(|| "/".into());
            let files = walk(fs, &root);
            let mut found = 0;
            for path in &files {
                let Ok(data) = fs.read_file(path) else {
                    continue;
                };
                if data.len() > MAX_SCAN {
                    continue;
                }
                av.scanned += 1;
                if let Some(t) = av.scan(&data) {
                    found += 1;
                    av.found += 1;
                    let moved = match quarantine(fs, path, now) {
                        Ok(q) => format!("-> {q}"),
                        Err(err) => format!("(no se pudo mover: {err})"),
                    };
                    o.push_str(&format!("{path}: {} {moved}\n", t.name()));
                    ctx.log.push(format!("ANTIVIRUS_AMENAZA {path} escaneo"));
                }
            }
            o.push_str(&format!(
                "\n----------- RESUMEN -----------\nArchivos revisados: {}\nAmenazas: {found}\n",
                files.len()
            ));
            ctx.log
                .push(format!("ANTIVIRUS_ESCANEO {} {found}", files.len()));
            Ok(if found > 0 { 1 } else { 0 })
        }
        "cuarentena" => {
            let q = quarantined(fs);
            if q.is_empty() {
                o.push_str("La cuarentena está vacía.\n");
            }
            for (name, from) in q {
                o.push_str(&format!("{name}  (de {from})\n"));
            }
            Ok(0)
        }
        "restaurar" => {
            let Some(name) = args.get(2) else {
                e.push_str(
                    "antivirus: decime qué archivo restaurar (ver «antivirus cuarentena»)\n",
                );
                return Ok(1);
            };
            match restore(fs, name, now) {
                Ok(p) => {
                    o.push_str(&format!(
                        "Restaurado en {p}. Ojo: el antivirus lo había marcado.\n"
                    ));
                    Ok(0)
                }
                Err(err) => {
                    e.push_str(&format!("antivirus: {err}\n"));
                    Ok(1)
                }
            }
        }
        "activar" | "desactivar" => {
            av.enabled = args[1] == "activar";
            let _ = av.save_state(fs, now);
            o.push_str(if av.enabled {
                "Protección en tiempo real activada.\n"
            } else {
                "Protección en tiempo real DESACTIVADA: lo que se baje no se revisa.\n"
            });
            Ok(0)
        }
        "help" | "--help" | "-h" | "ayuda" => {
            o.push_str(HELP);
            o.push('\n');
            Ok(0)
        }
        other => {
            e.push_str(&format!(
                "antivirus: «{other}» no es una orden (probá «antivirus ayuda»)\n"
            ));
            Ok(1)
        }
    }
}

/// Llegaron las firmas (`antivirus actualizar`).
pub(crate) fn on_update<D: BlockDevice>(
    result: &Result<crate::system::HttpResponse, String>,
    ctx: &mut Ctx<'_, D>,
    out: &mut crate::term::Out,
) -> (i32, String) {
    let body = match result {
        Ok(r) if r.status == 200 => &r.body,
        Ok(r) => {
            out.err(&format!(
                "antivirus: el servidor de firmas respondió HTTP {}",
                r.status
            ));
            return (1, String::new());
        }
        Err(err) => {
            out.err(&format!(
                "antivirus: no se pudieron bajar las firmas: {err}"
            ));
            return (1, String::new());
        }
    };
    let now = ctx.timestamp();
    let secs = crate::procs::unix_seconds(now);
    let Some(fs) = ctx.fs.as_deref_mut() else {
        out.err("antivirus: no hay disco");
        return (1, String::new());
    };
    let av = &mut *ctx.antivirus;
    av.load(fs);
    let added = av.merge_feed(&String::from_utf8_lossy(body), secs);
    if let Err(err) = av.save(fs, now) {
        out.err(&format!(
            "antivirus: no se pudieron guardar las firmas: {err}"
        ));
        return (1, String::new());
    }
    ctx.log
        .push(format!("ANTIVIRUS_FIRMAS {} {added}", av.count()));
    out.info(&format!(
        "{added} firmas nuevas. La base tiene {} firmas.",
        av.count()
    ));
    (0, String::new())
}

/// Aparta `path` en la cuarentena (con un nombre único) y anota de dónde vino.
pub fn quarantine<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    path: &str,
    now: Timestamp,
) -> Result<String, FsError> {
    crate::term::apt::ensure_dirs(fs, QUARANTINE, now)?;
    let name = crate::files::move_unique(fs, path, QUARANTINE)?;
    let mut origins = fs.read_file(ORIGINS).unwrap_or_default();
    origins.extend_from_slice(format!("{name}|{path}\n").as_bytes());
    crate::term::apt::ensure_dirs(fs, DIR, now)?;
    fs.write_file(ORIGINS, &origins, now)?;
    Ok(format!("{QUARANTINE}/{name}"))
}

/// Lo que hay en cuarentena: (nombre, ruta original).
pub fn quarantined<D: BlockDevice>(fs: &mut FileSystem<D>) -> Vec<(String, String)> {
    let origins = fs.read_file(ORIGINS).unwrap_or_default();
    String::from_utf8_lossy(&origins)
        .lines()
        .filter_map(|l| {
            let (n, p) = l.split_once('|')?;
            fs.exists(&format!("{QUARANTINE}/{n}"))
                .then(|| (n.to_string(), p.to_string()))
        })
        .collect()
}

/// Devuelve un archivo de la cuarentena a su carpeta original. Devuelve dónde quedó.
pub fn restore<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    name: &str,
    now: Timestamp,
) -> Result<String, String> {
    let (_, original) = quarantined(fs)
        .into_iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| format!("{name} no está en la cuarentena"))?;
    let dir = crate::files::parent(&original);
    crate::term::apt::ensure_dirs(fs, &dir, now).map_err(|e| e.to_string())?;
    let back = crate::files::move_unique(fs, &format!("{QUARANTINE}/{name}"), &dir)
        .map_err(|e| e.to_string())?;
    Ok(if dir == "/" {
        format!("/{back}")
    } else {
        format!("{dir}/{back}")
    })
}

/// Todos los archivos debajo de `root` (sin la cuarentena ni la Papelera).
pub fn walk<D: BlockDevice>(fs: &mut FileSystem<D>, root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = alloc::vec![root.to_string()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs.list(&dir) else {
            if fs.exists(&dir) {
                out.push(dir); // era un archivo
            }
            continue;
        };
        for e in entries {
            let p = if dir == "/" {
                format!("/{}", e.name)
            } else {
                format!("{dir}/{}", e.name)
            };
            if p == QUARANTINE || p == "/Papelera" {
                continue;
            }
            if e.is_dir {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmas_y_eicar() {
        let mut av = Antivirus::new();
        let malo = b"programa malicioso de mentira";
        let h = sha256(malo);
        assert_eq!(av.scan(malo), None);
        let feed = format!(
            "# MalwareBazaar\n#\n{}\nno-es-un-hash\n{}\n",
            hex(&h),
            hex(&sha256(b"otro"))
        );
        assert_eq!(av.merge_feed(&feed, 1_790_000_000), 2);
        assert_eq!(
            av.merge_feed(&feed, 1_790_000_100),
            0,
            "las repetidas no se suman"
        );
        assert_eq!(av.count(), 2);
        assert_eq!(av.scan(malo), Some(Threat::Known(h)));
        assert!(av.scan(malo).unwrap().name().contains(&hex(&h)[..16]));
        assert_eq!(av.scan(b"un archivo cualquiera"), None);
        // EICAR: la firma sola (con un salto de línea), pero no metida en otro archivo.
        let mut e = eicar();
        e.extend_from_slice(b"\r\n");
        assert_eq!(av.scan(&e), Some(Threat::Eicar));
        let mut dentro = b"texto antes ".to_vec();
        dentro.extend_from_slice(&eicar());
        assert_eq!(av.scan(&dentro), None);
        assert_eq!(parse_hash(&hex(&h)), Some(h));
    }

    #[test]
    fn la_base_tiene_tope() {
        let mut av = Antivirus::new();
        let feed: String = (0..(MAX_SIGNATURES as u32 + 10))
            .map(|i| format!("{}\n", hex(&sha256(&i.to_le_bytes()))))
            .collect();
        av.merge_feed(&feed, 1);
        assert_eq!(av.count(), MAX_SIGNATURES);
    }
}
