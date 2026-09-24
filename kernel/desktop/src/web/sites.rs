//! Adaptadores para sitios que se arman con JavaScript.
//!
//! Hay páginas que llegan casi vacías: el contenido lo arma un programa en JavaScript, que
//! JARVIS-OS todavía no ejecuta. Algunas igual traen los datos adentro (como JSON en un
//! `<script>`) para que ese programa no tenga que pedirlos. Acá se leen esos datos y se arma una
//! versión en HTML simple que el navegador sí puede mostrar.
//!
//! - **YouTube**: búsquedas (`/results`), videos (`/watch`) e inicio. Los videos no se
//!   reproducen: haría falta decodificar H.264/VP9 y audio (roadmap K10); se muestran la
//!   miniatura, el título, el canal, las vistas y la descripción.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::json::{self, Json};
use super::url::Url;

/// Si la página es de un sitio conocido que necesita JavaScript, la reemplaza por una versión
/// que se puede mostrar. Si no, la devuelve igual.
pub fn adapt(u: Option<&Url>, html: String) -> String {
    let Some(u) = u else { return html };
    let host = u.host.to_ascii_lowercase();
    if host == "youtube.com" || host.ends_with(".youtube.com") {
        return youtube(u, &html).unwrap_or(html);
    }
    html
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// El texto de un campo de YouTube: `{"simpleText": …}` o `{"runs": [{"text": …}, …]}`.
fn yt_text(v: Option<&Json>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = v.get("simpleText").and_then(Json::str) {
        return s.to_string();
    }
    if let Some(s) = v.get("content").and_then(Json::str) {
        return s.to_string();
    }
    v.get("runs")
        .map(|r| {
            r.arr()
                .iter()
                .filter_map(|x| x.get("text").and_then(Json::str))
                .collect::<Vec<_>>()
                .concat()
        })
        .unwrap_or_default()
}

struct Video {
    id: String,
    title: String,
    channel: String,
    views: String,
    length: String,
    published: String,
}

/// Todos los objetos `"<clave>": {…}` de la página.
fn objects_after(html: &str, key: &str, max: usize) -> Vec<Json> {
    let needle = format!("\"{key}\":");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = html[from..].find(&needle) {
        let at = from + p + needle.len();
        from = at;
        if let Some((v, used)) = json::parse_prefix(&html[at..]) {
            from = at + used;
            out.push(v);
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

fn videos(html: &str) -> Vec<Video> {
    let mut out: Vec<Video> = Vec::new();
    for key in ["videoRenderer", "compactVideoRenderer", "gridVideoRenderer"] {
        for v in objects_after(html, key, 60) {
            let Some(id) = v.get("videoId").and_then(Json::str) else {
                continue;
            };
            if out.iter().any(|x| x.id == id) {
                continue;
            }
            out.push(Video {
                id: id.to_string(),
                title: yt_text(v.get("title")),
                channel: yt_text(
                    v.get("ownerText")
                        .or_else(|| v.get("longBylineText"))
                        .or_else(|| v.get("shortBylineText")),
                ),
                views: yt_text(
                    v.get("shortViewCountText")
                        .or_else(|| v.get("viewCountText")),
                ),
                length: yt_text(v.get("lengthText")),
                published: yt_text(v.get("publishedTimeText")),
            });
        }
    }
    out
}

const YT_CSS: &str = "<style>\
body{margin:0;background:#fff;color:#0f0f0f;font-family:Roboto,Arial,sans-serif;font-size:14px}\
.top{display:flex;align-items:center;gap:24px;padding:10px 24px;border-bottom:1px solid #e5e5e5}\
.logo{display:flex;align-items:center;gap:6px;font-size:20px;font-weight:bold;letter-spacing:-1px;color:#0f0f0f;text-decoration:none}\
.play{background:#ff0000;color:#fff;border-radius:6px;padding:0 9px;font-size:13px;line-height:22px}\
form{display:flex;flex:1;max-width:640px;margin:0 auto}\
input[name=search_query]{flex:1;font-size:16px;padding:8px 14px;border:1px solid #ccc;border-radius:20px 0 0 20px}\
button{padding:8px 20px;border:1px solid #d3d3d3;border-left:0;background:#f8f8f8;border-radius:0 20px 20px 0;font-size:14px}\
.aviso{margin:14px 24px;padding:10px 14px;background:#f2f2f2;border-radius:10px;color:#606060;font-size:13px}\
.grilla{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:24px 16px;padding:8px 24px 40px}\
.video a{color:#0f0f0f;text-decoration:none}\
.mini{position:relative}\
.mini img{width:100%;border-radius:12px;display:block}\
.largo{position:absolute;right:8px;bottom:8px;background:#000;color:#fff;font-size:12px;font-weight:bold;padding:1px 4px;border-radius:4px}\
.titulo{font-size:16px;font-weight:bold;line-height:22px;margin:10px 0 4px}\
.canal{color:#606060;font-size:14px;line-height:20px}\
.ver{max-width:1000px;padding:20px 24px}\
.ver img{width:100%;border-radius:12px}\
.ver h1{font-size:20px;line-height:28px;margin:12px 0 6px}\
.desc{background:#f2f2f2;border-radius:12px;padding:12px;white-space:pre-wrap;font-size:14px;line-height:20px;margin-top:12px}\
</style>";

fn header(query: &str) -> String {
    format!(
        "{YT_CSS}<div class=top><a class=logo href='https://www.youtube.com/'><span class=play>▶</span>YouTube</a>\
         <form action='https://www.youtube.com/results'><input name=search_query value=\"{}\" \
         placeholder='Buscar'><button>Buscar</button></form></div>",
        esc(query)
    )
}

const NOTE: &str = "<div class=aviso>YouTube se arma con JavaScript, que JARVIS-OS todavía no \
ejecuta: esta es una versión simple hecha con los datos que trae la página. Los videos no se \
reproducen todavía (falta decodificar video y audio).</div>";

fn grid(vs: &[Video]) -> String {
    let mut s = String::from("<div class=grilla>");
    for v in vs.iter().take(40) {
        let len = if v.length.is_empty() {
            String::new()
        } else {
            format!("<span class=largo>{}</span>", esc(&v.length))
        };
        let meta: Vec<&str> = [v.views.as_str(), v.published.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect();
        s.push_str(&format!(
            "<div class=video><a href='https://www.youtube.com/watch?v={id}'>\
             <div class=mini><img src='https://i.ytimg.com/vi/{id}/mqdefault.jpg' width=320 height=180 alt=''>{len}</div>\
             <div class=titulo>{t}</div></a><div class=canal>{c}</div><div class=canal>{m}</div></div>",
            id = esc(&v.id),
            t = esc(&v.title),
            c = esc(&v.channel),
            m = esc(&meta.join(" · ")),
        ));
    }
    s.push_str("</div>");
    s
}

fn youtube(u: &Url, html: &str) -> Option<String> {
    let path = u.path_only();
    if path.starts_with("/watch") {
        let id = u.query_param("v")?;
        let player = html
            .find("ytInitialPlayerResponse = ")
            .and_then(|p| json::parse_prefix(&html[p + "ytInitialPlayerResponse = ".len()..]))
            .map(|(v, _)| v);
        let details = player.as_ref().and_then(|p| p.get("videoDetails"));
        let title = details
            .and_then(|d| d.get("title"))
            .and_then(Json::str)
            .unwrap_or("Video de YouTube")
            .to_string();
        let author = details
            .and_then(|d| d.get("author"))
            .and_then(Json::str)
            .unwrap_or("")
            .to_string();
        let views = details
            .and_then(|d| d.get("viewCount"))
            .and_then(Json::str)
            .map(|v| format!("{} vistas", group(v)))
            .unwrap_or_default();
        let secs: u32 = details
            .and_then(|d| d.get("lengthSeconds"))
            .and_then(Json::str)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let desc = details
            .and_then(|d| d.get("shortDescription"))
            .and_then(Json::str)
            .unwrap_or("")
            .to_string();
        let related = videos(html);
        let mut s = format!(
            "<title>{} - YouTube</title>{}{NOTE}<div class=ver>\
             <img src='https://i.ytimg.com/vi/{}/hqdefault.jpg' width=480 height=360 alt=''>\
             <h1>{}</h1><div class=canal><b>{}</b> · {} · {}:{:02}</div><div class=desc>{}</div></div>",
            esc(&title),
            header(""),
            esc(&id),
            esc(&title),
            esc(&author),
            views,
            secs / 60,
            secs % 60,
            esc(&desc)
        );
        if !related.is_empty() {
            s.push_str("<h2 style='margin:8px 24px;font-size:18px'>Más videos</h2>");
            s.push_str(&grid(&related));
        }
        return Some(s);
    }
    let query = if path.starts_with("/results") {
        u.query_param("search_query")
            .or_else(|| u.query_param("q"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let vs = videos(html);
    // Otra página de YouTube que no conocemos y que igual tiene contenido: se deja.
    if vs.is_empty() && !(path == "/" || path.starts_with("/results") || path.is_empty()) {
        return None;
    }
    let title = if query.is_empty() {
        "YouTube".to_string()
    } else {
        format!("{} - YouTube", esc(&query))
    };
    let body = if vs.is_empty() {
        "<div class=aviso>Escribí arriba lo que quieras buscar.</div>".to_string()
    } else {
        grid(&vs)
    };
    Some(format!(
        "<title>{title}</title>{}{NOTE}{body}",
        header(&query)
    ))
}

/// `1234567` → `1.234.567`.
fn group(n: &str) -> String {
    let digits: Vec<char> = n.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_busqueda_y_video() {
        let search = r#"<html><script>var ytInitialData = {"contents":[{"videoRenderer":{"videoId":"abc123","title":{"runs":[{"text":"Un kernel en Rust"}]},"ownerText":{"runs":[{"text":"Canal"}]},"viewCountText":{"simpleText":"1.234 vistas"},"lengthText":{"simpleText":"10:05"}}}]};</script></html>"#;
        let u = Url::parse("https://www.youtube.com/results?search_query=rust+kernel").unwrap();
        let out = adapt(Some(&u), search.into());
        assert!(out.contains("watch?v=abc123"), "{out}");
        assert!(out.contains("Un kernel en Rust"));
        assert!(out.contains("i.ytimg.com/vi/abc123/mqdefault.jpg"));
        assert!(out.contains("value=\"rust kernel\""));
        let watch = r#"<script>var ytInitialPlayerResponse = {"videoDetails":{"videoId":"abc123","title":"Título <b>","author":"Canal","viewCount":"1234567","lengthSeconds":"605","shortDescription":"Hola\nmundo"}};</script>"#;
        let u = Url::parse("https://www.youtube.com/watch?v=abc123").unwrap();
        let out = adapt(Some(&u), watch.into());
        assert!(out.contains("Título &lt;b&gt;"));
        assert!(out.contains("1.234.567 vistas"));
        assert!(out.contains("10:05"));
        // Otros sitios no se tocan.
        let u = Url::parse("https://example.com/").unwrap();
        assert_eq!(adapt(Some(&u), "<p>x".into()), "<p>x");
    }
}
