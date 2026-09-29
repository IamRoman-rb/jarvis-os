//! Los comandos de `jsh`: los de siempre de Linux (coreutils, grep, find…), los de red y los
//! propios de JARVIS-OS. Cada uno recibe sus argumentos y la entrada estándar, y escribe en la
//! salida (`o`) y en la de errores (`e`).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, DirEntry, FileSystem};

use super::regex::Regex;
use super::{BIN, Job, Out, PATH, Res, Shell, ansi, apt, binfmt, child, snap, winget};
use crate::apps::Ctx;
use crate::files::{FilesApp, basename, format_size, parent};
use crate::system::{AppKind, FetchKind, HttpResponse, Launch, Power};
use crate::web::url::Url;

/// Comandos que existen (para `help`, `which` y completar con Tab).
pub const NAMES: &[(&str, &str)] = &[
    ("alias", "define un alias: alias ll='ls -l'"),
    ("apt", "gestor de paquetes: apt install neofetch"),
    ("basename", "el nombre de una ruta, sin carpetas"),
    ("cal", "calendario del mes"),
    ("cat", "muestra archivos (-n numera las líneas)"),
    ("cd", "cambia de carpeta (cd .., cd -, cd ~)"),
    ("clear", "limpia la pantalla (Ctrl+L)"),
    ("cp", "copia archivos (-r: carpetas)"),
    ("curl", "baja una dirección a la pantalla (-o archivo)"),
    ("cut", "columnas: cut -d, -f2"),
    ("date", "fecha y hora"),
    ("df", "espacio del disco (-h)"),
    ("dirname", "la carpeta de una ruta"),
    ("dpkg", "dpkg -l: paquetes instalados"),
    ("du", "cuánto ocupa una carpeta (-s -h)"),
    ("echo", "escribe texto (-n sin salto, -e con escapes)"),
    ("env", "variables de entorno"),
    ("exit", "cierra la terminal"),
    ("export", "define una variable: export X=1"),
    ("expr", "cuentas: expr 2 + 3"),
    ("false", "sale con error"),
    ("file", "qué tipo de archivo es (reconoce .exe y ELF)"),
    ("find", "busca: find /Documentos -name '*.txt'"),
    ("free", "memoria (-h)"),
    (
        "grep",
        "busca texto (-i -v -n -c -r, expresiones regulares)",
    ),
    ("head", "primeras líneas (-n N)"),
    ("help", "esta ayuda"),
    ("history", "comandos anteriores"),
    ("hostname", "nombre del equipo"),
    ("id", "usuario y grupos"),
    ("ip", "dirección de red (ip a)"),
    ("jarvis", "JARVIS lo dice en voz alta: jarvis hola"),
    ("kill", "cierra una ventana por su PID (ver ps)"),
    ("ls", "lista una carpeta (-l -a -h -1)"),
    ("man", "ayuda de un comando"),
    ("mkdir", "crea carpetas (-p)"),
    ("mv", "mueve o renombra"),
    ("nano", "abre el editor de texto"),
    ("open", "abre con la app que corresponde (xdg-open)"),
    ("ping", "prueba si responde un sitio (por HTTP)"),
    ("printf", "escribe con formato"),
    ("ps", "procesos: tareas del kernel y ventanas"),
    ("pwd", "carpeta actual"),
    ("reboot", "reinicia"),
    ("rev", "da vuelta cada línea"),
    ("rm", "mueve a la Papelera (-r: carpetas)"),
    ("rmdir", "borra una carpeta vacía (a la Papelera)"),
    ("screenshot", "captura de pantalla"),
    ("sed", "reemplaza texto: sed 's/viejo/nuevo/g'"),
    ("seq", "números: seq 1 10"),
    ("sh", "ejecuta un script"),
    ("shutdown", "apaga (también poweroff)"),
    ("sleep", "espera N segundos"),
    (
        "snap",
        "snaps: snap find, snap install clima, snap refresh, snap revert",
    ),
    ("sort", "ordena líneas (-r -n -u)"),
    ("stat", "datos de un archivo"),
    ("strings", "texto adentro de un binario"),
    ("tail", "últimas líneas (-n N)"),
    ("tee", "copia la entrada a un archivo y a la salida"),
    ("test", "condiciones: test -f archivo (también [ ])"),
    ("top", "abre el monitor del sistema"),
    ("touch", "crea un archivo vacío"),
    ("tr", "cambia caracteres: tr a-z A-Z"),
    ("tree", "árbol de carpetas"),
    ("true", "sale bien"),
    ("type", "qué es un comando"),
    ("ufw", "firewall: ufw status, ufw deny out to sitio.com"),
    ("uname", "datos del sistema (-a)"),
    ("uniq", "saca líneas repetidas seguidas (-c)"),
    ("unset", "borra una variable"),
    ("uptime", "cuánto hace que está encendido"),
    ("wc", "cuenta líneas, palabras y bytes"),
    ("wget", "baja una dirección a un archivo (-O nombre)"),
    ("which", "dónde está un comando"),
    ("whoami", "tu usuario"),
    (
        "winget",
        "programas de Windows: winget search zip, winget install 7zip.7zip",
    ),
    ("xdg-open", "abre con la app que corresponde"),
    ("xxd", "volcado hexadecimal (-l N)"),
];

/// Opciones cortas (`-la` → `l`, `a`) y el resto de los argumentos.
fn opts(args: &[String]) -> (Vec<char>, Vec<String>) {
    let mut flags = Vec::new();
    let mut rest = Vec::new();
    let mut only_args = false;
    for a in args.iter().skip(1) {
        if only_args || !a.starts_with('-') || a.len() == 1 {
            rest.push(a.clone());
        } else if a == "--" {
            only_args = true;
        } else if let Some(long) = a.strip_prefix("--") {
            flags.push(match long {
                "all" => 'a',
                "human-readable" => 'h',
                "recursive" => 'r',
                "ignore-case" => 'i',
                "count" => 'c',
                "reverse" => 'r',
                _ => '?',
            });
        } else {
            flags.extend(a[1..].chars());
        }
    }
    (flags, rest)
}

/// `-n 5` o `-5` → 5.
fn count_arg(args: &[String], default: usize) -> (usize, Vec<String>) {
    let mut n = default;
    let mut rest = Vec::new();
    let mut it = args.iter().skip(1).peekable();
    while let Some(a) = it.next() {
        if a == "-n" {
            n = it.next().and_then(|v| v.parse().ok()).unwrap_or(default);
        } else if let Some(v) = a.strip_prefix("-n") {
            n = v.parse().unwrap_or(default);
        } else if a.len() > 1 && a.starts_with('-') && a[1..].chars().all(|c| c.is_ascii_digit()) {
            n = a[1..].parse().unwrap_or(default);
        } else {
            rest.push(a.clone());
        }
    }
    (n, rest)
}

fn lines_of(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split('\n').collect();
    if v.last() == Some(&"") {
        v.pop();
    }
    v
}

fn join_lines(v: &[String]) -> String {
    let mut s = v.join("\n");
    if !v.is_empty() {
        s.push('\n');
    }
    s
}

const MONTHS: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];
const DAYS: [&str; 7] = ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"];

impl Shell {
    fn fs<'a, D: BlockDevice>(
        ctx: &'a mut Ctx<'_, D>,
        e: &mut String,
    ) -> Option<&'a mut FileSystem<D>> {
        let fs = ctx.fs.as_deref_mut();
        if fs.is_none() {
            e.push_str("no hay disco\n");
        }
        fs
    }

    /// Lee los archivos de `files` (o la entrada estándar si no hay).
    fn inputs<D: BlockDevice>(
        &self,
        name: &str,
        files: &[String],
        stdin: Option<&str>,
        ctx: &mut Ctx<'_, D>,
        e: &mut String,
    ) -> Vec<(String, String)> {
        if files.is_empty() || files == ["-"] {
            return alloc::vec![(String::new(), stdin.unwrap_or("").to_string())];
        }
        let mut out = Vec::new();
        // /proc: archivos que no están en el disco, los arma el sistema (como en Linux).
        let (procs, files): (Vec<&String>, Vec<&String>) = files
            .iter()
            .partition(|f| self.abs(f).starts_with("/proc/"));
        for f in procs {
            match proc_file(&self.abs(f), ctx) {
                Some(text) => out.push((f.clone(), text)),
                None => e.push_str(&format!("{name}: {f}: no existe el archivo\n")),
            }
        }
        if files.is_empty() {
            return out;
        }
        let Some(fs) = Self::fs(ctx, e) else {
            return out;
        };
        for f in files {
            let p = self.abs(f);
            match fs.stat(&p) {
                Ok(st) if st.is_dir => e.push_str(&format!("{name}: {f}: es una carpeta\n")),
                Ok(_) => match fs.read_file(&p) {
                    Ok(b) => out.push((f.clone(), String::from_utf8_lossy(&b).into_owned())),
                    Err(err) => e.push_str(&format!("{name}: {f}: {err}\n")),
                },
                Err(err) => e.push_str(&format!("{name}: {f}: {err}\n")),
            }
        }
        out
    }

    pub(crate) fn exec<D: BlockDevice>(
        &mut self,
        argv: &[String],
        stdin: Option<&str>,
        o: &mut String,
        e: &mut String,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Res {
        let name = argv[0].as_str();
        // `VAR=valor` solo: define la variable.
        if argv.len() == 1
            && let Some((k, v)) = name.split_once('=')
            && !k.is_empty()
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.set_var(k, v);
            return Res::Code(0);
        }
        let code = match name {
            "help" | "ayuda" => {
                o.push_str("jsh, la shell de JARVIS-OS. Comandos:\n");
                for (n, d) in NAMES {
                    o.push_str(&format!("  {}{n:<10}{} {d}\n", ansi::CYAN, ansi::RESET));
                }
                o.push_str("También: tuberías (|), redirecciones (> >> < 2>), && || ;, $VAR, $(cmd), *.txt\n");
                o.push_str(
                    "Teclas: Tab completa · flechas: historial · Ctrl+C cancela · Ctrl+L limpia\n",
                );
                0
            }
            "man" => match argv.get(1).and_then(|c| NAMES.iter().find(|(n, _)| n == c)) {
                Some((n, d)) => {
                    o.push_str(&format!("{}{n}{}: {d}\n", ansi::BOLD, ansi::RESET));
                    0
                }
                None => {
                    e.push_str("¿De qué comando? Probá: man ls (o help para la lista)\n");
                    1
                }
            },
            "true" | ":" => 0,
            "false" => 1,
            "echo" => {
                let (mut newline, mut escapes, mut i) = (true, false, 1);
                while let Some(a) = argv.get(i) {
                    match a.as_str() {
                        "-n" => newline = false,
                        "-e" => escapes = true,
                        "-ne" | "-en" => {
                            newline = false;
                            escapes = true;
                        }
                        _ => break,
                    }
                    i += 1;
                }
                let text = argv[i.min(argv.len())..].join(" ");
                o.push_str(&if escapes { unescape(&text) } else { text });
                if newline {
                    o.push('\n');
                }
                0
            }
            "printf" => {
                let Some(fmt) = argv.get(1) else {
                    e.push_str("printf: falta el formato\n");
                    return Res::Code(1);
                };
                o.push_str(&printf(fmt, &argv[2..]));
                0
            }
            "pwd" => {
                o.push_str(&self.cwd);
                o.push('\n');
                0
            }
            "cd" => self.cd(argv.get(1).map(String::as_str), ctx, o, e),
            "ls" => self.ls(argv, ctx, o, e),
            "tree" => self.tree(argv, ctx, o, e),
            "cat" => {
                let (flags, files) = opts(argv);
                let mut n = 0;
                for (_, text) in self.inputs("cat", &files, stdin, ctx, e) {
                    if flags.contains(&'n') {
                        for l in lines_of(&text) {
                            n += 1;
                            o.push_str(&format!("{n:>6}  {l}\n"));
                        }
                    } else {
                        o.push_str(&text);
                    }
                }
                i32::from(!e.is_empty())
            }
            "head" | "tail" => {
                let (n, files) = count_arg(argv, 10);
                for (f, text) in self.inputs(name, &files, stdin, ctx, e) {
                    if files.len() > 1 {
                        o.push_str(&format!("==> {f} <==\n"));
                    }
                    let lines = lines_of(&text);
                    let pick: Vec<String> = if name == "head" {
                        lines.iter().take(n).map(|s| s.to_string()).collect()
                    } else {
                        lines[lines.len().saturating_sub(n)..]
                            .iter()
                            .map(|s| s.to_string())
                            .collect()
                    };
                    o.push_str(&join_lines(&pick));
                }
                i32::from(!e.is_empty())
            }
            "wc" => {
                let (flags, files) = opts(argv);
                let all = flags.is_empty();
                let inputs = self.inputs("wc", &files, stdin, ctx, e);
                // Como GNU wc: un solo número de una sola entrada va sin relleno.
                let single = !all && flags.len() == 1 && inputs.len() == 1;
                for (f, text) in inputs {
                    let mut counts = Vec::new();
                    if all || flags.contains(&'l') {
                        counts.push(text.matches('\n').count());
                    }
                    if all || flags.contains(&'w') {
                        counts.push(text.split_whitespace().count());
                    }
                    if all || flags.contains(&'c') {
                        counts.push(text.len());
                    }
                    let parts: Vec<String> = counts
                        .iter()
                        .map(|n| {
                            if single {
                                n.to_string()
                            } else {
                                format!("{n:>7}")
                            }
                        })
                        .collect();
                    o.push_str(&format!("{} {f}\n", parts.join(" ")).replace(" \n", "\n"));
                }
                i32::from(!e.is_empty())
            }
            "grep" => self.grep(argv, stdin, ctx, o, e),
            "sort" => {
                let (flags, files) = opts(argv);
                let mut lines: Vec<String> = self
                    .inputs("sort", &files, stdin, ctx, e)
                    .iter()
                    .flat_map(|(_, t)| {
                        lines_of(t)
                            .into_iter()
                            .map(String::from)
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if flags.contains(&'n') {
                    let num = |s: &str| {
                        s.split_whitespace()
                            .next()
                            .and_then(|n| n.parse::<f64>().ok())
                            .unwrap_or(0.0)
                    };
                    lines.sort_by(|a, b| {
                        num(a)
                            .partial_cmp(&num(b))
                            .unwrap_or(core::cmp::Ordering::Equal)
                    });
                } else {
                    lines.sort_by_key(|a| a.to_lowercase());
                }
                if flags.contains(&'r') {
                    lines.reverse();
                }
                if flags.contains(&'u') {
                    lines.dedup();
                }
                o.push_str(&join_lines(&lines));
                0
            }
            "uniq" => {
                let (flags, files) = opts(argv);
                for (_, text) in self.inputs("uniq", &files, stdin, ctx, e) {
                    let mut groups: Vec<(usize, &str)> = Vec::new();
                    for l in lines_of(&text) {
                        match groups.last_mut() {
                            Some((n, prev)) if *prev == l => *n += 1,
                            _ => groups.push((1, l)),
                        }
                    }
                    for (n, l) in groups {
                        if flags.contains(&'c') {
                            o.push_str(&format!("{n:>7} {l}\n"));
                        } else {
                            o.push_str(l);
                            o.push('\n');
                        }
                    }
                }
                0
            }
            "rev" => {
                let (_, files) = opts(argv);
                for (_, text) in self.inputs("rev", &files, stdin, ctx, e) {
                    for l in lines_of(&text) {
                        o.push_str(&l.chars().rev().collect::<String>());
                        o.push('\n');
                    }
                }
                0
            }
            "sed" => {
                let (flags, rest) = opts(argv);
                let Some(script) = rest.first() else {
                    e.push_str("sed: uso: sed 's/patrón/reemplazo/g' [archivo]\n");
                    return Res::Code(1);
                };
                let quiet = flags.contains(&'n');
                let cmd = match parse_sed(script, flags.contains(&'i')) {
                    Ok(c) => c,
                    Err(err) => {
                        e.push_str(&format!("sed: {err}\n"));
                        return Res::Code(1);
                    }
                };
                for (_, text) in self.inputs("sed", &rest[1..], stdin, ctx, e) {
                    for l in lines_of(&text) {
                        match &cmd {
                            SedCmd::Sub(re, rep, all) => {
                                let changed = re.replace(l, rep, *all);
                                if !quiet {
                                    o.push_str(&changed);
                                    o.push('\n');
                                }
                            }
                            SedCmd::Delete(re) => {
                                if !re.is_match(l) && !quiet {
                                    o.push_str(l);
                                    o.push('\n');
                                }
                            }
                            SedCmd::Print(re) => {
                                if re.is_match(l) || !quiet {
                                    o.push_str(l);
                                    o.push('\n');
                                }
                            }
                        }
                    }
                }
                0
            }
            "tr" => {
                let (flags, sets) = opts(argv);
                let text = stdin.unwrap_or("");
                let from = expand_set(sets.first().map(String::as_str).unwrap_or(""));
                if flags.contains(&'s') && sets.len() == 1 {
                    // Junta las repeticiones seguidas de los caracteres del conjunto.
                    let mut prev: Option<char> = None;
                    for c in text.chars() {
                        if Some(c) == prev && from.contains(&c) {
                            continue;
                        }
                        o.push(c);
                        prev = Some(c);
                    }
                } else if flags.contains(&'d') {
                    o.push_str(
                        &text
                            .chars()
                            .filter(|c| !from.contains(c))
                            .collect::<String>(),
                    );
                } else {
                    let to = expand_set(sets.get(1).map(String::as_str).unwrap_or(""));
                    o.push_str(
                        &text
                            .chars()
                            .map(|c| match from.iter().position(|&f| f == c) {
                                Some(i) => *to.get(i).or(to.last()).unwrap_or(&c),
                                None => c,
                            })
                            .collect::<String>(),
                    );
                }
                0
            }
            "cut" => {
                let mut delim = '\t';
                let mut ranges: Vec<(usize, usize)> = Vec::new();
                let mut by_char = false;
                let mut files = Vec::new();
                let mut it = argv.iter().skip(1);
                let parse_ranges = |spec: &str| -> Vec<(usize, usize)> {
                    spec.split(',')
                        .filter_map(|r| match r.split_once('-') {
                            Some((a, b)) => Some((
                                a.parse().unwrap_or(1),
                                if b.is_empty() {
                                    usize::MAX
                                } else {
                                    b.parse().ok()?
                                },
                            )),
                            None => r.parse().ok().map(|n| (n, n)),
                        })
                        .collect()
                };
                while let Some(a) = it.next() {
                    if let Some(d) = a.strip_prefix("-d") {
                        let d = if d.is_empty() {
                            it.next().map(String::as_str).unwrap_or("\t")
                        } else {
                            d
                        };
                        delim = d.chars().next().unwrap_or('\t');
                    } else if let Some(f) = a.strip_prefix("-f").or_else(|| a.strip_prefix("-c")) {
                        by_char = a.starts_with("-c");
                        let f = if f.is_empty() {
                            it.next().map(String::as_str).unwrap_or("1")
                        } else {
                            f
                        };
                        ranges = parse_ranges(f);
                    } else {
                        files.push(a.clone());
                    }
                }
                let wanted = |i: usize| ranges.iter().any(|&(a, b)| i >= a && i <= b);
                for (_, text) in self.inputs("cut", &files, stdin, ctx, e) {
                    for l in lines_of(&text) {
                        if by_char {
                            let s: String = l
                                .chars()
                                .enumerate()
                                .filter(|(i, _)| wanted(i + 1))
                                .map(|(_, c)| c)
                                .collect();
                            o.push_str(&s);
                        } else {
                            let parts: Vec<&str> = l.split(delim).collect();
                            let picked: Vec<&str> = parts
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| wanted(i + 1))
                                .map(|(_, p)| *p)
                                .collect();
                            o.push_str(&picked.join(&delim.to_string()));
                        }
                        o.push('\n');
                    }
                }
                0
            }
            "tee" => {
                let text = stdin.unwrap_or("").to_string();
                let (flags, files) = opts(argv);
                let now = ctx.timestamp();
                if let Some(fs) = Self::fs(ctx, e) {
                    for f in files {
                        let p = self.abs(&f);
                        let mut data = if flags.contains(&'a') {
                            fs.read_file(&p).unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        data.extend_from_slice(text.as_bytes());
                        if let Err(err) = fs.write_file(&p, &data, now) {
                            e.push_str(&format!("tee: {f}: {err}\n"));
                        }
                    }
                }
                o.push_str(&text);
                0
            }
            "seq" => {
                let nums: Vec<i64> = argv[1..].iter().filter_map(|a| a.parse().ok()).collect();
                let (a, step, b) = match nums.as_slice() {
                    [b] => (1, 1, *b),
                    [a, b] => (*a, 1, *b),
                    [a, s, b] => (*a, *s, *b),
                    _ => {
                        e.push_str("seq: uso: seq [inicio [paso]] fin\n");
                        return Res::Code(1);
                    }
                };
                if step == 0 {
                    e.push_str("seq: el paso no puede ser 0\n");
                    return Res::Code(1);
                }
                let mut i = a;
                let mut n = 0;
                while (step > 0 && i <= b) || (step < 0 && i >= b) {
                    o.push_str(&format!("{i}\n"));
                    i += step;
                    n += 1;
                    if n > 10_000 {
                        break;
                    }
                }
                0
            }
            "expr" => match expr(&argv[1..]) {
                Ok(v) => {
                    o.push_str(&format!("{v}\n"));
                    i32::from(v == 0)
                }
                Err(err) => {
                    e.push_str(&format!("expr: {err}\n"));
                    2
                }
            },
            "basename" => {
                let p = argv.get(1).map(String::as_str).unwrap_or("");
                let mut b = basename(p.trim_end_matches('/')).to_string();
                if let Some(suffix) = argv.get(2) {
                    b = b.strip_suffix(suffix.as_str()).unwrap_or(&b).to_string();
                }
                o.push_str(&b);
                o.push('\n');
                0
            }
            "dirname" => {
                let p = argv.get(1).map(String::as_str).unwrap_or(".");
                let d = if p.contains('/') {
                    parent(p)
                } else {
                    ".".into()
                };
                o.push_str(&d);
                o.push('\n');
                0
            }
            "test" | "[" => {
                let mut a: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                if name == "[" {
                    if a.last() != Some(&"]") {
                        e.push_str("[: falta el ]\n");
                        return Res::Code(2);
                    }
                    a.pop();
                }
                i32::from(!self.test(&a, ctx))
            }
            "touch" => {
                let (_, files) = opts(argv);
                let now = ctx.timestamp();
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                for f in &files {
                    let p = self.abs(f);
                    if !fs.exists(&p)
                        && let Err(err) = fs.create_file(&p, b"", now)
                    {
                        e.push_str(&format!("touch: {f}: {err}\n"));
                    }
                }
                i32::from(!e.is_empty())
            }
            "mkdir" => {
                let (flags, dirs) = opts(argv);
                let now = ctx.timestamp();
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                if dirs.is_empty() {
                    e.push_str("mkdir: falta el nombre\n");
                }
                for d in &dirs {
                    let p = self.abs(d);
                    let r = if flags.contains(&'p') {
                        apt::ensure_dirs(fs, &p, now)
                    } else {
                        fs.mkdir(&p, now)
                    };
                    if let Err(err) = r {
                        e.push_str(&format!("mkdir: {d}: {err}\n"));
                    }
                }
                i32::from(!e.is_empty())
            }
            "rmdir" | "rm" => {
                let (flags, targets) = opts(argv);
                let now = ctx.timestamp();
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                if targets.is_empty() {
                    e.push_str(&format!("{name}: falta qué borrar\n"));
                }
                let mut moved = 0;
                let mut logs = Vec::new();
                for t in &targets {
                    let p = self.abs(t);
                    match fs.stat(&p) {
                        Err(_) if flags.contains(&'f') => {}
                        Err(err) => e.push_str(&format!("{name}: {t}: {err}\n")),
                        Ok(_) if p == "/" || p == crate::files::TRASH => {
                            e.push_str(&format!("{name}: {t}: no se puede borrar\n"))
                        }
                        Ok(st)
                            if st.is_dir
                                && name == "rm"
                                && !flags.contains(&'r')
                                && !flags.contains(&'R') =>
                        {
                            e.push_str(&format!("rm: {t}: es una carpeta (usá rm -r)\n"))
                        }
                        Ok(st) if !st.is_dir && name == "rmdir" => {
                            e.push_str(&format!("rmdir: {t}: no es una carpeta\n"))
                        }
                        Ok(_) if name == "rmdir" && fs.list(&p).is_ok_and(|l| !l.is_empty()) => {
                            e.push_str(&format!("rmdir: {t}: la carpeta no está vacía\n"))
                        }
                        Ok(_) => match FilesApp::move_to_trash(fs, &p, now) {
                            Ok(_) => {
                                moved += 1;
                                logs.push(format!("TERMINAL_PAPELERA {p}"));
                            }
                            Err(err) => e.push_str(&format!("{name}: {t}: {err}\n")),
                        },
                    }
                }
                ctx.log.extend(logs);
                if moved > 0 && flags.contains(&'v') {
                    o.push_str(&format!("{moved} elemento(s) movidos a la Papelera\n"));
                }
                i32::from(!e.is_empty())
            }
            "cp" | "mv" => self.cp_mv(name, argv, ctx, e),
            "find" => self.find(argv, ctx, o, e),
            "du" => {
                let (flags, paths) = opts(argv);
                let paths = if paths.is_empty() {
                    alloc::vec![".".to_string()]
                } else {
                    paths
                };
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                for p in &paths {
                    let abs = self.abs(p);
                    let mut rows = Vec::new();
                    let total = du(fs, &abs, p, &mut rows, 0);
                    if !flags.contains(&'s') {
                        for (size, path) in rows {
                            o.push_str(&format!("{}\t{path}\n", human(size, flags.contains(&'h'))));
                        }
                    }
                    o.push_str(&format!("{}\t{p}\n", human(total, flags.contains(&'h'))));
                }
                0
            }
            "df" => {
                let (flags, _) = opts(argv);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                let (total, free) = (fs.total_bytes(), fs.free_bytes());
                let used = total - free;
                let h = flags.contains(&'h');
                o.push_str("S.ficheros      Tamaño  Usados  Disp  Uso% Montado en\n");
                o.push_str(&format!(
                    "/dev/vda  {:>12} {:>7} {:>6} {:>4}% /\n",
                    human(total, h),
                    human(used, h),
                    human(free, h),
                    (used * 100).checked_div(total).unwrap_or(0)
                ));
                0
            }
            "free" => {
                let (flags, _) = opts(argv);
                let s = ctx.stats;
                let h = flags.contains(&'h');
                o.push_str("               total       usado       libre\n");
                o.push_str(&format!(
                    "Mem:     {:>11} {:>11} {:>11}\n",
                    human(s.ram_total, h),
                    human(s.heap_used, h),
                    human(s.ram_total.saturating_sub(s.heap_used), h)
                ));
                o.push_str(&format!(
                    "Heap:    {:>11} {:>11} {:>11}\n",
                    human(s.heap_total, h),
                    human(s.heap_used, h),
                    human(s.heap_total.saturating_sub(s.heap_used), h)
                ));
                o.push_str("Swap:              0           0           0\n");
                0
            }
            "uname" => {
                let (flags, _) = opts(argv);
                let host = self.host().to_string();
                let parts: Vec<String> = if flags.contains(&'a') {
                    alloc::vec![
                        "JARVIS-OS".into(),
                        host,
                        "0.1-k5".into(),
                        "#1 SMP".into(),
                        "x86_64".into(),
                        "JARVIS".into()
                    ]
                } else {
                    let mut v = Vec::new();
                    if flags.is_empty() || flags.contains(&'s') {
                        v.push("JARVIS-OS".into());
                    }
                    if flags.contains(&'n') {
                        v.push(host);
                    }
                    if flags.contains(&'r') {
                        v.push("0.1-k5".into());
                    }
                    if flags.contains(&'m') || flags.contains(&'p') {
                        v.push("x86_64".into());
                    }
                    v
                };
                o.push_str(&parts.join(" "));
                o.push('\n');
                0
            }
            "whoami" => {
                o.push_str(self.user());
                o.push('\n');
                0
            }
            "hostname" => {
                o.push_str(self.host());
                o.push('\n');
                0
            }
            "id" => {
                let u = self.user().to_string();
                o.push_str(&format!(
                    "uid=1000({u}) gid=1000({u}) grupos=1000({u}),27(sudo)\n"
                ));
                0
            }
            "ufw" => {
                let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                if matches!(args.as_slice(), ["show", "blocked" | "log"]) {
                    let Some(fs) = Self::fs(ctx, e) else {
                        return Res::Code(1);
                    };
                    match fs.read_file(crate::firewall::LOG_PATH) {
                        Ok(b) if !b.is_empty() => o.push_str(&String::from_utf8_lossy(&b)),
                        _ => o.push_str("Todavía no se bloqueó nada.\n"),
                    }
                    return Res::Code(0);
                }
                // Si un comando anterior de la misma línea ya cambió algo, se parte de eso.
                let mut cfg = ctx.out.config.clone().unwrap_or_else(|| ctx.config.clone());
                match cfg.firewall.ufw(&args) {
                    Ok((text, changed)) => {
                        o.push_str(&text);
                        if changed {
                            ctx.log.push(format!("FIREWALL ufw {}", args.join(" ")));
                            ctx.out.config = Some(cfg);
                        }
                        0
                    }
                    Err(msg) => {
                        e.push_str(&format!("ERROR: {msg}\n"));
                        1
                    }
                }
            }
            "sudo" => {
                if argv.len() == 1 {
                    e.push_str("sudo: decime qué ejecutar\n");
                    1
                } else {
                    // En JARVIS-OS sos el dueño de todo: sudo solo ejecuta.
                    return self.exec(&argv[1..], stdin, o, e, ctx, out);
                }
            }
            "date" => match ctx.clock {
                Some(t) => {
                    let wd = DAYS[t.weekday()];
                    o.push_str(&format!(
                        "{wd} {:02} {} {} {:02}:{:02}:{:02} {}\n",
                        t.day,
                        MONTHS[t.month as usize - 1],
                        t.year,
                        t.hour,
                        t.minute,
                        t.second,
                        ctx.config.zone_label()
                    ));
                    0
                }
                None => {
                    e.push_str("date: el reloj del hardware no respondió\n");
                    1
                }
            },
            "cal" => {
                let Some(t) = ctx.clock else {
                    e.push_str("cal: no hay reloj\n");
                    return Res::Code(1);
                };
                o.push_str(&cal(t, self.tty));
                0
            }
            "uptime" => {
                let s = ctx.stats;
                let now = ctx
                    .clock
                    .map(|t| format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second))
                    .unwrap_or_default();
                o.push_str(&format!(
                    " {now} arriba {}, 1 usuario, carga: {}.{:02}\n",
                    crate::widgets::duration(s.uptime_ms),
                    s.cpu_pct / 100,
                    s.cpu_pct % 100
                ));
                0
            }
            "ps" => {
                o.push_str("  PID TTY      TIEMPO   CMD\n");
                // Las tareas del kernel (K9), entre corchetes como los hilos del kernel en Linux.
                let kernel = &ctx.stats.kernel_tasks;
                if kernel.is_empty() {
                    o.push_str("    1 ?        00:00:00 [kernel]\n");
                }
                for t in kernel {
                    let secs = t.cpu_ms / 1000;
                    o.push_str(&format!(
                        "{:>5} ?        {:02}:{:02}:{:02} [{}]\n",
                        t.pid,
                        secs / 3600,
                        secs / 60 % 60,
                        secs % 60,
                        t.name
                    ));
                }
                for t in ctx.tasks {
                    o.push_str(&format!(
                        "{:>5} tty1     00:00:00 {}{}\n",
                        t.id as u64 + 100,
                        crate::apps::name_of(t.kind),
                        if t.minimized { " (minimizada)" } else { "" }
                    ));
                }
                0
            }
            "kill" | "killall" => {
                let mut code = 0;
                for a in argv.iter().skip(1).filter(|a| !a.starts_with('-')) {
                    let target = a.parse::<u64>().ok().and_then(|pid| {
                        ctx.tasks
                            .iter()
                            .find(|t| t.id as u64 + 100 == pid)
                            .map(|t| t.id)
                    });
                    match target {
                        Some(id) => ctx.out.close.push(id),
                        None if a == "1"
                            || ctx
                                .stats
                                .kernel_tasks
                                .iter()
                                .any(|t| t.pid.to_string() == *a) =>
                        {
                            e.push_str(&format!(
                                "kill: ({a}): es una tarea del kernel, no se puede cerrar\n"
                            ));
                            code = 1;
                        }
                        None => {
                            e.push_str(&format!("kill: ({a}): no existe ese proceso\n"));
                            code = 1;
                        }
                    }
                }
                code
            }
            "top" | "htop" => {
                ctx.out.launch.push(Launch::App(AppKind::Monitor));
                0
            }
            "clear" | "reset" => {
                self.clear = true;
                0
            }
            "history" => {
                for (i, h) in self.history.iter().enumerate() {
                    o.push_str(&format!("{:>5}  {h}\n", i + 1));
                }
                0
            }
            "alias" => {
                if argv.len() == 1 {
                    for (k, v) in self.aliases() {
                        o.push_str(&format!("alias {k}='{v}'\n"));
                    }
                } else {
                    for a in &argv[1..] {
                        match a.split_once('=') {
                            Some((k, v)) => {
                                let aliases = self.aliases_mut();
                                aliases.retain(|(x, _)| x != k);
                                aliases.push((k.into(), v.into()));
                            }
                            None => match self.aliases().iter().find(|(k, _)| k == a) {
                                Some((k, v)) => o.push_str(&format!("alias {k}='{v}'\n")),
                                None => e.push_str(&format!("alias: {a}: no existe\n")),
                            },
                        }
                    }
                }
                i32::from(!e.is_empty())
            }
            "unalias" => {
                for a in &argv[1..] {
                    self.aliases_mut().retain(|(k, _)| k != a);
                }
                0
            }
            "export" | "set" if argv.len() == 1 => {
                for (k, v) in self.vars() {
                    o.push_str(&format!("{k}={v}\n"));
                }
                0
            }
            "env" | "printenv" => {
                for (k, v) in self.vars() {
                    o.push_str(&format!("{k}={v}\n"));
                }
                0
            }
            "export" | "set" => {
                for a in &argv[1..] {
                    if let Some((k, v)) = a.split_once('=') {
                        self.set_var(k, v);
                    }
                }
                0
            }
            "unset" => {
                for a in &argv[1..] {
                    self.unset_var(a);
                }
                0
            }
            "which" | "type" | "command" => {
                let mut code = 0;
                for a in argv.iter().skip(1).filter(|a| !a.starts_with('-')) {
                    if let Some((_, v)) = self.aliases().iter().find(|(k, _)| k == a) {
                        o.push_str(&format!("{a}: es un alias de «{v}»\n"));
                    } else if NAMES.iter().any(|(n, _)| n == a)
                        || matches!(a.as_str(), "sudo" | "[" | "source" | ".")
                    {
                        if name == "which" {
                            o.push_str(&format!("/bin/{a}\n"));
                        } else {
                            o.push_str(&format!("{a}: es un comando interno de jsh\n"));
                        }
                    } else if let Some(p) = self.find_program(a, ctx) {
                        o.push_str(&format!("{p}\n"));
                    } else {
                        e.push_str(&format!("{name}: {a}: no encontrado\n"));
                        code = 1;
                    }
                }
                code
            }
            "exit" | "logout" => {
                self.exit = true;
                0
            }
            "reboot" => {
                ctx.out.power = Some(Power::Reboot);
                0
            }
            "shutdown" | "poweroff" | "halt" => {
                ctx.out.power = Some(Power::Shutdown);
                0
            }
            "screenshot" | "captura" => {
                ctx.out.screenshot = true;
                o.push_str("Captura en camino: queda en /Imágenes.\n");
                0
            }
            "jarvis" | "say" => {
                let text = argv[1..].join(" ");
                if text.is_empty() {
                    ctx.out.launch.push(Launch::App(AppKind::Console));
                } else {
                    ctx.out.say = Some(text);
                }
                0
            }
            "nano" | "edit" | "gedit" | "code" | "notepad" => {
                match argv.get(1) {
                    Some(f) => {
                        let p = self.abs(f);
                        ctx.out.launch.push(Launch::Edit(p));
                    }
                    None => ctx.out.launch.push(Launch::App(AppKind::Editor)),
                }
                0
            }
            "open" | "xdg-open" | "start" | "explorer" => self.open(argv, ctx, e),
            "file" => {
                let (_, files) = opts(argv);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                for f in &files {
                    let p = self.abs(f);
                    match fs.stat(&p) {
                        Ok(st) if st.is_dir => o.push_str(&format!("{f}: carpeta\n")),
                        Ok(_) => {
                            let b = fs.read_prefix(&p, 256 * 1024).unwrap_or_default();
                            o.push_str(&format!("{f}: {}\n", binfmt::describe(&b)));
                        }
                        Err(err) => e.push_str(&format!("file: {f}: {err}\n")),
                    }
                }
                i32::from(!e.is_empty())
            }
            "stat" => {
                let (_, files) = opts(argv);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                for f in &files {
                    match fs.stat(&self.abs(f)) {
                        Ok(st) => o.push_str(&format!(
                            "  Archivo: {f}\n   Tamaño: {} bytes\tTipo: {}\nNombre 8.3: {}\tCluster: {}\n Creado: {}\nModificado: {}\n",
                            st.size,
                            if st.is_dir { "carpeta" } else { "archivo" },
                            st.short_name,
                            st.first_cluster,
                            crate::files::format_date(st.created),
                            crate::files::format_date(st.modified)
                        )),
                        Err(err) => e.push_str(&format!("stat: {f}: {err}\n")),
                    }
                }
                i32::from(!e.is_empty())
            }
            "xxd" | "hexdump" | "od" => {
                let (limit, files) = {
                    let mut limit = 256usize;
                    let mut files = Vec::new();
                    let mut it = argv.iter().skip(1);
                    while let Some(a) = it.next() {
                        if a == "-l" || a == "-n" {
                            limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(limit);
                        } else if !a.starts_with('-') {
                            files.push(a.clone());
                        }
                    }
                    (limit.min(64 * 1024), files)
                };
                let data: Vec<u8> = match files.first() {
                    Some(f) => {
                        let Some(fs) = Self::fs(ctx, e) else {
                            return Res::Code(1);
                        };
                        match fs.read_prefix(&self.abs(f), limit) {
                            Ok(b) => b,
                            Err(err) => {
                                e.push_str(&format!("{name}: {f}: {err}\n"));
                                return Res::Code(1);
                            }
                        }
                    }
                    None => stdin
                        .unwrap_or("")
                        .as_bytes()
                        .iter()
                        .take(limit)
                        .copied()
                        .collect(),
                };
                o.push_str(&hexdump(&data));
                0
            }
            "strings" => {
                let (_, files) = opts(argv);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                for f in &files {
                    match fs.read_prefix(&self.abs(f), 4 * 1024 * 1024) {
                        Ok(b) => {
                            for s in binfmt::strings(&b, 5).iter().take(400) {
                                o.push_str(s);
                                o.push('\n');
                            }
                        }
                        Err(err) => e.push_str(&format!("strings: {f}: {err}\n")),
                    }
                }
                0
            }
            "ip" => {
                let n = &ctx.stats.net;
                let m = n.mac;
                o.push_str("1: lo: <LOOPBACK,UP> mtu 65536\n    inet 127.0.0.1/8\n");
                o.push_str(&format!(
                    "2: eth0: <BROADCAST,{}> mtu 1500 (virtio-net)\n    link/ether {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                    if n.present { "UP" } else { "DOWN" },
                    m[0], m[1], m[2], m[3], m[4], m[5]
                ));
                match n.ip {
                    Some(a) => {
                        o.push_str(&format!("    inet {}/24 (DHCP)\n", crate::widgets::ip(a)))
                    }
                    None => o.push_str("    (sin dirección todavía)\n"),
                }
                if let Some(g) = n.gateway {
                    o.push_str(&format!("default via {} dev eth0\n", crate::widgets::ip(g)));
                }
                0
            }
            "ping" => {
                let Some(host) = argv.iter().skip(1).find(|a| !a.starts_with('-')) else {
                    e.push_str("ping: ¿a qué sitio?\n");
                    return Res::Code(2);
                };
                let host = host
                    .trim_start_matches("http://")
                    .trim_start_matches("https://")
                    .trim_end_matches('/')
                    .to_string();
                out.info(&format!(
                    "PING {host}: el kernel todavía no manda ICMP; se prueba con un pedido HTTP."
                ));
                let id = ctx.out.fetch(&format!("http://{host}/"));
                return Res::Wait(Job::Ping {
                    id,
                    host,
                    since: ctx.now_ms,
                });
            }
            "curl" | "wget" => return self.download(name, argv, ctx, e),
            "apt" => {
                return match apt::run(argv, ctx, o, e, out) {
                    Ok(code) => Res::Code(code),
                    Err(job) => Res::Wait(Job::Apt(job)),
                };
            }
            "snap" if argv.get(1).is_some_and(|a| a == "run") => {
                let Some(name) = argv.get(2) else {
                    e.push_str("error: decime qué snap ejecutar\n");
                    return Res::Code(1);
                };
                let mut cmd = alloc::vec![format!("{}/{name}", snap::BIN)];
                cmd.extend(argv[3..].iter().cloned());
                return self.exec(&cmd, stdin, o, e, ctx, out);
            }
            "snap" => {
                return match snap::run(argv, ctx, o, e) {
                    Ok(code) => Res::Code(code),
                    Err(job) => Res::Wait(Job::Snap(job)),
                };
            }
            "winget" | "winget.exe" => {
                return match winget::run(argv, ctx, o, e) {
                    Ok(code) => Res::Code(code),
                    Err(job) => Res::Wait(Job::Winget(job)),
                };
            }
            "dpkg" => {
                if argv.get(1).is_some_and(|a| a == "-l" || a == "--list") {
                    let Some(fs) = Self::fs(ctx, e) else {
                        return Res::Code(1);
                    };
                    o.push_str("Deseado=Instalado\n| Nombre          Versión\n+-================-==========\n");
                    for (n, v) in apt::installed(fs) {
                        o.push_str(&format!("ii  {n:<16} {v}\n"));
                    }
                    0
                } else {
                    e.push_str("dpkg: en JARVIS-OS usá apt (dpkg -l lista lo instalado)\n");
                    1
                }
            }
            "sleep" => {
                let secs: f64 = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let ms = (secs * 1000.0) as u64;
                return Res::Wait(Job::Sleep {
                    until: ctx.now_ms + ms.min(3_600_000),
                });
            }
            "sh" | "bash" | "jsh" | "source" | "." => {
                let Some(f) = argv.get(1) else {
                    e.push_str(&format!("{name}: decime qué script ejecutar\n"));
                    return Res::Code(2);
                };
                let p = self.abs(f);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                match fs.read_file(&p) {
                    Ok(b) => {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        let args: Vec<String> = argv[1..].to_vec();
                        self.run_script(&text, args, stdin, o, ctx, out)
                    }
                    Err(err) => {
                        e.push_str(&format!("{name}: {f}: {err}\n"));
                        127
                    }
                }
            }
            "wine" => {
                let Some(f) = argv.get(1) else {
                    e.push_str("wine: decime qué programa (.exe)\n");
                    return Res::Code(1);
                };
                let p = self.abs(f);
                let Some(fs) = Self::fs(ctx, e) else {
                    return Res::Code(1);
                };
                match fs.read_prefix(&p, 4 * 1024 * 1024) {
                    Ok(b) => {
                        e.push_str(
                            &binfmt::why_not(&b).unwrap_or_else(|| {
                                format!("wine: {f}: no es un programa de Windows")
                            }),
                        );
                        e.push('\n');
                        126
                    }
                    Err(err) => {
                        e.push_str(&format!("wine: {f}: {err}\n"));
                        1
                    }
                }
            }
            _ => return self.external(argv, stdin, o, e, ctx, out),
        };
        Res::Code(code)
    }

    // --- comandos largos ----------------------------------------------------------------------

    fn cd<D: BlockDevice>(
        &mut self,
        arg: Option<&str>,
        ctx: &mut Ctx<'_, D>,
        o: &mut String,
        e: &mut String,
    ) -> i32 {
        let target = match arg {
            None | Some("~") => super::HOME.to_string(),
            Some("-") => {
                let prev = self.prev_dir().to_string();
                o.push_str(&prev);
                o.push('\n');
                prev
            }
            Some(p) => self.abs(p),
        };
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        let ok = target == "/" || fs.stat(&target).is_ok_and(|s| s.is_dir);
        if !ok {
            let why = if fs.exists(&target) {
                "no es una carpeta"
            } else {
                "no existe el archivo o la carpeta"
            };
            e.push_str(&format!("cd: {}: {why}\n", arg.unwrap_or("")));
            return 1;
        }
        // Se usa el nombre como está en el disco (FAT no distingue mayúsculas).
        let real = real_path(fs, &target);
        self.set_cwd(real);
        0
    }

    fn ls<D: BlockDevice>(
        &mut self,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        o: &mut String,
        e: &mut String,
    ) -> i32 {
        let (flags, paths) = opts(argv);
        let long = flags.contains(&'l');
        let all = flags.contains(&'a') || flags.contains(&'A');
        let human_sizes = flags.contains(&'h');
        let one = flags.contains(&'1') || !self.tty;
        let paths = if paths.is_empty() {
            alloc::vec![".".to_string()]
        } else {
            paths
        };
        let tty = self.tty;
        let cols = self.cols;
        if paths.len() == 1 && self.abs(&paths[0]) == "/proc" {
            o.push_str(&PROC.join("\n"));
            o.push('\n');
            return 0;
        }
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        for (i, p) in paths.iter().enumerate() {
            let abs = self.abs(p);
            let st = if abs == "/" { None } else { fs.stat(&abs).ok() };
            let entries: Vec<DirEntry> = match st {
                Some(s) if !s.is_dir => alloc::vec![DirEntry {
                    name: p.clone(),
                    ..s
                }],
                _ => match fs.list(&abs) {
                    Ok(v) => v,
                    Err(err) => {
                        e.push_str(&format!("ls: {p}: {err}\n"));
                        continue;
                    }
                },
            };
            if paths.len() > 1 {
                if i > 0 {
                    o.push('\n');
                }
                o.push_str(&format!("{p}:\n"));
            }
            let mut entries: Vec<DirEntry> = entries
                .into_iter()
                .filter(|e| all || (!e.name.starts_with('.') && !e.hidden))
                .collect();
            entries.sort_by_key(|e| e.name.to_lowercase());
            let is_exec = |e: &DirEntry| abs == BIN || e.name.ends_with(".sh");
            let color = |e: &DirEntry| -> (&str, &str) {
                if !tty {
                    ("", "")
                } else if e.is_dir {
                    (ansi::DIR, ansi::RESET)
                } else if is_exec(e) || e.name.to_lowercase().ends_with(".exe") {
                    (ansi::EXEC, ansi::RESET)
                } else if e.name.to_lowercase().ends_with(".bmp") {
                    (ansi::MAGENTA, ansi::RESET)
                } else {
                    ("", "")
                }
            };
            if long {
                let total: u64 = entries.iter().map(|e| e.size as u64).sum();
                o.push_str(&format!("total {}\n", human(total, human_sizes)));
                let user = self.user().to_string();
                for en in &entries {
                    let (c0, c1) = color(en);
                    let perms = if en.is_dir {
                        "drwxr-xr-x"
                    } else if is_exec(en) {
                        "-rwxr-xr-x"
                    } else if en.read_only {
                        "-r--r--r--"
                    } else {
                        "-rw-r--r--"
                    };
                    let m = en.modified;
                    let size = if en.is_dir { 4096 } else { en.size as u64 };
                    o.push_str(&format!(
                        "{perms} 1 {user} {user} {:>8} {} {:>2} {:02}:{:02} {c0}{}{c1}\n",
                        human(size, human_sizes),
                        MONTHS[(m.month.clamp(1, 12) - 1) as usize],
                        m.day,
                        m.hour,
                        m.minute,
                        en.name
                    ));
                }
            } else if one {
                for en in &entries {
                    let (c0, c1) = color(en);
                    o.push_str(&format!("{c0}{}{c1}\n", en.name));
                }
            } else {
                // En columnas, como `ls` en una terminal.
                let width = entries
                    .iter()
                    .map(|e| e.name.chars().count())
                    .max()
                    .unwrap_or(0)
                    + 2;
                let per_row = (cols / width.max(1)).max(1);
                for (j, en) in entries.iter().enumerate() {
                    let (c0, c1) = color(en);
                    let pad = width - en.name.chars().count();
                    let last = (j + 1) % per_row == 0 || j + 1 == entries.len();
                    o.push_str(&format!("{c0}{}{c1}", en.name));
                    if last {
                        o.push('\n');
                    } else {
                        o.push_str(&" ".repeat(pad));
                    }
                }
            }
        }
        i32::from(!e.is_empty())
    }

    fn tree<D: BlockDevice>(
        &mut self,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        o: &mut String,
        e: &mut String,
    ) -> i32 {
        let (_, paths) = opts(argv);
        let root = paths.first().cloned().unwrap_or_else(|| ".".into());
        let abs = self.abs(&root);
        let tty = self.tty;
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        let (mut dirs, mut files) = (0, 0);
        fn walk<D: BlockDevice>(
            fs: &mut FileSystem<D>,
            path: &str,
            prefix: &str,
            o: &mut String,
            counts: (&mut u32, &mut u32),
            tty: bool,
            depth: u32,
        ) {
            if depth > 8 {
                return;
            }
            let mut entries: Vec<DirEntry> = fs
                .list(path)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| !e.name.starts_with('.') && !e.hidden)
                .collect();
            entries.sort_by_key(|e| e.name.to_lowercase());
            let n = entries.len();
            let (dirs, files) = counts;
            for (i, en) in entries.iter().enumerate() {
                let last = i + 1 == n;
                let branch = if last { "`-- " } else { "|-- " };
                if en.is_dir {
                    *dirs += 1;
                    let (c0, c1) = if tty {
                        (ansi::DIR, ansi::RESET)
                    } else {
                        ("", "")
                    };
                    o.push_str(&format!("{prefix}{branch}{c0}{}{c1}\n", en.name));
                    let next = format!("{prefix}{}", if last { "    " } else { "|   " });
                    walk(
                        fs,
                        &child(path, &en.name),
                        &next,
                        o,
                        (&mut *dirs, &mut *files),
                        tty,
                        depth + 1,
                    );
                } else {
                    *files += 1;
                    o.push_str(&format!("{prefix}{branch}{}\n", en.name));
                }
            }
        }
        if !fs.exists(&abs) && abs != "/" {
            e.push_str(&format!("tree: {root}: no existe\n"));
            return 1;
        }
        o.push_str(&format!("{root}\n"));
        walk(fs, &abs, "", o, (&mut dirs, &mut files), tty, 0);
        o.push_str(&format!("\n{dirs} carpetas, {files} archivos\n"));
        0
    }

    fn grep<D: BlockDevice>(
        &mut self,
        argv: &[String],
        stdin: Option<&str>,
        ctx: &mut Ctx<'_, D>,
        o: &mut String,
        e: &mut String,
    ) -> i32 {
        let (flags, rest) = opts(argv);
        let Some(pattern) = rest.first() else {
            e.push_str("uso: grep [-i -v -n -c -r] PATRÓN [ARCHIVO...]\n");
            return 2;
        };
        let re = match Regex::new(pattern, flags.contains(&'i')) {
            Ok(r) => r,
            Err(err) => {
                e.push_str(&format!("grep: patrón inválido: {err}\n"));
                return 2;
            }
        };
        let mut files: Vec<String> = rest[1..].to_vec();
        if flags.contains(&'r') || flags.contains(&'R') {
            let roots = if files.is_empty() {
                alloc::vec![".".to_string()]
            } else {
                files.clone()
            };
            files.clear();
            let Some(fs) = Self::fs(ctx, e) else { return 2 };
            for r in roots {
                let abs = self.abs(&r);
                collect_files(fs, &abs, &r, &mut files, 0);
            }
        }
        let many = files.len() > 1;
        let mut found = false;
        let tty = self.tty;
        for (f, text) in self.inputs("grep", &files, stdin, ctx, e) {
            let mut count = 0;
            for (i, l) in lines_of(&text).iter().enumerate() {
                if re.is_match(l) == flags.contains(&'v') {
                    continue;
                }
                found = true;
                count += 1;
                if flags.contains(&'c') || flags.contains(&'l') {
                    continue;
                }
                if many {
                    let (c0, c1) = if tty {
                        (ansi::MAGENTA, ansi::RESET)
                    } else {
                        ("", "")
                    };
                    o.push_str(&format!("{c0}{f}{c1}:"));
                }
                if flags.contains(&'n') {
                    let (c0, c1) = if tty {
                        (ansi::GREEN, ansi::RESET)
                    } else {
                        ("", "")
                    };
                    o.push_str(&format!("{c0}{}{c1}:", i + 1));
                }
                o.push_str(l);
                o.push('\n');
            }
            if flags.contains(&'l') && count > 0 {
                o.push_str(&format!("{f}\n"));
            } else if flags.contains(&'c') {
                if many {
                    o.push_str(&format!("{f}:"));
                }
                o.push_str(&format!("{count}\n"));
            }
        }
        if !e.is_empty() { 2 } else { i32::from(!found) }
    }

    fn cp_mv<D: BlockDevice>(
        &mut self,
        name: &str,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        e: &mut String,
    ) -> i32 {
        let (flags, paths) = opts(argv);
        if paths.len() < 2 {
            e.push_str(&format!("{name}: uso: {name} ORIGEN... DESTINO\n"));
            return 1;
        }
        let now = ctx.timestamp();
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        let (sources, dest) = paths.split_at(paths.len() - 1);
        let dest_abs = self.abs(&dest[0]);
        let dest_is_dir = dest_abs == "/" || fs.stat(&dest_abs).is_ok_and(|s| s.is_dir);
        if sources.len() > 1 && !dest_is_dir {
            e.push_str(&format!("{name}: {}: no es una carpeta\n", dest[0]));
            return 1;
        }
        for s in sources {
            let src = self.abs(s);
            let st = match fs.stat(&src) {
                Ok(st) => st,
                Err(err) => {
                    e.push_str(&format!("{name}: {s}: {err}\n"));
                    continue;
                }
            };
            let target = if dest_is_dir {
                child(&dest_abs, &st.name)
            } else {
                dest_abs.clone()
            };
            if target.eq_ignore_ascii_case(&src) {
                e.push_str(&format!("{name}: {s} y {} son el mismo archivo\n", dest[0]));
                continue;
            }
            let result = if name == "cp" {
                if st.is_dir && !flags.contains(&'r') && !flags.contains(&'R') {
                    e.push_str(&format!("cp: {s}: es una carpeta (usá cp -r)\n"));
                    continue;
                }
                if fs.exists(&target) && !st.is_dir {
                    // cp pisa el destino (como en Linux): el viejo va a la Papelera.
                    let _ = FilesApp::move_to_trash(fs, &target, now);
                }
                fs.copy(&src, &target, now)
            } else {
                if fs.exists(&target) {
                    let _ = FilesApp::move_to_trash(fs, &target, now);
                }
                let (from_dir, to_dir) = (parent(&src), parent(&target));
                let new_name = basename(&target).to_string();
                let mut r = Ok(());
                let mut cur = src.clone();
                if !from_dir.eq_ignore_ascii_case(&to_dir) {
                    r = fs.move_to(&src, &to_dir);
                    cur = child(&to_dir, &st.name);
                }
                if r.is_ok() && st.name != new_name {
                    r = fs.rename(&cur, &new_name);
                }
                r
            };
            if let Err(err) = result {
                e.push_str(&format!("{name}: {s}: {err}\n"));
            }
        }
        i32::from(!e.is_empty())
    }

    fn find<D: BlockDevice>(
        &mut self,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        o: &mut String,
        e: &mut String,
    ) -> i32 {
        let mut root = ".".to_string();
        let mut pattern: Option<String> = None;
        let mut kind: Option<char> = None;
        let mut it = argv.iter().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "-name" | "-iname" => pattern = it.next().cloned(),
                "-type" => kind = it.next().and_then(|t| t.chars().next()),
                p if !p.starts_with('-') => root = p.to_string(),
                _ => {}
            }
        }
        let abs = self.abs(&root);
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        fn walk<D: BlockDevice>(
            fs: &mut FileSystem<D>,
            abs: &str,
            shown: &str,
            pattern: Option<&str>,
            kind: Option<char>,
            o: &mut String,
            depth: u32,
        ) {
            if depth > 16 {
                return;
            }
            for en in fs.list(abs).unwrap_or_default() {
                let a = child(abs, &en.name);
                let s = if shown == "/" {
                    format!("/{}", en.name)
                } else {
                    format!("{shown}/{}", en.name)
                };
                let type_ok = match kind {
                    Some('d') => en.is_dir,
                    Some('f') => !en.is_dir,
                    _ => true,
                };
                if type_ok && pattern.is_none_or(|p| super::parse::glob_match(p, &en.name)) {
                    o.push_str(&s);
                    o.push('\n');
                }
                if en.is_dir {
                    walk(fs, &a, &s, pattern, kind, o, depth + 1);
                }
            }
        }
        if kind != Some('f') && pattern.is_none() {
            o.push_str(&root);
            o.push('\n');
        }
        walk(fs, &abs, &root, pattern.as_deref(), kind, o, 0);
        0
    }

    fn test<D: BlockDevice>(&self, a: &[&str], ctx: &mut Ctx<'_, D>) -> bool {
        match a {
            [] => false,
            ["!", rest @ ..] => !self.test(rest, ctx),
            [s] => !s.is_empty(),
            ["-z", s] => s.is_empty(),
            ["-n", s] => !s.is_empty(),
            [op @ ("-e" | "-f" | "-d" | "-s"), p] => {
                let path = self.abs(p);
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    return false;
                };
                if path == "/" {
                    return *op != "-f";
                }
                match fs.stat(&path) {
                    Ok(st) => match *op {
                        "-f" => !st.is_dir,
                        "-d" => st.is_dir,
                        "-s" => st.size > 0,
                        _ => true,
                    },
                    Err(_) => false,
                }
            }
            [x, "=" | "==", y] => x == y,
            [x, "!=", y] => x != y,
            [x, op, y] => {
                let (Ok(x), Ok(y)) = (x.parse::<i64>(), y.parse::<i64>()) else {
                    return false;
                };
                match *op {
                    "-eq" => x == y,
                    "-ne" => x != y,
                    "-lt" => x < y,
                    "-le" => x <= y,
                    "-gt" => x > y,
                    "-ge" => x >= y,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn open<D: BlockDevice>(
        &mut self,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        e: &mut String,
    ) -> i32 {
        let Some(target) = argv.get(1) else {
            ctx.out.launch.push(Launch::Folder(self.cwd.clone()));
            return 0;
        };
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.contains(".com")
        {
            ctx.out.launch.push(Launch::Browse(target.clone()));
            return 0;
        }
        let p = self.abs(target);
        let Some(fs) = Self::fs(ctx, e) else { return 1 };
        let Ok(st) = fs.stat(&p).or_else(|err| {
            if p == "/" {
                fs.stat("/Documentos")
            } else {
                Err(err)
            }
        }) else {
            e.push_str(&format!("open: {target}: no existe\n"));
            return 1;
        };
        let lower = p.to_lowercase();
        let launch = if st.is_dir || p == "/" {
            Launch::Folder(p)
        } else if [".bmp", ".png", ".jpg", ".jpeg"]
            .iter()
            .any(|e| lower.ends_with(e))
        {
            Launch::View(p)
        } else if lower.ends_with(".html") || lower.ends_with(".htm") {
            Launch::Browse(format!("file://{p}"))
        } else {
            let b = fs.read_prefix(&p, 4 * 1024 * 1024).unwrap_or_default();
            if let Some(why) = binfmt::why_not(&b) {
                e.push_str(&why);
                e.push('\n');
                return 126;
            }
            Launch::Edit(p)
        };
        ctx.out.launch.push(launch);
        0
    }

    fn download<D: BlockDevice>(
        &mut self,
        name: &str,
        argv: &[String],
        ctx: &mut Ctx<'_, D>,
        e: &mut String,
    ) -> Res {
        let mut url: Option<String> = None;
        let mut save: Option<String> = None;
        let mut quiet = name == "curl";
        let mut remote_name = name == "wget";
        let mut it = argv.iter().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "-o" | "-O" if name == "curl" && a == "-o" => save = it.next().cloned(),
                "-O" if name == "curl" => remote_name = true,
                "-O" | "--output-document" => save = it.next().cloned(),
                "-q" | "--quiet" | "-s" | "--silent" => quiet = true,
                "-v" => quiet = false,
                "-L" | "-sL" | "-Ls" | "-fsSL" => {}
                a if !a.starts_with('-') => url = Some(a.to_string()),
                _ => {}
            }
        }
        let Some(raw) = url else {
            e.push_str(&format!(
                "{name}: falta la dirección. Ejemplo: {name} https://example.com/\n"
            ));
            return Res::Code(2);
        };
        let full = if raw.contains("://") {
            raw.clone()
        } else {
            format!("http://{raw}")
        };
        let Some(u) = Url::parse(&full) else {
            e.push_str(&format!("{name}: dirección inválida: {raw}\n"));
            return Res::Code(3);
        };
        if save.is_none() && remote_name {
            let file = basename(u.path_only()).to_string();
            let file = crate::web::url::percent_decode(&file);
            save = Some(if file.is_empty() {
                "index.html".into()
            } else {
                file
            });
        }
        let save = save.map(|s| self.abs(&s));
        if name == "wget" {
            e.push_str(&format!(
                "--{}--  {}\nResolviendo {}... conectando... Petición HTTP enviada, esperando respuesta...\n",
                ctx.clock.map(|t| format!("{}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)).unwrap_or_default(),
                u,
                u.host
            ));
        }
        let id = ctx.out.fetch_kind(&u.to_string(), FetchKind::Download);
        Res::Wait(Job::Fetch {
            id,
            url: u.to_string(),
            save,
            quiet,
            since: ctx.now_ms,
        })
    }

    /// Un programa que no es un comando interno: un script de `/Programas/bin` o una ruta.
    fn external<D: BlockDevice>(
        &mut self,
        argv: &[String],
        stdin: Option<&str>,
        o: &mut String,
        e: &mut String,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Res {
        let name = &argv[0];
        let Some(path) = self.find_program(name, ctx) else {
            e.push_str(&format!("{name}: no se encontró la orden\n"));
            // ¿Es un paquete que se puede instalar?
            if let Some(fs) = ctx.fs.as_deref_mut()
                && let Some(index) = apt::local_index(fs)
                && index.iter().any(|p| p.name == *name)
            {
                e.push_str(&format!("Se puede instalar con:\n  apt install {name}\n"));
            }
            return Res::Code(127);
        };
        let Some(fs) = Self::fs(ctx, e) else {
            return Res::Code(1);
        };
        let data = match fs.read_prefix(&path, 4 * 1024 * 1024) {
            Ok(d) => d,
            Err(err) => {
                e.push_str(&format!("{name}: {err}\n"));
                return Res::Code(126);
            }
        };
        if let Some(why) = binfmt::why_not(&data) {
            e.push_str(&why);
            e.push('\n');
            ctx.log.push(format!("TERMINAL_EXE {path}"));
            return Res::Code(126);
        }
        match binfmt::detect(&data) {
            binfmt::Kind::Script | binfmt::Kind::Text => {
                let text = String::from_utf8_lossy(&data).into_owned();
                let mut args = alloc::vec![path.clone()];
                args.extend(argv[1..].iter().cloned());
                Res::Code(self.run_script(&text, args, stdin, o, ctx, out))
            }
            _ => {
                e.push_str(&format!(
                    "{name}: no es un programa ({})\n",
                    binfmt::describe(&data)
                ));
                Res::Code(126)
            }
        }
    }

    pub(crate) fn find_program<D: BlockDevice>(
        &self,
        name: &str,
        ctx: &mut Ctx<'_, D>,
    ) -> Option<String> {
        let fs = ctx.fs.as_deref_mut()?;
        let is_file = |fs: &mut FileSystem<D>, p: &str| fs.stat(p).is_ok_and(|s| !s.is_dir);
        if name.contains('/') {
            let p = self.abs(name);
            return is_file(fs, &p).then_some(p);
        }
        let path = self.var("PATH").unwrap_or(PATH).to_string();
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            for candidate in [child(dir, name), child(dir, &format!("{name}.sh"))] {
                if is_file(fs, &candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

/// Terminó un `curl`/`wget`. Devuelve (código, salida estándar).
pub(crate) fn finish_fetch<D: BlockDevice>(
    url: &str,
    save: Option<&str>,
    quiet: bool,
    since: u64,
    result: &Result<HttpResponse, String>,
    ctx: &mut Ctx<'_, D>,
    out: &mut Out,
) -> (i32, String) {
    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            out.err(&format!("No se pudo descargar {url}: {e}"));
            return (4, String::new());
        }
    };
    let ms = ctx.now_ms.saturating_sub(since).max(1);
    if !quiet {
        out.info(&format!(
            "HTTP {} · {} · {} · {}",
            resp.status,
            if resp.content_type.is_empty() {
                "sin tipo"
            } else {
                resp.content_type.as_str()
            },
            format_size(resp.body.len() as u64),
            crate::widgets::rate((resp.body.len() as u64 * 1000 / ms) as u32)
        ));
    }
    if resp.status >= 400 {
        out.err(&format!("ERROR {}: el servidor no lo dio.", resp.status));
        return (8, String::new());
    }
    match save {
        Some(path) => {
            let now = ctx.timestamp();
            let Some(fs) = ctx.fs.as_deref_mut() else {
                out.err("no hay disco");
                return (1, String::new());
            };
            if fs.exists(path) {
                let _ = FilesApp::move_to_trash(fs, path, now);
            }
            match fs.write_file(path, &resp.body, now) {
                Ok(()) => {
                    ctx.log
                        .push(format!("DESCARGA {path} ({} bytes)", resp.body.len()));
                    out.info(&format!(
                        "«{path}» guardado [{}]\n{}",
                        resp.body.len(),
                        binfmt::describe(&resp.body[..resp.body.len().min(256 * 1024)])
                    ));
                    (0, String::new())
                }
                Err(e) => {
                    out.err(&format!("No se pudo guardar {path}: {e}"));
                    (1, String::new())
                }
            }
        }
        None => {
            let text = crate::web::html::decode_bytes(&resp.body);
            (0, text)
        }
    }
}

// --- ayudantes -------------------------------------------------------------------------------

/// Los archivos de `/proc` que existen.
pub const PROC: [&str; 5] = ["cpuinfo", "meminfo", "uptime", "version", "loadavg"];

/// `/proc/…`: información del sistema como texto (lo que leen `neofetch` y compañía).
fn proc_file<D: BlockDevice>(path: &str, ctx: &Ctx<'_, D>) -> Option<String> {
    let s = ctx.stats;
    let secs = s.uptime_ms / 1000;
    Some(match path.strip_prefix("/proc/")? {
        "cpuinfo" => format!(
            "processor\t: 0\nvendor_id\t: x86_64\nmodel name\t: {}\ncpu MHz\t\t: -\nflags\t\t: fpu tsc apic sse2\n",
            if s.cpu_name.is_empty() {
                "x86_64"
            } else {
                s.cpu_name.as_str()
            }
        ),
        "meminfo" => format!(
            "MemTotal:       {:>10} kB\nMemFree:        {:>10} kB\nHeapTotal:      {:>10} kB\nHeapUsed:       {:>10} kB\n",
            s.ram_total / 1024,
            s.ram_total.saturating_sub(s.heap_used) / 1024,
            s.heap_total / 1024,
            s.heap_used / 1024
        ),
        "uptime" => format!("{secs}.{:02} 0.00\n", s.uptime_ms % 1000 / 10),
        "version" => "JARVIS-OS versión 0.1-k5 (Rust nightly, no_std) #1 SMP x86_64\n".into(),
        "loadavg" => format!(
            "{}.{:02} 0.00 0.00 1/1 1\n",
            s.cpu_pct / 100,
            s.cpu_pct % 100
        ),
        _ => return None,
    })
}

fn human(bytes: u64, h: bool) -> String {
    if h {
        let s = format_size(bytes);
        s.replace(" KiB", "K")
            .replace(" MiB", "M")
            .replace(" B", "")
    } else {
        bytes.to_string()
    }
}

fn du<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    abs: &str,
    shown: &str,
    rows: &mut Vec<(u64, String)>,
    depth: u32,
) -> u64 {
    if depth > 16 {
        return 0;
    }
    let mut total = 0;
    match fs.stat(abs) {
        Ok(st) if !st.is_dir => return st.size as u64,
        _ => {}
    }
    for en in fs.list(abs).unwrap_or_default() {
        let a = child(abs, &en.name);
        let s = if shown == "/" {
            format!("/{}", en.name)
        } else {
            format!("{shown}/{}", en.name)
        };
        if en.is_dir {
            let sub = du(fs, &a, &s, rows, depth + 1);
            rows.push((sub, s));
            total += sub;
        } else {
            total += en.size as u64;
        }
    }
    total
}

fn collect_files<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    abs: &str,
    shown: &str,
    out: &mut Vec<String>,
    depth: u32,
) {
    if depth > 16 || out.len() > 2000 {
        return;
    }
    match fs.stat(abs) {
        Ok(st) if !st.is_dir => {
            out.push(shown.to_string());
            return;
        }
        _ => {}
    }
    for en in fs.list(abs).unwrap_or_default() {
        if en.name.starts_with('.') {
            continue;
        }
        let s = if shown == "/" {
            format!("/{}", en.name)
        } else {
            format!("{shown}/{}", en.name)
        };
        collect_files(fs, &child(abs, &en.name), &s, out, depth + 1);
    }
}

/// La ruta con los nombres como están guardados (mayúsculas incluidas).
fn real_path<D: BlockDevice>(fs: &mut FileSystem<D>, path: &str) -> String {
    let mut cur = String::from("/");
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        let name = fs
            .list(&cur)
            .unwrap_or_default()
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(seg))
            .map(|e| e.name)
            .unwrap_or_else(|| seg.to_string());
        cur = child(&cur, &name);
    }
    cur
}

enum SedCmd {
    /// s/patrón/reemplazo/[g]
    Sub(Regex, String, bool),
    /// /patrón/d
    Delete(Regex),
    /// /patrón/p (con -n: solo esas líneas)
    Print(Regex),
}

fn parse_sed(script: &str, ignore_case: bool) -> Result<SedCmd, &'static str> {
    if let Some(rest) = script.strip_prefix('s') {
        let sep = rest.chars().next().ok_or("falta el separador")?;
        let parts: Vec<&str> = rest[sep.len_utf8()..].split(sep).collect();
        if parts.len() < 2 {
            return Err("formato: s/patrón/reemplazo/g");
        }
        let flags = parts.get(2).copied().unwrap_or("");
        let re = Regex::new(parts[0], ignore_case || flags.contains('I'))?;
        return Ok(SedCmd::Sub(re, parts[1].to_string(), flags.contains('g')));
    }
    if let Some(rest) = script.strip_prefix('/') {
        let (pat, cmd) = rest.rsplit_once('/').ok_or("formato: /patrón/d")?;
        let re = Regex::new(pat, ignore_case)?;
        return match cmd {
            "d" => Ok(SedCmd::Delete(re)),
            "p" => Ok(SedCmd::Print(re)),
            _ => Err("solo se entienden s///, /x/d y /x/p"),
        };
    }
    Err("solo se entienden s///, /x/d y /x/p")
}

/// `\n`, `\t`, `\e[32m`…
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('\\') => out.push('\\'),
            Some('0') => {
                // \033
                let digits: String = core::iter::from_fn(|| it.next_if(|d| d.is_digit(8)))
                    .take(3)
                    .collect();
                out.push(u8::from_str_radix(&digits, 8).map_or('\0', |b| b as char));
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn printf(fmt: &str, args: &[String]) -> String {
    let fmt = unescape(fmt);
    let mut out = String::new();
    let mut args = args.iter();
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // Ancho opcional: %-10s, %5d
        let mut spec = String::new();
        while let Some(&d) = it.peek() {
            if d.is_ascii_digit() || d == '-' {
                spec.push(d);
                it.next();
            } else {
                break;
            }
        }
        let left = spec.starts_with('-');
        let width: usize = spec.trim_start_matches('-').parse().unwrap_or(0);
        let value = match it.next() {
            Some('%') => "%".to_string(),
            Some('s') => args.next().cloned().unwrap_or_default(),
            Some('d') | Some('i') => args
                .next()
                .and_then(|a| a.parse::<i64>().ok())
                .unwrap_or(0)
                .to_string(),
            Some(other) => format!("%{other}"),
            None => "%".into(),
        };
        let pad = width.saturating_sub(value.chars().count());
        if left {
            out.push_str(&value);
            out.push_str(&" ".repeat(pad));
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(&value);
        }
    }
    out
}

fn expand_set(s: &str) -> Vec<char> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if i + 2 < c.len() && c[i + 1] == '-' && c[i] <= c[i + 2] {
            let (a, b) = (c[i] as u32, c[i + 2] as u32);
            out.extend((a..=b).filter_map(char::from_u32));
            i += 3;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `expr 2 + 3 '*' 4` (izquierda a derecha, con * / % antes que + -).
fn expr(args: &[String]) -> Result<i64, &'static str> {
    if args.is_empty() {
        return Err("falta la expresión");
    }
    let num = |s: &String| s.parse::<i64>().map_err(|_| "no es un número");
    // Primero * / %.
    let mut terms: Vec<(char, i64)> = Vec::new();
    let mut cur = num(&args[0])?;
    let mut i = 1;
    while i < args.len() {
        let op = args[i].as_str();
        let rhs = num(args.get(i + 1).ok_or("falta un número")?)?;
        match op {
            "*" | "x" | "X" => cur *= rhs,
            "/" => cur = cur.checked_div(rhs).ok_or("división por cero")?,
            "%" => cur = cur.checked_rem(rhs).ok_or("división por cero")?,
            "+" | "-" => {
                terms.push((op.chars().next().unwrap_or('+'), cur));
                cur = rhs;
            }
            _ => return Err("operador desconocido (usá + - x / %)"),
        }
        i += 2;
    }
    terms.push(('=', cur));
    let mut total = terms[0].1;
    for w in terms.windows(2) {
        match w[0].0 {
            '+' => total += w[1].1,
            '-' => total -= w[1].1,
            _ => {}
        }
    }
    Ok(total)
}

fn hexdump(data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}: ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x}")),
                None => out.push_str("  "),
            }
            if j % 2 == 1 {
                out.push(' ');
            }
        }
        out.push(' ');
        for &b in chunk {
            out.push(if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

fn cal(t: jarvis_gfx::clock::DateTime, tty: bool) -> String {
    const NAMES: [&str; 12] = [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ];
    let title = format!("{} {}", NAMES[t.month as usize - 1], t.year);
    let mut out = format!("{:^20}\n do lu ma mi ju vi sá\n", title);
    let first = jarvis_gfx::clock::DateTime { day: 1, ..t }.weekday();
    let days = (28..=31)
        .rev()
        .find(|&d| jarvis_gfx::clock::DateTime { day: d, ..t }.is_valid())
        .unwrap_or(28);
    out.push_str(&"   ".repeat(first));
    for d in 1..=days {
        if d == t.day && tty {
            out.push_str(&format!(" \x1b[7m{d:>2}\x1b[0m"));
        } else {
            out.push_str(&format!(" {d:>2}"));
        }
        if (first + d as usize).is_multiple_of(7) {
            out.push('\n');
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ayudantes() {
        assert_eq!(unescape("a\\tb\\n\\e[1m"), "a\tb\n\x1b[1m");
        assert_eq!(
            printf("%-5s|%3d%%\\n", &["ab".into(), "7".into()]),
            "ab   |  7%\n"
        );
        assert_eq!(expand_set("a-e"), ['a', 'b', 'c', 'd', 'e']);
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(expr(&args("2 + 3 * 4")), Ok(14));
        assert_eq!(expr(&args("10 - 4 - 3")), Ok(3));
        assert!(expr(&args("1 / 0")).is_err());
        assert!(hexdump(b"hola").starts_with("00000000: 686f 6c61"));
        assert_eq!(human(2048, true), "2.0K");
    }
}
