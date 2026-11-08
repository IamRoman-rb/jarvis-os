//! Listas de bloqueo del firewall (ADR 0024), como pi-hole o los archivos `hosts` de
//! StevenBlack: miles de dominios de malware o de publicidad que se bloquean sin escribir una
//! regla por cada uno.
//!
//! - **URLhaus** (abuse.ch): dominios que están repartiendo malware ahora. Uso libre.
//! - **StevenBlack/hosts** (GitHub, MIT): publicidad y malware unificados (~80 000 dominios).
//!
//! Los dos vienen como archivos `hosts` (`0.0.0.0 dominio` o `127.0.0.1 dominio`). Se guardan
//! **compactos**: el hash FNV-1a de 64 bits de cada dominio, ordenados, en
//! `/Sistema/firewall/<lista>.bin`. Buscar es una búsqueda binaria (unas 17 comparaciones para
//! 100 000 dominios) y ocupa 8 bytes por dominio en vez del texto. Un dominio bloquea también
//! sus subdominios: se prueba `a.b.ejemplo.com`, `b.ejemplo.com` y `ejemplo.com`.
//!
//! Dos dominios distintos con el mismo hash bloquearían uno de más: con 64 bits y 100 000
//! dominios la probabilidad es de una en mil millones.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};

pub const DIR: &str = "/Sistema/firewall";

/// Las listas conocidas: (nombre, descripción, de dónde se baja).
pub const LISTS: [(&str, &str, &str); 2] = [
    (
        "urlhaus",
        "Malware (URLhaus, abuse.ch)",
        "https://urlhaus.abuse.ch/downloads/hostfile/",
    ),
    (
        "stevenblack",
        "Publicidad y malware (StevenBlack/hosts)",
        "https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts",
    ),
];

/// Lo más que se guarda de una lista.
pub const MAX_ENTRIES: usize = 400_000;

pub fn url_of(name: &str) -> Option<&'static str> {
    LISTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, u)| *u)
}

pub fn path_of(name: &str) -> String {
    format!("{DIR}/{name}.bin")
}

/// FNV-1a de 64 bits del dominio en minúsculas.
pub fn hash(domain: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in domain.bytes() {
        h ^= u64::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Los dominios de un archivo `hosts` (sin `localhost` ni las direcciones de la red local).
pub fn parse_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(first), second) = (words.next(), words.next()) else {
            continue;
        };
        // `0.0.0.0 dominio` o solo `dominio`.
        let domain = match second {
            Some(d) if first.contains('.') || first.contains(':') => d,
            _ => first,
        };
        let d = domain.trim_end_matches('.').to_ascii_lowercase();
        if d.is_empty()
            || !d.contains('.')
            || matches!(
                d.as_str(),
                "localhost" | "localhost.localdomain" | "local" | "broadcasthost" | "0.0.0.0"
            )
            || d.chars()
                .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        {
            continue;
        }
        out.push(d);
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out
}

/// Los hashes ordenados y sin repetidos (lo que se guarda en el `.bin`).
pub fn compile(domains: &[String]) -> Vec<u64> {
    let mut v: Vec<u64> = domains.iter().map(|d| hash(d)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

pub fn to_bytes(hashes: &[u64]) -> Vec<u8> {
    hashes.iter().flat_map(|h| h.to_le_bytes()).collect()
}

pub fn from_bytes(b: &[u8]) -> Vec<u64> {
    let mut v: Vec<u64> = b
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    // Por si el archivo se armó a mano: ordenado para la búsqueda binaria.
    if !v.is_sorted() {
        v.sort_unstable();
        v.dedup();
    }
    v
}

/// Las listas activas, cargadas.
#[derive(Default)]
pub struct Blocklists {
    /// (nombre, hashes ordenados).
    lists: Vec<(String, Vec<u64>)>,
    loaded: bool,
}

impl Blocklists {
    pub fn new() -> Blocklists {
        Blocklists::default()
    }

    /// Vuelve a leer del disco las listas activas (`enabled`).
    pub fn reload<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>, enabled: &[String]) {
        self.lists.clear();
        for name in enabled {
            if let Ok(b) = fs.read_file(&path_of(name)) {
                self.lists.push((name.clone(), from_bytes(&b)));
            }
        }
        self.loaded = true;
    }

    /// Para los tests: listas armadas en memoria.
    pub fn reload_from(&mut self, lists: Vec<(String, Vec<u64>)>) {
        self.lists = lists;
        self.loaded = true;
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn invalidate(&mut self) {
        self.loaded = false;
    }

    /// Si `host` (o un dominio del que es subdominio) está en una lista, cuál.
    pub fn find(&self, host: &str) -> Option<&str> {
        if self.lists.is_empty() || host.parse::<core::net::Ipv4Addr>().is_ok() {
            return None;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let mut candidates = Vec::new();
        let mut rest = host.as_str();
        loop {
            candidates.push(hash(rest));
            match rest.split_once('.') {
                // No se prueba el dominio de primer nivel solo ("com").
                Some((_, tail)) if tail.contains('.') => rest = tail,
                _ => break,
            }
        }
        for (name, hashes) in &self.lists {
            if candidates.iter().any(|h| hashes.binary_search(h).is_ok()) {
                return Some(name);
            }
        }
        None
    }

    pub fn count(&self) -> usize {
        self.lists.iter().map(|(_, v)| v.len()).sum()
    }
}

/// Cuántos dominios tiene la lista guardada (o `None` si no se bajó).
pub fn stored_count<D: BlockDevice>(fs: &mut FileSystem<D>, name: &str) -> Option<usize> {
    fs.stat(&path_of(name)).ok().map(|s| s.size as usize / 8)
}

/// Guarda una lista recién bajada. Devuelve cuántos dominios quedaron.
pub fn store<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    name: &str,
    text: &str,
    now: jarvis_fs::Timestamp,
) -> Result<usize, String> {
    let domains = parse_hosts(text);
    if domains.is_empty() {
        return Err("la lista llegó vacía (¿la página cambió?)".into());
    }
    let hashes = compile(&domains);
    if !fs.exists(DIR) {
        if !fs.exists("/Sistema") {
            fs.mkdir("/Sistema", now).map_err(|e| e.to_string())?;
        }
        fs.mkdir(DIR, now).map_err(|e| e.to_string())?;
    }
    fs.write_file(&path_of(name), &to_bytes(&hashes), now)
        .map_err(|e| e.to_string())?;
    Ok(hashes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archivo_hosts() {
        let text = "# StevenBlack\n127.0.0.1 localhost\n0.0.0.0 0.0.0.0\n\
                    0.0.0.0 ads.ejemplo.com # publicidad\n127.0.0.1\tmalo.net\n\
                    ::1 ip6-localhost\nsolo-dominio.org\n0.0.0.0 conñ.com\n";
        assert_eq!(
            parse_hosts(text),
            ["ads.ejemplo.com", "malo.net", "solo-dominio.org"]
        );
    }

    #[test]
    fn busca_dominios_y_subdominios() {
        let mut b = Blocklists::new();
        let hashes = compile(&["malo.net".into(), "ads.ejemplo.com".into()]);
        b.lists
            .push(("urlhaus".into(), from_bytes(&to_bytes(&hashes))));
        assert_eq!(b.find("malo.net"), Some("urlhaus"));
        assert_eq!(b.find("WWW.Malo.NET."), Some("urlhaus"));
        assert_eq!(b.find("x.ads.ejemplo.com"), Some("urlhaus"));
        assert_eq!(b.find("ejemplo.com"), None);
        assert_eq!(b.find("bueno.net"), None);
        assert_eq!(b.find("10.0.2.2"), None);
        assert_eq!(b.count(), 2);
    }
}
