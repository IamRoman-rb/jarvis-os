//! Paquetes de Debian para `apt`: los programas de Linux de verdad (con glibc), bajados del
//! repositorio oficial de Debian estable.
//!
//! - **El índice**: `dists/stable/main/binary-amd64/Packages.xz` (unos 10 MB comprimidos, 50 MB
//!   de texto). Se descomprime **por partes** ([`IndexBuilder`]) y se guarda resumido, un paquete
//!   por línea ([`INDEX`]): nombre, versión, dependencias, lo que provee, dónde está el `.deb`,
//!   su tamaño y la descripción.
//! - **Las dependencias**: [`plan`] sigue `Pre-Depends` y `Depends` (la primera alternativa de
//!   `a | b`, sin las versiones) y los paquetes virtuales (`Provides`). Los que solo sirven para
//!   los scripts de instalación de Debian (`debconf`, `dpkg`…) se saltean: JARVIS-OS no los corre.
//! - **El `.deb`**: un archivo `ar` con `debian-binary`, `control.tar.xz` y `data.tar.xz`.
//!   [`unpack`] saca los archivos de `data.tar.xz` (XZ con `xz4rust`; el `tar` acá). FAT32 no
//!   tiene enlaces simbólicos: un enlace se convierte en una **copia** del archivo al que apunta.
//!   La documentación, los manuales y las traducciones no se instalan (ocupan y no se usan).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const MIRROR: &str = "http://deb.debian.org/debian";
pub const PACKAGES: &str =
    "http://deb.debian.org/debian/dists/stable/main/binary-amd64/Packages.xz";
/// El índice de Debian resumido (lo arma `apt update`).
pub const INDEX: &str = "/Sistema/paquetes/debian.txt";
/// Un `.deb` o un índice no puede descomprimirse a más que esto.
pub const MAX_UNPACKED: usize = 256 * 1024 * 1024;

/// Paquetes que solo hacen falta para los scripts de instalación de Debian (o que son el
/// sistema base de Debian): JARVIS-OS no los corre ni los necesita.
const SKIP: &[&str] = &[
    "adduser",
    "base-files",
    "base-passwd",
    "debconf",
    "debconf-2.0",
    "debianutils",
    "dpkg",
    "init-system-helpers",
    "install-info",
    "libc-bin",
    "lsb-base",
    "perl-base",
    "sensible-utils",
    "sysvinit-utils",
    "ucf",
];

/// Carpetas de un paquete que no se instalan.
const SKIP_DIRS: &[&str] = &[
    "/usr/share/doc/",
    "/usr/share/man/",
    "/usr/share/info/",
    "/usr/share/locale/",
    "/usr/share/lintian/",
    "/usr/share/bug/",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebPkg {
    pub name: String,
    pub version: String,
    /// Los nombres de lo que necesita (`Pre-Depends` y `Depends`).
    pub depends: Vec<String>,
    /// Los paquetes virtuales que ofrece.
    pub provides: Vec<String>,
    /// Dónde está el `.deb`, relativo a [`MIRROR`] (`pool/main/h/hello/hello_2.12_amd64.deb`).
    pub filename: String,
    pub size: u64,
    pub description: String,
}

/// Los nombres de un campo `Depends`: `"a (>= 1), b | c, d:any"` → `[a, b, d]`.
pub fn dep_names(field: &str) -> Vec<String> {
    field
        .split(',')
        .filter_map(|alt| {
            let first = alt.split('|').next()?.trim();
            let name = first.split([' ', '(', '[']).next()?.trim();
            let name = name.split(':').next()?.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Un párrafo del índice (`Campo: valor`, una línea por campo; las que empiezan con espacio
/// siguen al anterior y no hacen falta).
fn stanza(text: &str) -> Option<DebPkg> {
    let mut p = DebPkg::default();
    let mut arch = "";
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k {
            "Package" => p.name = v.to_string(),
            "Version" => p.version = v.to_string(),
            "Architecture" => arch = v,
            "Pre-Depends" | "Depends" => p.depends.extend(dep_names(v)),
            "Provides" => p.provides = dep_names(v),
            "Filename" => p.filename = v.to_string(),
            "Size" => p.size = v.parse().unwrap_or(0),
            "Description" => p.description = v.to_string(),
            _ => {}
        }
    }
    let ok = !p.name.is_empty() && !p.filename.is_empty() && matches!(arch, "amd64" | "all");
    ok.then_some(p)
}

fn clean(s: &str) -> String {
    s.replace(['|', '\n'], " ")
}

/// Una línea del índice resumido.
fn compact(p: &DebPkg) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}\n",
        p.name,
        p.version,
        p.depends.join(","),
        p.provides.join(","),
        p.filename,
        p.size,
        clean(&p.description)
    )
}

pub fn parse_compact(text: &str) -> Vec<DebPkg> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(7, '|').collect();
            if f.len() < 7 || f[0].is_empty() {
                return None;
            }
            let list = |s: &str| -> Vec<String> {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .map(String::from)
                    .collect()
            };
            Some(DebPkg {
                name: f[0].into(),
                version: f[1].into(),
                depends: list(f[2]),
                provides: list(f[3]),
                filename: f[4].into(),
                size: f[5].parse().unwrap_or(0),
                description: f[6].into(),
            })
        })
        .collect()
}

/// Arma el índice resumido a partir del texto de `Packages`, que llega de a pedazos.
#[derive(Default)]
pub struct IndexBuilder {
    line: Vec<u8>,
    stanza: String,
    pub out: String,
    pub count: usize,
}

impl IndexBuilder {
    pub fn feed(&mut self, data: &[u8]) {
        for &b in data {
            if b != b'\n' {
                self.line.push(b);
                continue;
            }
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            if line.trim().is_empty() {
                self.end_stanza();
            } else {
                self.stanza.push_str(&line);
                self.stanza.push('\n');
            }
        }
    }

    fn end_stanza(&mut self) {
        if let Some(p) = stanza(&self.stanza) {
            self.out.push_str(&compact(&p));
            self.count += 1;
        }
        self.stanza.clear();
    }

    pub fn finish(mut self) -> (String, usize) {
        if !self.line.is_empty() {
            self.feed(b"\n");
        }
        self.end_stanza();
        (self.out, self.count)
    }
}

// --- XZ, ar y tar ------------------------------------------------------------------------------

/// Descomprime XZ de a pedazos: `sink` recibe cada uno.
pub fn xz_decode(data: &[u8], max: usize, mut sink: impl FnMut(&[u8])) -> Result<usize, String> {
    // Diccionario de hasta 64 MiB (el nivel -9 de xz); Debian usa mucho menos.
    let mut dec: Box<xz4rust::XzDecoder<'static>> =
        xz4rust::XzDecoder::in_heap_with_alloc_dict_size(xz4rust::DICT_SIZE_MIN, 64 << 20);
    let mut buf = alloc::vec![0u8; 64 * 1024];
    let (mut at, mut total) = (0usize, 0usize);
    loop {
        match dec.decode(&data[at..], &mut buf) {
            Ok(xz4rust::XzNextBlockResult::NeedMoreData(used, made)) => {
                at += used;
                total += made;
                sink(&buf[..made]);
                if used == 0 && made == 0 {
                    return Err("XZ cortado".into());
                }
            }
            Ok(xz4rust::XzNextBlockResult::EndOfStream(_, made)) => {
                total += made;
                if total > max {
                    return Err("descomprimido es demasiado grande".into());
                }
                sink(&buf[..made]);
                return Ok(total);
            }
            Err(e) => return Err(format!("XZ dañado: {e:?}")),
        }
        if total > max {
            return Err("descomprimido es demasiado grande".into());
        }
    }
}

pub fn xz_to_vec(data: &[u8], max: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    xz_decode(data, max, |b| out.extend_from_slice(b))?;
    Ok(out)
}

/// Los miembros de un archivo `ar` (el formato de los `.deb`).
pub fn ar_members(data: &[u8]) -> Result<Vec<(String, &[u8])>, String> {
    if !data.starts_with(b"!<arch>\n") {
        return Err("no es un paquete .deb (falta la firma de ar)".into());
    }
    let mut out = Vec::new();
    let mut o = 8;
    while o + 60 <= data.len() {
        let h = &data[o..o + 60];
        let name = String::from_utf8_lossy(&h[..16])
            .trim()
            .trim_end_matches('/')
            .to_string();
        let size: usize = core::str::from_utf8(&h[48..58])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or("tamaño inválido en el .deb")?;
        let body = data
            .get(o + 60..o + 60 + size)
            .ok_or("el .deb está cortado")?;
        out.push((name, body));
        o += 60 + size + (size & 1);
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TarEntry {
    File(String, Vec<u8>),
    Dir(String),
    /// Un enlace simbólico: la ruta y a dónde apunta (tal cual).
    Symlink(String, String),
    /// Un enlace duro: la ruta y la del archivo (dentro del mismo tar).
    Hardlink(String, String),
}

fn octal(field: &[u8]) -> Option<u64> {
    // Base 256 (números grandes de GNU tar): el primer bit en 1.
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return Some(
            field[1..]
                .iter()
                .fold(0u64, |acc, &b| (acc << 8) | b as u64),
        );
    }
    let s = core::str::from_utf8(field).ok()?;
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

fn tar_str(field: &[u8]) -> String {
    let n = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..n]).into_owned()
}

/// Las entradas de un `tar` (ustar, con los nombres largos de GNU y de pax).
pub fn tar_entries(data: &[u8]) -> Result<Vec<TarEntry>, String> {
    let mut out = Vec::new();
    let mut o = 0;
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    while o + 512 <= data.len() {
        let h = &data[o..o + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = octal(&h[124..136]).ok_or("tamaño inválido en el tar")? as usize;
        let kind = h[156];
        let body = data
            .get(o + 512..o + 512 + size)
            .ok_or("el tar está cortado")?;
        o += 512 + size.div_ceil(512) * 512;
        let mut name = tar_str(&h[..100]);
        if &h[257..262] == b"ustar" {
            let prefix = tar_str(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let mut link = tar_str(&h[157..257]);
        if let Some(n) = long_name.take() {
            name = n;
        }
        if let Some(l) = long_link.take() {
            link = l;
        }
        match kind {
            b'L' => long_name = Some(tar_str(body)),
            b'K' => long_link = Some(tar_str(body)),
            b'x' => {
                // pax: registros "largo clave=valor\n".
                for rec in String::from_utf8_lossy(body).lines() {
                    if let Some((_, kv)) = rec.split_once(' ')
                        && let Some((k, v)) = kv.split_once('=')
                    {
                        match k {
                            "path" => long_name = Some(v.to_string()),
                            "linkpath" => long_link = Some(v.to_string()),
                            _ => {}
                        }
                    }
                }
            }
            b'g' => {}
            b'0' | 0 | b'7' => out.push(TarEntry::File(name, body.to_vec())),
            b'5' => out.push(TarEntry::Dir(name)),
            b'2' => out.push(TarEntry::Symlink(name, link)),
            b'1' => out.push(TarEntry::Hardlink(name, link)),
            _ => {} // dispositivos, FIFOs: no van en un paquete normal
        }
    }
    Ok(out)
}

/// `./usr/bin/hello` → `/usr/bin/hello` (sin `.`, `..` ni barras dobles).
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    let mut s = String::from("/");
    s.push_str(&parts.join("/"));
    s
}

/// ¿Un nombre que FAT32 acepta?
fn fat_ok(path: &str) -> bool {
    !path
        .chars()
        .any(|c| "<>:\"\\|?*".contains(c) || c.is_control())
}

/// Lo que hay que escribir en el disco para instalar un `.deb`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    pub files: Vec<(String, Vec<u8>)>,
    pub dirs: Vec<String>,
    /// Enlaces a archivos que no están en el paquete (de otro paquete, ya instalado o no):
    /// (dónde va la copia, el archivo al que apunta).
    pub links_out: Vec<(String, String)>,
    /// Lo que se salteó (documentación, nombres que FAT32 no acepta).
    pub skipped: usize,
}

/// Desarma un `.deb`.
pub fn unpack(deb: &[u8]) -> Result<Unpacked, String> {
    let members = ar_members(deb)?;
    let (name, data) = members
        .iter()
        .find(|(n, _)| n.starts_with("data.tar"))
        .ok_or("el .deb no trae data.tar")?;
    let tar = match name.as_str() {
        "data.tar.xz" => xz_to_vec(data, MAX_UNPACKED)?,
        "data.tar" => data.to_vec(),
        other => {
            return Err(format!(
                "viene comprimido como {other}, que JARVIS-OS todavía no sabe abrir"
            ));
        }
    };
    let mut out = Unpacked::default();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for e in tar_entries(&tar)? {
        match e {
            TarEntry::Dir(p) => {
                let p = normalize(&p);
                if p != "/" && !SKIP_DIRS.iter().any(|d| (p.clone() + "/").starts_with(d)) {
                    out.dirs.push(p);
                }
            }
            TarEntry::File(p, data) => {
                files.insert(normalize(&p), data);
            }
            TarEntry::Symlink(p, target) => {
                let p = normalize(&p);
                let base = p.rsplit_once('/').map_or("", |(d, _)| d);
                let t = if target.starts_with('/') {
                    normalize(&target)
                } else {
                    normalize(&format!("{base}/{target}"))
                };
                links.push((p, t));
            }
            TarEntry::Hardlink(p, target) => links.push((normalize(&p), normalize(&target))),
        }
    }
    // Los enlaces: una copia del archivo (siguiendo cadenas de enlaces dentro del paquete).
    for (p, mut t) in links.clone() {
        for _ in 0..8 {
            match links.iter().find(|(l, _)| *l == t) {
                Some((_, next)) => t = next.clone(),
                None => break,
            }
        }
        match files.get(&t).cloned() {
            Some(data) => {
                files.insert(p, data);
            }
            None => out.links_out.push((p, t)),
        }
    }
    for (p, data) in files {
        if SKIP_DIRS.iter().any(|d| p.starts_with(d)) || !fat_ok(&p) {
            out.skipped += 1;
            continue;
        }
        out.files.push((p, data));
    }
    out.links_out
        .retain(|(p, _)| !SKIP_DIRS.iter().any(|d| p.starts_with(d)) && fat_ok(p));
    Ok(out)
}

// --- dependencias ------------------------------------------------------------------------------

/// Qué hay que bajar para instalar `names` (las dependencias primero). `have`: lo instalado.
pub fn plan(
    index: &[DebPkg],
    have: &[(String, String)],
    names: &[String],
) -> Result<Vec<DebPkg>, String> {
    let by_name: BTreeMap<&str, &DebPkg> = index.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut providers: BTreeMap<&str, &DebPkg> = BTreeMap::new();
    for p in index {
        for v in &p.provides {
            providers.entry(v.as_str()).or_insert(p);
        }
    }
    let mut queue: Vec<DebPkg> = Vec::new();
    fn add<'a>(
        name: &str,
        explicit: bool,
        by_name: &BTreeMap<&str, &'a DebPkg>,
        providers: &BTreeMap<&str, &'a DebPkg>,
        have: &[(String, String)],
        queue: &mut Vec<DebPkg>,
        seen: &mut Vec<String>,
    ) -> Result<(), String> {
        if (!explicit && SKIP.contains(&name)) || seen.iter().any(|s| s == name) {
            return Ok(());
        }
        seen.push(name.to_string());
        let Some(p) = by_name.get(name).or_else(|| providers.get(name)) else {
            return if explicit {
                Err(format!("No se ha podido localizar el paquete {name}"))
            } else {
                Ok(()) // una dependencia que Debian ya no tiene: se sigue sin ella
            };
        };
        if have.iter().any(|(n, _)| *n == p.name) {
            return Ok(());
        }
        for d in &p.depends {
            add(d, false, by_name, providers, have, queue, seen)?;
        }
        if !queue.iter().any(|q| q.name == p.name) {
            queue.push((*p).clone());
        }
        Ok(())
    }
    let mut seen = Vec::new();
    for n in names {
        add(n, true, &by_name, &providers, have, &mut queue, &mut seen)?;
    }
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const PACKAGES_TXT: &str = "Package: hello
Version: 2.10-3
Architecture: amd64
Depends: libc6 (>= 2.34)
Filename: pool/main/h/hello/hello_2.10-3_amd64.deb
Size: 53000
Description: example package based on GNU hello
 The GNU hello program produces a familiar, friendly greeting.

Package: libc6
Version: 2.41-12
Architecture: amd64
Depends: libgcc-s1
Pre-Depends: debconf | debconf-2.0
Filename: pool/main/g/glibc/libc6_2.41-12_amd64.deb
Size: 2900000
Description: GNU C Library: Shared libraries

Package: libgcc-s1
Version: 14.2.0-19
Architecture: amd64
Depends: gcc-14-base (= 14.2.0-19), libc6 (>= 2.35)
Provides: libgcc1 (= 1:14.2.0-19)
Filename: pool/main/g/gcc-14/libgcc-s1_14.2.0-19_amd64.deb
Size: 72000
Description: GCC support library

Package: gcc-14-base
Version: 14.2.0-19
Architecture: amd64
Filename: pool/main/g/gcc-14/gcc-14-base_14.2.0-19_amd64.deb
Size: 50000
Description: GCC, the GNU Compiler Collection (base package)

Package: solo-arm
Version: 1
Architecture: arm64
Filename: pool/x.deb
Size: 1
Description: no es para esta máquina

Package: usa-virtual
Version: 1
Architecture: all
Depends: libgcc1 | otra, falta-en-debian
Filename: pool/main/u/usa-virtual_1_all.deb
Size: 10
Description: depende de un paquete virtual
";

    fn index() -> Vec<DebPkg> {
        let mut b = IndexBuilder::default();
        // De a pedazos que cortan las líneas por la mitad.
        for chunk in PACKAGES_TXT.as_bytes().chunks(7) {
            b.feed(chunk);
        }
        let (text, n) = b.finish();
        assert_eq!(n, 5, "solo amd64 y all");
        parse_compact(&text)
    }

    #[test]
    fn indice_y_dependencias() {
        assert_eq!(
            dep_names("a (>= 1), b | c, d:any, e [amd64]"),
            ["a", "b", "d", "e"]
        );
        let idx = index();
        let hello = idx.iter().find(|p| p.name == "hello").unwrap();
        assert_eq!(hello.depends, ["libc6"]);
        assert_eq!(hello.filename, "pool/main/h/hello/hello_2.10-3_amd64.deb");
        assert_eq!(hello.description, "example package based on GNU hello");
        let libc = idx.iter().find(|p| p.name == "libc6").unwrap();
        assert_eq!(libc.depends, ["libgcc-s1", "debconf"]);

        // hello trae libc6, que trae libgcc-s1 y gcc-14-base (debconf se saltea); las
        // dependencias van primero.
        let q = plan(&idx, &[], &["hello".into()]).unwrap();
        let names: Vec<&str> = q.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["gcc-14-base", "libgcc-s1", "libc6", "hello"]);
        // Con libc6 instalado, solo hello.
        let q = plan(
            &idx,
            &[("libc6".into(), "2.41-12".into())],
            &["hello".into()],
        )
        .unwrap();
        assert_eq!(q.len(), 1);
        // Un paquete virtual lo resuelve quien lo provee; una dependencia que no existe no corta.
        let q = plan(&idx, &[], &["usa-virtual".into()]).unwrap();
        assert!(q.iter().any(|p| p.name == "libgcc-s1"));
        assert!(plan(&idx, &[], &["no-existe".into()]).is_err());
    }

    /// Un tar con un archivo, una carpeta, un enlace relativo, uno absoluto y uno que sale del
    /// paquete, más un archivo de documentación.
    fn tar() -> Vec<u8> {
        let mut t = Vec::new();
        let mut entry = |name: &str, kind: u8, link: &str, body: &[u8]| {
            let mut h = vec![0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", body.len());
            h[124..136].copy_from_slice(size.as_bytes());
            h[156] = kind;
            h[157..157 + link.len()].copy_from_slice(link.as_bytes());
            h[257..263].copy_from_slice(b"ustar\0");
            t.extend_from_slice(&h);
            t.extend_from_slice(body);
            t.resize(t.len().div_ceil(512) * 512, 0);
        };
        entry("./usr/bin/", b'5', "", b"");
        entry("./usr/bin/hola", b'0', "", b"#!programa");
        entry("./usr/bin/hola-rel", b'2', "hola", b"");
        entry(
            "./usr/lib/x86_64-linux-gnu/libx.so.1",
            b'2',
            "/usr/bin/hola",
            b"",
        );
        entry(
            "./usr/lib64/ld-linux-x86-64.so.2",
            b'2',
            "../lib/afuera.so",
            b"",
        );
        entry("./usr/share/doc/hola/copyright", b'0', "", b"GPL");
        t.extend_from_slice(&[0u8; 1024]);
        t
    }

    #[test]
    fn tar_y_enlaces() {
        let e = tar_entries(&tar()).unwrap();
        assert_eq!(
            e[1],
            TarEntry::File("./usr/bin/hola".into(), b"#!programa".to_vec())
        );
        assert_eq!(
            e[2],
            TarEntry::Symlink("./usr/bin/hola-rel".into(), "hola".into())
        );
        // Un .deb sin comprimir (data.tar), en un ar.
        let tar = tar();
        let mut deb = b"!<arch>\n".to_vec();
        for (name, body) in [("debian-binary", &b"2.0\n"[..]), ("data.tar", &tar[..])] {
            let h = format!(
                "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                0,
                0,
                0,
                644,
                body.len()
            );
            deb.extend_from_slice(h.as_bytes());
            deb.extend_from_slice(body);
            if body.len() % 2 == 1 {
                deb.push(b'\n');
            }
        }
        let u = unpack(&deb).unwrap();
        let names: Vec<&str> = u.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            names,
            [
                "/usr/bin/hola",
                "/usr/bin/hola-rel",
                "/usr/lib/x86_64-linux-gnu/libx.so.1"
            ]
        );
        assert!(
            u.files.iter().all(|(_, d)| d == b"#!programa"),
            "los enlaces son copias"
        );
        assert_eq!(
            u.links_out,
            [(
                "/usr/lib64/ld-linux-x86-64.so.2".into(),
                "/usr/lib/afuera.so".into()
            )]
        );
        assert_eq!(u.skipped, 1, "la documentación no se instala");
        assert_eq!(u.dirs, ["/usr/bin"]);
        assert!(unpack(b"no es un deb").is_err());
    }

    #[test]
    fn xz() {
        // "Hola desde Debian\n" comprimido con `xz -6` (CRC64).
        const XZ: &[u8] = &[
            0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 0x00, 0x04, 0xe6, 0xd6, 0xb4, 0x46, 0x02, 0x00,
            0x21, 0x01, 0x16, 0x00, 0x00, 0x00, 0x74, 0x2f, 0xe5, 0xa3, 0x01, 0x00, 0x11, 0x48,
            0x6f, 0x6c, 0x61, 0x20, 0x64, 0x65, 0x73, 0x64, 0x65, 0x20, 0x44, 0x65, 0x62, 0x69,
            0x61, 0x6e, 0x0a, 0x00, 0x00, 0x00, 0x76, 0xef, 0xb8, 0x31, 0x45, 0x4c, 0xf4, 0x02,
            0x00, 0x01, 0x2a, 0x12, 0x4b, 0x08, 0x54, 0xbc, 0x1f, 0xb6, 0xf3, 0x7d, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x04, 0x59, 0x5a,
        ];
        assert_eq!(xz_to_vec(XZ, 1 << 20).unwrap(), b"Hola desde Debian\n");
        assert!(xz_to_vec(&XZ[..30], 1 << 20).is_err());
        assert!(xz_to_vec(XZ, 4).is_err(), "con tope");
    }
}
