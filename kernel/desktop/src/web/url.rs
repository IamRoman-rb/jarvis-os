//! Direcciones web: `esquema://host:puerto/camino?consulta`.

use alloc::format;
use alloc::string::{String, ToString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
    /// Un archivo del disco de JARVIS (`file:///Documentos/pagina.html`).
    File,
    /// Páginas internas del navegador (`about:inicio`).
    About,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    /// Camino con la consulta (`/buscar?q=hola`); siempre empieza con `/` (salvo `about:`).
    pub path: String,
}

/// El buscador que usa la barra de direcciones: la versión HTML de DuckDuckGo, que funciona
/// sin JavaScript.
pub const SEARCH: &str = "https://html.duckduckgo.com/html/?q=";

impl Url {
    pub fn parse(s: &str) -> Option<Url> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("about:") {
            return Some(Url {
                scheme: Scheme::About,
                host: String::new(),
                port: 0,
                path: rest.into(),
            });
        }
        let (scheme, rest) = s.split_once("://")?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            "file" => Scheme::File,
            _ => return None,
        };
        if scheme == Scheme::File {
            let path = if rest.starts_with('/') {
                rest.to_string()
            } else {
                format!("/{rest}")
            };
            return Some(Url {
                scheme,
                host: String::new(),
                port: 0,
                path,
            });
        }
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // Sin usuario:contraseña@ (no se mandan credenciales en direcciones).
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (authority, if scheme == Scheme::Https { 443 } else { 80 }),
        };
        if host.is_empty() {
            return None;
        }
        let mut path = path.split('#').next().unwrap_or("/").to_string();
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        Some(Url {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            path,
        })
    }

    /// Resuelve un enlace (`href`) relativo a esta página.
    pub fn join(&self, href: &str) -> Option<Url> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return Some(self.clone());
        }
        if href.contains("://") || href.starts_with("about:") {
            return Url::parse(href);
        }
        if let Some(rest) = href.strip_prefix("//") {
            let scheme = if self.scheme == Scheme::Https {
                "https"
            } else {
                "http"
            };
            return Url::parse(&format!("{scheme}://{rest}"));
        }
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("javascript:")
            || lower.starts_with("mailto:")
            || lower.starts_with("data:")
        {
            return None;
        }
        let mut u = self.clone();
        let href = href.split('#').next().unwrap_or("");
        u.path = if href.starts_with('/') {
            href.to_string()
        } else if href.starts_with('?') {
            let base = self.path.split('?').next().unwrap_or("/");
            format!("{base}{href}")
        } else {
            let base = self.path.split('?').next().unwrap_or("/");
            let dir = &base[..base.rfind('/').map_or(0, |i| i + 1)];
            format!("{dir}{href}")
        };
        u.path = normalize(&u.path);
        Some(u)
    }

    /// Solo el camino, sin la consulta.
    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or("/")
    }

    /// Valor de un parámetro de la consulta (`?q=hola` → `q` = "hola"), ya decodificado.
    pub fn query_param(&self, name: &str) -> Option<String> {
        let query = self.path.split_once('?')?.1;
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == name).then(|| percent_decode(v))
        })
    }
}

impl core::fmt::Display for Url {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.scheme {
            Scheme::About => write!(f, "about:{}", self.path),
            Scheme::File => write!(f, "file://{}", self.path),
            Scheme::Http | Scheme::Https => {
                let (name, default) = if self.scheme == Scheme::Https {
                    ("https", 443)
                } else {
                    ("http", 80)
                };
                if self.port == default {
                    write!(f, "{name}://{}{}", self.host, self.path)
                } else {
                    write!(f, "{name}://{}:{}{}", self.host, self.port, self.path)
                }
            }
        }
    }
}

/// Saca los `.` y `..` de un camino.
fn normalize(path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    if path.ends_with('/') && !parts.is_empty() {
        out.push('/');
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    out
}

/// Lo que se escribe en la barra de direcciones → una dirección. Si no parece una dirección
/// (tiene espacios, no tiene punto, o empieza con "? "), se busca en la web.
pub fn from_input(text: &str) -> String {
    let t = text.trim();
    if let Some(q) = t.strip_prefix("? ") {
        return format!("{SEARCH}{}", percent_encode(q.trim()));
    }
    if t.contains("://") || t.starts_with("about:") {
        return t.into();
    }
    if t.starts_with('/') {
        return format!("file://{t}");
    }
    let looks_like_host = !t.contains(' ') && (t.contains('.') || t.starts_with("localhost"));
    if looks_like_host {
        format!("https://{t}")
    } else {
        format!("{SEARCH}{}", percent_encode(t))
    }
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = alloc::vec::Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(core::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partes_de_una_direccion() {
        let u = Url::parse("https://Example.com:8443/a/b?x=1#frag").unwrap();
        assert_eq!(
            (u.scheme, u.host.as_str(), u.port, u.path.as_str()),
            (Scheme::Https, "example.com", 8443, "/a/b?x=1")
        );
        let u = Url::parse("http://example.com").unwrap();
        assert_eq!((u.port, u.path.as_str()), (80, "/"));
        assert_eq!(u.to_string(), "http://example.com/");
        assert_eq!(Url::parse("ftp://x").map(|_| ()), None);
        let f = Url::parse("file:///Documentos/a.html").unwrap();
        assert_eq!(
            (f.scheme, f.path.as_str()),
            (Scheme::File, "/Documentos/a.html")
        );
    }

    #[test]
    fn enlaces_relativos() {
        let base = Url::parse("https://site.org/docs/guia/intro.html?v=2").unwrap();
        let j = |h: &str| base.join(h).unwrap().to_string();
        assert_eq!(j("cap2.html"), "https://site.org/docs/guia/cap2.html");
        assert_eq!(j("../api/"), "https://site.org/docs/api/");
        assert_eq!(j("/raiz"), "https://site.org/raiz");
        assert_eq!(j("//otro.com/x"), "https://otro.com/x");
        assert_eq!(j("?v=3"), "https://site.org/docs/guia/intro.html?v=3");
        assert_eq!(j("http://abs.com"), "http://abs.com/");
        assert_eq!(j("#arriba"), base.to_string());
        assert!(base.join("javascript:alert(1)").is_none());
    }

    #[test]
    fn barra_de_direcciones() {
        assert_eq!(from_input("example.com"), "https://example.com");
        assert_eq!(from_input("http://a.b/c"), "http://a.b/c");
        assert_eq!(
            from_input("clima en córdoba"),
            format!("{SEARCH}clima+en+c%C3%B3rdoba")
        );
        assert_eq!(from_input("? rust"), format!("{SEARCH}rust"));
        assert_eq!(
            from_input("/Documentos/x.html"),
            "file:///Documentos/x.html"
        );
        assert_eq!(percent_decode("c%C3%B3rdoba+x%2"), "córdoba x%2");
        let u = Url::parse("https://d.com/l/?uddg=https%3A%2F%2Fa.com%2F&x=1").unwrap();
        assert_eq!(u.query_param("uddg").as_deref(), Some("https://a.com/"));
    }
}
