//! Instalar un `.snap` de verdad (ADR 0021): un squashfs con un programa de Linux adentro.
//!
//! Lo que hace `snapd` en Ubuntu, en chico:
//! 1. abrir el squashfs (`jarvis-squashfs`) y leer `meta/snap.yaml` (nombre, versión, `base`,
//!    `confinement` y las `apps` con su `command` y su `environment`);
//! 2. copiar todo a `/snap/<nombre>/<revisión>/` (snapd lo *monta*; JARVIS-OS no tiene montajes
//!    de squashfs, así que lo copia al FAT32). Los enlaces simbólicos se copian como archivos,
//!    como con los `.deb`: FAT32 no tiene enlaces;
//! 3. un **lanzador** por app en `/snap/bin` (`hello`, y `hello.universe` para las otras) con las
//!    variables que todo snap espera (`SNAP`, `SNAP_NAME`, `SNAP_REVISION`, `SNAP_DATA`,
//!    `SNAP_USER_DATA`…) y `LD_LIBRARY_PATH` con las bibliotecas que trae el snap.
//!
//! **La base** (`core20`, `core22`…) es un Ubuntu mínimo con glibc que snapd monta como `/`
//! del snap. JARVIS-OS no la baja (pesa más que el tope de descarga): los programas usan la
//! glibc de Debian que instala `apt` (más nueva que la de las bases, así que la aceptan). Si no
//! está, se dice cómo instalarla.
//!
//! El `command-chain` (scripts de snapcraft que preparan el entorno) no se corre: el lanzador
//! llama directo al programa.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_squashfs::{Kind, SquashFs};

use super::apt::ensure_dirs;
use crate::apps::Ctx;

/// La primera línea de un lanzador de snap (la terminal lo reconoce y no lo corre como script).
pub const LAUNCHER_MAGIC: &str = "#!lanzador";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub command: String,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapYaml {
    pub name: String,
    pub version: String,
    pub summary: String,
    pub base: String,
    pub confinement: String,
    pub apps: Vec<App>,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let v = v
        .strip_prefix('\'')
        .and_then(|x| x.strip_suffix('\''))
        .or_else(|| v.strip_prefix('"').and_then(|x| x.strip_suffix('"')))
        .unwrap_or(v);
    v.to_string()
}

/// `meta/snap.yaml`, lo que importa. Es YAML con una forma conocida: claves de arriba y
/// `apps:` con un mapa por app (sangría de 2 y 4 espacios).
pub fn parse_snap_yaml(text: &str) -> SnapYaml {
    let mut y = SnapYaml::default();
    let mut in_apps = false;
    let mut app_indent = 0usize;
    let mut in_env = false;
    let mut env_indent = 0usize;
    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = line.len() - t.len();
        if indent == 0 {
            in_apps = t == "apps:";
            in_env = false;
            if let Some((k, v)) = t.split_once(':') {
                let v = unquote(v);
                match k {
                    "name" => y.name = v,
                    "version" => y.version = v,
                    "summary" => y.summary = v,
                    "base" => y.base = v,
                    "confinement" => y.confinement = v,
                    _ => {}
                }
            }
            continue;
        }
        if !in_apps {
            continue;
        }
        let Some((k, v)) = t.split_once(':') else {
            continue;
        };
        if y.apps.is_empty() || indent <= app_indent {
            // Una app nueva (`  hello:`).
            app_indent = indent;
            in_env = false;
            y.apps.push(App {
                name: k.trim().to_string(),
                ..App::default()
            });
            continue;
        }
        let Some(app) = y.apps.last_mut() else {
            continue;
        };
        if in_env && indent > env_indent {
            app.env.push((k.trim().to_string(), unquote(v)));
            continue;
        }
        in_env = false;
        match k.trim() {
            "command" => app.command = unquote(v),
            "environment" => {
                in_env = true;
                env_indent = indent;
            }
            _ => {}
        }
    }
    y
}

/// Expande `$VAR` y `${VAR}` con `vars` (lo que snapd hace con `command` y `environment`).
pub fn expand(s: &str, vars: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphanumeric() || n == '_' {
                name.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if braced && chars.peek() == Some(&'}') {
            chars.next();
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        if let Some((_, v)) = vars.iter().rev().find(|(k, _)| *k == name) {
            out.push_str(v);
        }
    }
    out
}

/// Lo que dice un lanzador: variables, el programa y sus argumentos fijos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub env: Vec<(String, String)>,
    pub exec: String,
    pub args: Vec<String>,
}

/// El texto de un lanzador: una línea por dato, sin comillas que interpretar.
pub fn launcher_text(snap: &str, version: &str, rev: u32, launch: &Launch) -> String {
    let mut s = format!(
        "{LAUNCHER_MAGIC}\n# snap {snap} {version} (revisión {rev}): lo generó `snap install`.\n"
    );
    for (k, v) in &launch.env {
        s.push_str(&format!("env {k}={v}\n"));
    }
    s.push_str(&format!("exec {}\n", launch.exec));
    for a in &launch.args {
        s.push_str(&format!("arg {a}\n"));
    }
    s
}

pub fn parse_launcher(text: &str) -> Option<Launch> {
    let mut lines = text.lines();
    if lines.next()?.trim() != LAUNCHER_MAGIC {
        return None;
    }
    let mut l = Launch {
        env: Vec::new(),
        exec: String::new(),
        args: Vec::new(),
    };
    for line in lines {
        if let Some(kv) = line.strip_prefix("env ") {
            let (k, v) = kv.split_once('=')?;
            l.env.push((k.to_string(), v.to_string()));
        } else if let Some(p) = line.strip_prefix("exec ") {
            l.exec = p.trim().to_string();
        } else if let Some(a) = line.strip_prefix("arg ") {
            l.args.push(a.to_string());
        }
    }
    (l.exec.starts_with('/')).then_some(l)
}

/// Las variables de un snap (como las pone snapd).
pub fn snap_env(name: &str, version: &str, rev: u32, arch_libs: bool) -> Vec<(String, String)> {
    let snap = format!("/snap/{name}/{rev}");
    let mut v = alloc::vec![
        ("SNAP".to_string(), snap.clone()),
        ("SNAP_NAME".into(), name.to_string()),
        ("SNAP_INSTANCE_NAME".into(), name.to_string()),
        ("SNAP_VERSION".into(), version.to_string()),
        ("SNAP_REVISION".into(), rev.to_string()),
        ("SNAP_ARCH".into(), "amd64".into()),
        ("SNAP_DATA".into(), format!("/var/snap/{name}/{rev}")),
        ("SNAP_COMMON".into(), format!("/var/snap/{name}/common")),
        ("SNAP_USER_DATA".into(), format!("/.snap/{name}/{rev}")),
        ("SNAP_USER_COMMON".into(), format!("/.snap/{name}/common")),
    ];
    if arch_libs {
        v.push((
            "LD_LIBRARY_PATH".into(),
            format!(
                "{snap}/lib:{snap}/usr/lib:{snap}/lib/x86_64-linux-gnu:{snap}/usr/lib/x86_64-linux-gnu"
            ),
        ));
    }
    v
}

/// Lo que dejó instalado.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installed {
    pub yaml: SnapYaml,
    pub rev: u32,
    pub files: usize,
    pub bytes: u64,
    pub commands: Vec<String>,
    /// Enlaces que apuntaban fuera del snap (o a carpetas): no se pudieron copiar.
    pub skipped_links: usize,
}

/// Instala la imagen `data` como revisión `rev`. `classic`: se pidió `--classic`.
pub fn install<D: BlockDevice>(
    data: &[u8],
    rev: u32,
    classic: bool,
    ctx: &mut Ctx<'_, D>,
) -> Result<Installed, String> {
    let fs_img = SquashFs::open(data).map_err(|e| format!("no es un snap válido: {e}"))?;
    let entries = fs_img.entries().map_err(|e| format!("snap dañado: {e}"))?;
    let yaml_entry = fs_img
        .find(&entries, "/meta/snap.yaml")
        .ok_or("al snap le falta meta/snap.yaml")?;
    let yaml_bytes = fs_img.read(yaml_entry).map_err(|e| e.to_string())?;
    let yaml = parse_snap_yaml(&String::from_utf8_lossy(&yaml_bytes));
    if yaml.name.is_empty()
        || !yaml
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("nombre de snap inválido: «{}»", yaml.name));
    }
    if yaml.confinement == "classic" && !classic {
        return Err(format!(
            "el snap {} usa confinamiento \"classic\": se instala con --classic (puede leer y \
             cambiar todo lo que el usuario puede)",
            yaml.name
        ));
    }
    let now = ctx.timestamp();
    let root = format!("{}/{}/{rev}", super::snap::ROOT, yaml.name);
    let mut result = Installed {
        yaml: yaml.clone(),
        rev,
        ..Installed::default()
    };
    // Primero los archivos y las carpetas, después los enlaces (que copian lo que apuntan).
    let mut written: Vec<(String, Vec<u8>, u16)> = Vec::new();
    for e in &entries {
        if e.path == "/" {
            continue;
        }
        if let Kind::File = e.kind {
            let bytes = fs_img.read(e).map_err(|x| x.to_string())?;
            written.push((e.path.clone(), bytes, e.mode));
        }
    }
    for e in &entries {
        if let Kind::Symlink(target) = &e.kind {
            let Some(real) = jarvis_squashfs::resolve_link(&e.path, target) else {
                result.skipped_links += 1;
                continue;
            };
            // Hasta 8 saltos (un enlace a un enlace…).
            let mut cur = real;
            let mut found = None;
            for _ in 0..8 {
                if let Some((_, b, m)) = written.iter().find(|(p, _, _)| *p == cur) {
                    found = Some((b.clone(), *m));
                    break;
                }
                match entries.iter().find(|x| x.path == cur).map(|x| &x.kind) {
                    Some(Kind::Symlink(t)) => match jarvis_squashfs::resolve_link(&cur, t) {
                        Some(n) => cur = n,
                        None => break,
                    },
                    _ => break,
                }
            }
            match found {
                Some((b, m)) => written.push((e.path.clone(), b, m)),
                None => result.skipped_links += 1,
            }
        }
    }
    // Lo anterior de esta revisión (una reinstalación) va a la Papelera (regla 17).
    {
        let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
        if fs.exists(&root) {
            let _ = crate::files::FilesApp::move_to_trash(fs, &root, now);
        }
        ensure_dirs(fs, &root, now).map_err(|x| x.to_string())?;
        for e in &entries {
            if matches!(e.kind, Kind::Dir) && e.path != "/" {
                ensure_dirs(fs, &format!("{root}{}", e.path), now).map_err(|x| x.to_string())?;
            }
        }
        for (path, bytes, _) in &written {
            let dest = format!("{root}{path}");
            ensure_dirs(fs, &crate::files::parent(&dest), now).map_err(|x| x.to_string())?;
            fs.write_file(&dest, bytes, now)
                .map_err(|x| format!("{dest}: {x}"))?;
            result.files += 1;
            result.bytes += bytes.len() as u64;
        }
    }
    // El antivirus revisa cada archivo.
    for (path, bytes, _) in &written {
        let dest = format!("{root}{path}");
        if let Some(msg) = crate::antivirus::guard(ctx, &dest, bytes) {
            return Err(msg);
        }
    }
    // Los permisos que trae el snap (los ejecutables que no están en una carpeta bin).
    {
        let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
        ctx.perms.ensure_loaded(fs);
        for (path, _, mode) in &written {
            let dest = format!("{root}{path}");
            if crate::perms::Perms::default_for(&dest, false).mode != *mode {
                ctx.perms.install_file(&dest, *mode);
            }
        }
        let _ = ctx.perms.save(fs, now);
    }
    // Los lanzadores y las carpetas de datos.
    let fs = ctx.fs.as_deref_mut().ok_or("no hay disco")?;
    ensure_dirs(fs, super::snap::BIN, now).map_err(|x| x.to_string())?;
    for dir in [
        format!("/var/snap/{}/{rev}", yaml.name),
        format!("/var/snap/{}/common", yaml.name),
        format!("/.snap/{}/{rev}", yaml.name),
        format!("/.snap/{}/common", yaml.name),
    ] {
        let _ = ensure_dirs(fs, &dir, now);
    }
    let base_env = snap_env(&yaml.name, &yaml.version, rev, true);
    for app in &yaml.apps {
        if app.command.trim().is_empty() {
            continue;
        }
        let mut env = base_env.clone();
        for (k, v) in &app.env {
            let value = expand(v, &env);
            env.push((k.clone(), value));
        }
        // Como snap-exec: primero se parte en palabras y después se expande cada una (así una
        // variable con espacios queda como un solo argumento).
        let words: Vec<String> = app
            .command
            .split_whitespace()
            .map(|w| expand(w, &env))
            .collect();
        let Some((prog, rest)) = words.split_first() else {
            continue;
        };
        let prog = prog.as_str();
        let exec = if prog.starts_with('/') {
            // Una ruta absoluta es adentro del snap ($SNAP/... ya expandida o /bin/... de la base).
            if prog.starts_with(&root) {
                prog.to_string()
            } else {
                format!("{root}{prog}")
            }
        } else {
            format!("{root}/{prog}")
        };
        let launch = Launch {
            env,
            exec,
            args: rest.to_vec(),
        };
        let command = if app.name == yaml.name {
            yaml.name.clone()
        } else {
            format!("{}.{}", yaml.name, app.name)
        };
        let path = format!("{}/{command}", super::snap::BIN);
        fs.write_file(
            &path,
            launcher_text(&yaml.name, &yaml.version, rev, &launch).as_bytes(),
            now,
        )
        .map_err(|x| format!("{path}: {x}"))?;
        result.commands.push(command);
    }
    if result.commands.is_empty() {
        return Err(format!("el snap {} no tiene apps para ejecutar", yaml.name));
    }
    ctx.log.push(format!(
        "SNAP_INSTALADO {} {} {rev}",
        yaml.name, yaml.version
    ));
    Ok(result)
}

/// ¿Está la glibc de Debian (la que reemplaza a la base del snap)?
pub fn has_glibc<D: BlockDevice>(fs: &mut jarvis_fs::FileSystem<D>) -> bool {
    jarvis_linux::elf::interpreter_paths("/lib64/ld-linux-x86-64.so.2")
        .into_iter()
        .any(|p| fs.exists(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = r"name: hello
version: '2.10'
summary: GNU Hello
description: GNU hello prints a friendly greeting.
apps:
  hello:
    command: bin/hello
    command-chain:
    - snap/command-chain/snapcraft-runner
  universe:
    command: bin/hello -g $SAY
    environment:
      SAY: Hello, universe!
    command-chain:
    - snap/command-chain/snapcraft-runner
architectures:
- amd64
base: core20
confinement: strict
grade: stable
";

    #[test]
    fn snap_yaml_del_hello_real() {
        let y = parse_snap_yaml(HELLO);
        assert_eq!(y.name, "hello");
        assert_eq!(y.version, "2.10");
        assert_eq!(y.base, "core20");
        assert_eq!(y.confinement, "strict");
        assert_eq!(y.apps.len(), 2);
        assert_eq!(y.apps[0].name, "hello");
        assert_eq!(y.apps[0].command, "bin/hello");
        assert_eq!(y.apps[1].command, "bin/hello -g $SAY");
        assert_eq!(
            y.apps[1].env,
            alloc::vec![("SAY".to_string(), "Hello, universe!".to_string())]
        );
    }

    #[test]
    fn variables_y_lanzador() {
        let vars = alloc::vec![
            ("SNAP".to_string(), "/snap/x/3".to_string()),
            ("SAY".to_string(), "hola, mundo".to_string()),
        ];
        assert_eq!(
            expand("$SNAP/bin/x -g $SAY", &vars),
            "/snap/x/3/bin/x -g hola, mundo"
        );
        assert_eq!(expand("${SNAP}/a $NADA b", &vars), "/snap/x/3/a  b");
        let l = Launch {
            env: alloc::vec![("SAY".into(), "Hello, universe!".into())],
            exec: "/snap/hello/42/bin/hello".into(),
            args: alloc::vec!["-g".into(), "Hello, universe!".into()],
        };
        let text = launcher_text("hello", "2.10", 42, &l);
        assert_eq!(parse_launcher(&text), Some(l));
        assert_eq!(parse_launcher("echo hola"), None);
        let env = snap_env("hello", "2.10", 42, true);
        assert!(env.contains(&("SNAP_REVISION".to_string(), "42".to_string())));
        assert!(
            env.iter()
                .any(|(k, v)| k == "LD_LIBRARY_PATH" && v.starts_with("/snap/hello/42/lib"))
        );
    }
}
