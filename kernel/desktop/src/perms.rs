//! Usuarios y permisos de archivos (ADR 0020).
//!
//! FAT32 no tiene dueños ni permisos: solo un bit de "solo lectura". Linux necesita, por cada
//! archivo, un **modo** (`rwxr-xr-x`: qué puede hacer el dueño, su grupo y los demás) y un
//! **dueño** (uid y gid). Los guardamos aparte, en [`DB`], como hace WSL con los discos de
//! Windows ("metadatos de DrvFs"): el FAT32 no cambia (así lo sigue leyendo Windows) y la tabla
//! solo tiene lo que difiere del valor por defecto ([`Perms::default_for`]).
//!
//! Los valores por defecto siguen a un Linux común:
//! - Las carpetas del sistema ([`SYSTEM_DIRS`]: `/Sistema`, `/Programas`, `/snap`, `/usr`…) son
//!   de **root**: carpetas `drwxr-xr-x` (0755), archivos `-rw-r--r--` (0644) y los de una
//!   carpeta `bin` ejecutables (0755). Para cambiarlas hace falta `sudo`.
//! - `/Sistema/config.ini` es 0600: tiene el hash de la contraseña (como `/etc/shadow`).
//! - El resto del disco es del usuario (uid 1000): 0755 las carpetas, 0644 los archivos.
//!
//! **root** (uid 0) puede leer y escribir todo; ejecutar, solo si algún bit `x` está prendido
//! (como en Linux). Los demás, según los bits del dueño, del grupo o de los otros. Crear,
//! borrar o renombrar algo pide escribir en la **carpeta** que lo contiene.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};

/// Dónde se guardan los modos y dueños que no son los de por defecto.
pub const DB: &str = "/Sistema/permisos.db";
pub const ROOT: u32 = 0;
/// El usuario de JARVIS-OS (el que crea el asistente de instalación).
pub const USER: u32 = 1000;
/// El grupo de los administradores (los que pueden usar `sudo`), como en Debian/Ubuntu.
pub const SUDO_GROUP: u32 = 27;
/// Carpetas del sistema: son de root.
pub const SYSTEM_DIRS: &[&str] = &[
    "/Sistema",
    "/Programas",
    "/snap",
    "/usr",
    "/lib",
    "/lib64",
    "/bin",
    "/sbin",
    "/etc",
    "/opt",
    "/var",
    "/Cuarentena",
];
/// Archivos del sistema que solo puede leer root.
const SECRET_FILES: &[&str] = &["/Sistema/config.ini", "/Sistema/permisos.db"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Exec,
}

/// Modo, dueño y grupo de un archivo o carpeta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Los 12 bits de permisos (`0o7777`: setuid, setgid, sticky y rwx×3).
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
}

/// La tabla de permisos (cargada del disco la primera vez que se usa).
#[derive(Default)]
pub struct Perms {
    /// Clave: la ruta en minúsculas (FAT32 no distingue mayúsculas).
    map: BTreeMap<String, Entry>,
    loaded: bool,
    /// El nombre del usuario (para `ls -l`, `id`).
    pub user: String,
}

/// Una ruta como clave: minúsculas y sin `/` al final.
fn key(path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        "/".into()
    } else {
        p.to_lowercase()
    }
}

fn under(path_key: &str, dir: &str) -> bool {
    let d = dir.to_lowercase();
    path_key == d
        || path_key
            .strip_prefix(&d)
            .is_some_and(|r| r.starts_with('/'))
}

/// ¿Está en una carpeta del sistema?
pub fn is_system(path: &str) -> bool {
    let k = key(path);
    SYSTEM_DIRS.iter().any(|d| under(&k, d))
}

impl Perms {
    pub fn new() -> Perms {
        Perms {
            map: BTreeMap::new(),
            loaded: false,
            user: "roman".into(),
        }
    }

    /// Lee la tabla del disco la primera vez (después usa la de memoria).
    pub fn ensure_loaded<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Ok(data) = fs.read_file(DB) {
            self.map = parse(&String::from_utf8_lossy(&data));
        }
    }

    /// Vuelve a leer la tabla (si otro la cambió en el disco, o con otro disco).
    pub fn reload(&mut self) {
        self.loaded = false;
        self.map.clear();
    }

    /// Guarda la tabla en el disco.
    pub fn save<D: BlockDevice>(
        &self,
        fs: &mut FileSystem<D>,
        now: jarvis_fs::Timestamp,
    ) -> Result<(), jarvis_fs::FsError> {
        if !fs.exists("/Sistema") {
            fs.mkdir("/Sistema", now)?;
        }
        fs.write_file(DB, serialize(&self.map).as_bytes(), now)
    }

    /// Modo y dueño que tiene un archivo sin entrada en la tabla.
    pub fn default_for(path: &str, is_dir: bool) -> Entry {
        let k = key(path);
        if is_system(&k) {
            let mode = if is_dir {
                0o755
            } else if SECRET_FILES.iter().any(|s| k == s.to_lowercase()) {
                0o600
            } else if exec_by_default(&k) {
                0o755
            } else {
                0o644
            };
            Entry {
                mode,
                uid: ROOT,
                gid: ROOT,
            }
        } else if k == "/" {
            Entry {
                mode: 0o755,
                uid: USER,
                gid: USER,
            }
        } else {
            Entry {
                mode: if is_dir { 0o755 } else { 0o644 },
                uid: USER,
                gid: USER,
            }
        }
    }

    pub fn get(&self, path: &str, is_dir: bool) -> Entry {
        self.map
            .get(&key(path))
            .copied()
            .unwrap_or_else(|| Self::default_for(path, is_dir))
    }

    /// Cambia el modo/dueño. Si queda igual al de por defecto, se saca de la tabla.
    pub fn set(&mut self, path: &str, is_dir: bool, e: Entry) {
        let k = key(path);
        if e == Self::default_for(path, is_dir) {
            self.map.remove(&k);
        } else {
            self.map.insert(k, e);
        }
    }

    pub fn set_mode(&mut self, path: &str, is_dir: bool, mode: u16) {
        let mut e = self.get(path, is_dir);
        e.mode = mode & 0o7777;
        self.set(path, is_dir, e);
    }

    /// Un archivo nuevo de un instalador (apt, snap, winget): el modo que traía el paquete,
    /// con root como dueño.
    pub fn install_file(&mut self, path: &str, mode: u16) {
        self.set(
            path,
            false,
            Entry {
                mode: mode & 0o7777,
                uid: ROOT,
                gid: ROOT,
            },
        );
    }

    /// Se movió o renombró algo: su entrada (y la de lo que tenía adentro) lo sigue.
    pub fn renamed(&mut self, from: &str, to: &str) {
        let (f, t) = (key(from), key(to));
        let moved: Vec<(String, Entry)> = self
            .map
            .iter()
            .filter(|(k, _)| under(k, &f))
            .map(|(k, e)| (k.clone(), *e))
            .collect();
        for (k, e) in moved {
            self.map.remove(&k);
            let new = format!("{t}{}", &k[f.len()..]);
            self.map.insert(new, e);
        }
    }

    /// Se borró algo (o fue a la Papelera): se olvidan sus permisos.
    pub fn removed(&mut self, path: &str) {
        let p = key(path);
        self.map.retain(|k, _| !under(k, &p));
    }

    /// ¿Puede `uid` hacer `access` sobre `path`?
    pub fn allowed(&self, uid: u32, path: &str, is_dir: bool, access: Access) -> bool {
        let e = self.get(path, is_dir);
        if uid == ROOT {
            // root lee y escribe todo; para ejecutar un archivo hace falta algún bit x.
            return access != Access::Exec || is_dir || e.mode & 0o111 != 0;
        }
        let shift = if uid == e.uid {
            6
        } else if groups(uid).contains(&e.gid) {
            3
        } else {
            0
        };
        let bit = match access {
            Access::Read => 4,
            Access::Write => 2,
            Access::Exec => 1,
        };
        (e.mode >> shift) & bit != 0
    }

    /// El nombre de un uid.
    pub fn user_name(&self, uid: u32) -> String {
        match uid {
            ROOT => "root".into(),
            USER => self.user.clone(),
            other => other.to_string(),
        }
    }

    pub fn group_name(&self, gid: u32) -> String {
        match gid {
            ROOT => "root".into(),
            SUDO_GROUP => "sudo".into(),
            USER => self.user.clone(),
            other => other.to_string(),
        }
    }

    /// `roman` → 1000, `root` → 0, o un número.
    pub fn uid_of(&self, name: &str) -> Option<u32> {
        match name {
            "root" => Some(ROOT),
            n if n == self.user => Some(USER),
            n => n.parse().ok(),
        }
    }

    pub fn gid_of(&self, name: &str) -> Option<u32> {
        match name {
            "root" => Some(ROOT),
            "sudo" => Some(SUDO_GROUP),
            n if n == self.user => Some(USER),
            n => n.parse().ok(),
        }
    }

    /// `uid=1000(roman) gid=1000(roman) grupos=1000(roman),27(sudo)`, como `id`.
    pub fn id_line(&self, uid: u32) -> String {
        let gs: Vec<String> = groups(uid)
            .iter()
            .map(|g| format!("{g}({})", self.group_name(*g)))
            .collect();
        let gid = if uid == ROOT { ROOT } else { USER };
        format!(
            "uid={uid}({}) gid={gid}({}) grupos={}",
            self.user_name(uid),
            self.group_name(gid),
            gs.join(",")
        )
    }

    /// Cuántas entradas tiene la tabla (para tests y `stat`).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Los grupos de un usuario.
pub fn groups(uid: u32) -> &'static [u32] {
    match uid {
        ROOT => &[ROOT],
        USER => &[USER, SUDO_GROUP],
        _ => &[],
    }
}

/// Archivos del sistema ejecutables aunque no tengan entrada: los de una carpeta `bin`/`sbin`,
/// los programas de Windows instalados y el cargador de bibliotecas de Linux.
fn exec_by_default(k: &str) -> bool {
    let parent = k.rsplit_once('/').map_or("", |(p, _)| p);
    let last_dir = parent.rsplit('/').next().unwrap_or("");
    matches!(last_dir, "bin" | "sbin")
        || k.ends_with(".exe")
        || k.ends_with(".so")
        || k.contains(".so.")
        || k.ends_with(".sh")
}

fn parse(text: &str) -> BTreeMap<String, Entry> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `0755 0 0 /ruta con espacios`
        let mut it = line.splitn(4, ' ');
        let (Some(m), Some(u), Some(g), Some(p)) = (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        let (Ok(mode), Ok(uid), Ok(gid)) = (u16::from_str_radix(m, 8), u.parse(), g.parse()) else {
            continue;
        };
        if p.starts_with('/') {
            map.insert(
                key(p),
                Entry {
                    mode: mode & 0o7777,
                    uid,
                    gid,
                },
            );
        }
    }
    map
}

fn serialize(map: &BTreeMap<String, Entry>) -> String {
    let mut s = String::from("# modo uid gid ruta (lo que difiere de los valores por defecto)\n");
    for (k, e) in map {
        s.push_str(&format!("{:04o} {} {} {k}\n", e.mode, e.uid, e.gid));
    }
    s
}

/// `drwxr-xr-x`, como `ls -l` (con `s`/`t` para setuid, setgid y sticky).
pub fn format_mode(is_dir: bool, mode: u16) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_dir { 'd' } else { '-' });
    let special = [(0o4000, 's', 'S'), (0o2000, 's', 'S'), (0o1000, 't', 'T')];
    for (i, shift) in [6u16, 3, 0].iter().enumerate() {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let (flag, on, off) = special[i];
        s.push(match (bits & 1 != 0, mode & flag != 0) {
            (true, true) => on,
            (false, true) => off,
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// Aplica un modo de `chmod` al modo actual: octal (`755`, `0644`) o simbólico (`u+x`,
/// `go-w`, `a=r`, `+x`, `u=rwx,g=rx,o=`). Como en GNU `chmod`, `X` es "ejecutable si es carpeta
/// o si ya lo era para alguien".
pub fn apply_mode(spec: &str, current: u16, is_dir: bool, umask: u16) -> Result<u16, String> {
    if !spec.is_empty() && spec.chars().all(|c| c.is_digit(8)) {
        if spec.len() > 4 {
            return Err(format!("modo inválido: «{spec}»"));
        }
        return u16::from_str_radix(spec, 8).map_err(|_| format!("modo inválido: «{spec}»"));
    }
    let mut mode = current;
    for clause in spec.split(',') {
        let ops_at = clause
            .find(['+', '-', '='])
            .ok_or_else(|| format!("modo inválido: «{spec}»"))?;
        let (who_s, rest) = clause.split_at(ops_at);
        let mut who: u16 = 0;
        for c in who_s.chars() {
            who |= match c {
                'u' => 0o4700,
                'g' => 0o2070,
                'o' => 0o1007,
                'a' => 0o7777,
                _ => return Err(format!("modo inválido: «{spec}»")),
            };
        }
        // Sin "quién": todos, menos lo que tapa la umask (como chmod).
        let masked = who == 0;
        if masked {
            who = 0o7777;
        }
        // Puede haber varias operaciones seguidas: `u+x-w`.
        let mut chars = rest.chars().peekable();
        while let Some(op) = chars.next() {
            if !matches!(op, '+' | '-' | '=') {
                return Err(format!("modo inválido: «{spec}»"));
            }
            let mut bits: u16 = 0;
            while let Some(&c) = chars.peek() {
                if matches!(c, '+' | '-' | '=') {
                    break;
                }
                chars.next();
                bits |= match c {
                    'r' => 0o444,
                    'w' => 0o222,
                    'x' => 0o111,
                    'X' => {
                        if is_dir || mode & 0o111 != 0 {
                            0o111
                        } else {
                            0
                        }
                    }
                    's' => 0o6000,
                    't' => 0o1000,
                    _ => return Err(format!("modo inválido: «{spec}»")),
                };
            }
            let mut apply = bits & who;
            if masked {
                apply &= !umask;
            }
            match op {
                '+' => mode |= apply,
                '-' => mode &= !apply,
                _ => {
                    // `=`: lo de "quién" queda exactamente así.
                    mode = (mode & !who) | apply;
                }
            }
        }
    }
    Ok(mode & 0o7777)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let e = Perms::default_for("/Sistema/firewall.log", false);
        assert_eq!((e.mode, e.uid), (0o644, ROOT));
        assert_eq!(Perms::default_for("/Sistema/config.ini", false).mode, 0o600);
        assert_eq!(
            Perms::default_for("/Programas/bin/saludo", false).mode,
            0o755
        );
        assert_eq!(
            Perms::default_for("/usr/lib/x86_64-linux-gnu/libc.so.6", false).mode,
            0o755
        );
        let doc = Perms::default_for("/Documentos/notas.txt", false);
        assert_eq!((doc.mode, doc.uid), (0o644, USER));
        assert_eq!(Perms::default_for("/Documentos", true).mode, 0o755);
        // "/Sistemas" no es "/Sistema".
        assert_eq!(Perms::default_for("/Sistemas/x", false).uid, USER);
    }

    #[test]
    fn checks() {
        let p = Perms::new();
        assert!(p.allowed(USER, "/Documentos/a.txt", false, Access::Write));
        assert!(!p.allowed(USER, "/Programas/bin", true, Access::Write));
        assert!(p.allowed(USER, "/Programas/bin/x", false, Access::Exec));
        assert!(!p.allowed(USER, "/Sistema/config.ini", false, Access::Read));
        assert!(p.allowed(ROOT, "/Sistema/config.ini", false, Access::Write));
        // root no ejecuta lo que no tiene ningún bit x.
        assert!(!p.allowed(ROOT, "/Documentos/a.txt", false, Access::Exec));
        assert!(!p.allowed(USER, "/Documentos/a.sh", false, Access::Exec));
    }

    #[test]
    fn table_round_trip_and_rename() {
        let mut p = Perms::new();
        p.set_mode("/Documentos/Script Largo.sh", false, 0o755);
        p.set(
            "/Sistema/x",
            false,
            Entry {
                mode: 0o640,
                uid: ROOT,
                gid: SUDO_GROUP,
            },
        );
        // Volver al de por defecto saca la entrada.
        p.set_mode("/Documentos/b.txt", false, 0o600);
        p.set_mode("/Documentos/b.txt", false, 0o644);
        assert_eq!(p.len(), 2);
        let back = parse(&serialize(&p.map));
        assert_eq!(back, p.map);
        assert_eq!(
            back.get("/documentos/script largo.sh").map(|e| e.mode),
            Some(0o755)
        );

        p.set_mode("/Documentos/dir/a", false, 0o700);
        p.renamed("/Documentos/dir", "/Proyectos/dir2");
        assert_eq!(p.get("/Proyectos/dir2/a", false).mode, 0o700);
        assert_eq!(p.get("/Documentos/dir/a", false).mode, 0o644);
        p.removed("/Proyectos/dir2");
        assert_eq!(p.get("/Proyectos/dir2/a", false).mode, 0o644);
    }

    #[test]
    fn modes() {
        assert_eq!(format_mode(true, 0o755), "drwxr-xr-x");
        assert_eq!(format_mode(false, 0o4755), "-rwsr-xr-x");
        assert_eq!(format_mode(true, 0o1777), "drwxrwxrwt");
        assert_eq!(format_mode(false, 0o600), "-rw-------");
        let m = |s: &str, cur: u16| apply_mode(s, cur, false, 0o022);
        assert_eq!(m("755", 0o644), Ok(0o755));
        assert_eq!(m("+x", 0o644), Ok(0o755));
        assert_eq!(m("u+x", 0o644), Ok(0o744));
        assert_eq!(m("go-r", 0o644), Ok(0o600));
        assert_eq!(m("a=r", 0o755), Ok(0o444));
        assert_eq!(m("u=rwx,g=rx,o=", 0o644), Ok(0o750));
        assert_eq!(m("u+x-w", 0o644), Ok(0o544));
        // `+w` sin quién respeta la umask: no le da escritura al grupo ni a los otros.
        assert_eq!(m("+w", 0o444), Ok(0o644));
        assert_eq!(apply_mode("a+X", 0o644, true, 0), Ok(0o755));
        assert_eq!(apply_mode("a+X", 0o644, false, 0), Ok(0o644));
        assert!(m("99", 0).is_err());
        assert!(m("u*x", 0).is_err());
        assert!(m("12345", 0).is_err());
    }

    #[test]
    fn id() {
        let p = Perms::new();
        assert_eq!(
            p.id_line(USER),
            "uid=1000(roman) gid=1000(roman) grupos=1000(roman),27(sudo)"
        );
        assert_eq!(p.id_line(ROOT), "uid=0(root) gid=0(root) grupos=0(root)");
    }
}
