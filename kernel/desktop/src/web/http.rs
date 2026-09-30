//! HTTP/1.1, del lado del cliente: armar el pedido y entender la respuesta.
//!
//! El kernel abre la conexión TCP y le pasa los bytes crudos a [`parse_response`]. Las
//! redirecciones (301, 302…) las maneja [`Fetch`]: dice a dónde conectarse y, con cada
//! respuesta, si terminó o hay que ir a otra dirección.
//!
//! HTTPS: desde K10 el kernel hace el TLS él mismo ([`Target::Tls`], ADR 0009). Si no puede (sin
//! entropía o sin hora) o si el usuario lo eligió en Configuración, esas páginas se piden a un
//! **puente en el anfitrión** (lo levanta `cargo xtask run`), con la dirección completa en el
//! pedido, como a un proxy HTTP: `GET https://sitio/camino HTTP/1.1`. El puente hace el TLS y
//! devuelve la respuesta en texto plano. Los nombres `.jarvis` van siempre al puente (el
//! repositorio de paquetes vive ahí).
//!
//! Imágenes: los PNG y JPEG los decodifica el kernel (`jarvis-image`, ADR 0009) y se piden
//! directo. Los SVG, GIF, WebP e ICO se piden al puente con `X-Jarvis-Imagen: bmp` (los
//! convierte). Si una imagen pedida directo resulta ser de otro formato (la dirección no lo
//! dice), se vuelve a pedir al puente.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::url::{Scheme, Url};
use crate::system::{FetchKind, HttpResponse};

/// Dirección del anfitrión vista desde QEMU (red "user": la puerta de enlace es el host).
pub const PROXY_HOST: [u8; 4] = [10, 0, 2, 2];
pub const PROXY_PORT: u16 = 8118;
const MAX_REDIRECTS: u32 = 5;
/// Nombres que resuelve el puente (el repositorio de paquetes): no se pregunta al DNS.
pub const BRIDGE_DOMAIN: &str = ".jarvis";

/// Tope de tamaño según para qué es la descarga.
pub fn max_body(kind: FetchKind) -> usize {
    match kind {
        FetchKind::Download => 32 * 1024 * 1024,
        FetchKind::Image => 12 * 1024 * 1024,
        FetchKind::Page => 8 * 1024 * 1024,
    }
}

/// A quién conectarse y qué mandarle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Directo al servidor (HTTP): hay que resolver el nombre con DNS.
    Direct { host: String, port: u16 },
    /// Directo al servidor, con TLS (HTTPS hecho por el kernel): `host` es el nombre que tiene que
    /// estar en el certificado.
    Tls { host: String, port: u16 },
    /// Al puente del anfitrión (HTTPS de respaldo, imágenes y `.jarvis`).
    Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connect {
    pub target: Target,
    pub request: Vec<u8>,
}

pub fn request(url: &Url, via_proxy: bool) -> Vec<u8> {
    request_kind(url, via_proxy, FetchKind::Page)
}

/// El pedido GET. A las imágenes que van al puente se les agrega `X-Jarvis-Imagen: bmp`: las
/// convierte.
pub fn request_kind(url: &Url, via_proxy: bool, kind: FetchKind) -> Vec<u8> {
    let target = if via_proxy {
        url.to_string()
    } else {
        url.path.clone()
    };
    // La API de la tienda de snaps exige decir la "serie" del sistema (la 16, como Ubuntu).
    let series = if url.host == "api.snapcraft.io" {
        "Snap-Device-Series: 16\r\nSnap-Device-Architecture: amd64\r\n"
    } else {
        ""
    };
    let (accept, extra) = match kind {
        FetchKind::Image if via_proxy => ("image/*", "X-Jarvis-Imagen: bmp\r\n"),
        FetchKind::Image => ("image/png,image/jpeg,image/*;q=0.5", ""),
        FetchKind::Download => ("*/*", ""),
        FetchKind::Page => ("text/html,text/plain;q=0.9,text/css;q=0.8,*/*;q=0.5", ""),
    };
    format!(
        "GET {target} HTTP/1.1\r\n\
         Host: {}\r\n\
         User-Agent: Mozilla/5.0 (compatible; JARVIS-OS/0.1; navegador de texto)\r\n\
         Accept: {accept}\r\n\
         Accept-Language: es-AR,es;q=0.9,en;q=0.5\r\n\
         Accept-Encoding: identity\r\n\
         {extra}{series}\
         Connection: close\r\n\r\n",
        url.host
    )
    .into_bytes()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Parsed {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Interpreta una respuesta completa (el servidor cerró la conexión).
pub fn parse_response(raw: &[u8]) -> Result<Parsed, String> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("respuesta incompleta (sin fin de cabeceras)")?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(format!("no es una respuesta HTTP: {status_line}"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or("código de estado inválido")?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let mut parsed = Parsed {
        status,
        headers,
        body: Vec::new(),
    };
    let body = &raw[end + 4..];
    parsed.body = if parsed
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        dechunk(body)?
    } else if let Some(len) = parsed
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        body[..len.min(body.len())].to_vec()
    } else {
        body.to_vec()
    };
    if let Some(enc) = parsed.header("content-encoding")
        && !enc.eq_ignore_ascii_case("identity")
    {
        return Err(format!(
            "el servidor mandó la página comprimida ({enc}) y todavía no sé descomprimir"
        ));
    }
    Ok(parsed)
}

/// "Transfer-Encoding: chunked": trozos precedidos por su tamaño en hexadecimal.
fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("trozo sin tamaño")?;
        let size_str = String::from_utf8_lossy(&b[..eol]);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16).map_err(|_| "tamaño de trozo inválido")?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            // La conexión se cortó a la mitad: se muestra lo que llegó.
            out.extend_from_slice(b);
            return Ok(out);
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
}

/// Una descarga con redirecciones.
pub struct Fetch {
    pub url: Url,
    pub kind: FetchKind,
    /// El kernel hace el TLS (si no, HTTPS va por el puente).
    pub tls: bool,
    /// La imagen va al puente para que la convierta (no es PNG ni JPEG).
    image_bridge: bool,
    redirects: u32,
}

/// Formatos de imagen que el kernel no decodifica: se piden al puente de entrada.
const BRIDGE_IMAGES: [&str; 5] = [".svg", ".svgz", ".gif", ".webp", ".ico"];

pub enum Step {
    Connect(Connect),
    Done(HttpResponse),
    Failed(String),
}

impl Fetch {
    pub fn start(url: &str) -> (Fetch, Step) {
        Self::start_kind(url, FetchKind::Page)
    }

    pub fn start_kind(url: &str, kind: FetchKind) -> (Fetch, Step) {
        Self::start_with(url, kind, false)
    }

    /// Con `tls`, el HTTPS lo hace el kernel ([`Target::Tls`]) en vez del puente.
    pub fn start_with(url: &str, kind: FetchKind, tls: bool) -> (Fetch, Step) {
        match Url::parse(url) {
            Some(u) => {
                let path = u.path_only().to_ascii_lowercase();
                let f = Fetch {
                    image_bridge: BRIDGE_IMAGES.iter().any(|e| path.ends_with(e)),
                    url: u,
                    kind,
                    tls,
                    redirects: 0,
                };
                let step = f.connect();
                (f, step)
            }
            None => (
                Fetch {
                    url: Url {
                        scheme: Scheme::About,
                        host: String::new(),
                        port: 0,
                        path: String::new(),
                    },
                    kind,
                    tls,
                    image_bridge: false,
                    redirects: 0,
                },
                Step::Failed(format!("dirección inválida: {url}")),
            ),
        }
    }

    fn connect(&self) -> Step {
        let bridge = self.bridge();
        match self.url.scheme {
            Scheme::Http if !bridge => Step::Connect(Connect {
                target: Target::Direct {
                    host: self.url.host.clone(),
                    port: self.url.port,
                },
                request: request_kind(&self.url, false, self.kind),
            }),
            Scheme::Https if self.tls && !bridge => Step::Connect(Connect {
                target: Target::Tls {
                    host: self.url.host.clone(),
                    port: self.url.port,
                },
                request: request_kind(&self.url, false, self.kind),
            }),
            Scheme::Http | Scheme::Https => Step::Connect(Connect {
                target: Target::Proxy,
                request: request_kind(&self.url, true, self.kind),
            }),
            _ => Step::Failed("el kernel solo descarga http:// y https://".into()),
        }
    }

    /// ¿El pedido va al puente? Los nombres `.jarvis` (el repositorio de paquetes), las imágenes
    /// que hay que convertir y, sin TLS propio, todo lo que es HTTPS.
    fn bridge(&self) -> bool {
        (self.kind == FetchKind::Image && self.image_bridge)
            || self.url.host.ends_with(BRIDGE_DOMAIN)
    }

    fn via_proxy(&self) -> bool {
        self.bridge() || (self.url.scheme == Scheme::Https && !self.tls)
    }

    /// Llegó la respuesta completa de la conexión anterior.
    pub fn on_response(&mut self, raw: &[u8]) -> Step {
        let parsed = match parse_response(raw) {
            Ok(p) => p,
            Err(e) => return Step::Failed(e),
        };
        // Una imagen pedida directo que no es PNG ni JPEG (ni BMP): que la convierta el puente.
        if self.kind == FetchKind::Image
            && !self.via_proxy()
            && parsed.status < 300
            && jarvis_image::sniff(&parsed.body).is_none()
            && !parsed.body.starts_with(b"BM")
        {
            self.image_bridge = true;
            return self.connect();
        }
        if (300..400).contains(&parsed.status)
            && let Some(loc) = parsed.header("location")
        {
            self.redirects += 1;
            if self.redirects > MAX_REDIRECTS {
                return Step::Failed("demasiadas redirecciones".into());
            }
            match self.url.join(loc) {
                Some(next) => {
                    let path = next.path_only().to_ascii_lowercase();
                    self.image_bridge |= BRIDGE_IMAGES.iter().any(|e| path.ends_with(e));
                    self.url = next;
                    return self.connect();
                }
                None => return Step::Failed(format!("redirección inválida: {loc}")),
            }
        }
        Step::Done(HttpResponse {
            status: parsed.status,
            content_type: parsed.header("content-type").unwrap_or("").to_string(),
            url: self.url.to_string(),
            body: parsed.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respuesta_simple_y_por_trozos() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\n\r\nhola!extra";
        let p = parse_response(raw).unwrap();
        assert_eq!((p.status, p.body.as_slice()), (200, &b"hola!"[..]));
        assert_eq!(p.header("CONTENT-TYPE"), Some("text/html"));

        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nJARV\r\n2;x=y\r\nIS\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap().body, b"JARVIS");
        assert!(parse_response(b"basura").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\nxx").is_err());
    }

    #[test]
    fn https_con_tls_propio_va_directo() {
        let (_, step) = Fetch::start_with("https://example.com/a", FetchKind::Page, true);
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(
            c.target,
            Target::Tls {
                host: "example.com".into(),
                port: 443
            }
        );
        // Pedido de servidor común, no de proxy.
        assert!(c.request.starts_with(b"GET /a HTTP/1.1\r\n"));
        // Los nombres .jarvis siguen yendo al puente.
        let (_, step) = Fetch::start_with("https://paquetes.jarvis/x", FetchKind::Page, true);
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(c.target, Target::Proxy);
    }

    #[test]
    fn imagenes_png_y_jpeg_directo_y_las_demas_al_puente() {
        // Un PNG va directo, sin pedirle al puente que lo convierta.
        let (mut f, step) = Fetch::start_with("https://example.com/a.png", FetchKind::Image, true);
        let Step::Connect(c) = step else { panic!() };
        assert!(matches!(c.target, Target::Tls { .. }));
        assert!(!String::from_utf8_lossy(&c.request).contains("X-Jarvis-Imagen"));
        let png = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n\x89PNG\r\n\x1a\n";
        assert!(matches!(f.on_response(png), Step::Done(r) if r.body.len() == 8));
        // Una dirección sin extensión que resulta ser un WebP: se vuelve a pedir al puente.
        let (mut f, _) = Fetch::start_with("http://example.com/foto", FetchKind::Image, true);
        let webp = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nRIFF";
        let Step::Connect(c) = f.on_response(webp) else {
            panic!()
        };
        assert_eq!(c.target, Target::Proxy);
        assert!(String::from_utf8_lossy(&c.request).contains("X-Jarvis-Imagen: bmp"));
        // Un SVG va al puente de entrada.
        let (_, step) = Fetch::start_with("https://example.com/logo.svg", FetchKind::Image, true);
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(c.target, Target::Proxy);
        // Sin TLS propio, un PNG por HTTPS va al puente (que igual lo convierte).
        let (_, step) = Fetch::start_with("https://example.com/a.png", FetchKind::Image, false);
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(c.target, Target::Proxy);
        assert!(String::from_utf8_lossy(&c.request).contains("X-Jarvis-Imagen: bmp"));
    }

    #[test]
    fn sigue_redirecciones() {
        let (mut f, step) = Fetch::start("http://example.com/viejo");
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(
            c.target,
            Target::Direct {
                host: "example.com".into(),
                port: 80
            }
        );
        assert!(
            c.request
                .starts_with(b"GET /viejo HTTP/1.1\r\nHost: example.com\r\n")
        );
        // Redirección a HTTPS: el próximo pedido va al puente, con la dirección completa.
        let step =
            f.on_response(b"HTTP/1.1 301 Moved\r\nLocation: https://example.com/nuevo\r\n\r\n");
        let Step::Connect(c) = step else { panic!() };
        assert_eq!(c.target, Target::Proxy);
        assert!(
            c.request
                .starts_with(b"GET https://example.com/nuevo HTTP/1.1")
        );
        let step = f.on_response(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nlisto");
        let Step::Done(r) = step else { panic!() };
        assert_eq!(
            (r.url.as_str(), r.body.as_slice()),
            ("https://example.com/nuevo", &b"listo"[..])
        );
    }

    #[test]
    fn corta_los_bucles_de_redirecciones() {
        let (mut f, _) = Fetch::start("http://a.com/");
        let mut last = None;
        for _ in 0..10 {
            last = Some(f.on_response(b"HTTP/1.1 302 Found\r\nLocation: /otra\r\n\r\n"));
            if matches!(last, Some(Step::Failed(_))) {
                break;
            }
        }
        assert!(matches!(last, Some(Step::Failed(_))));
    }
}
