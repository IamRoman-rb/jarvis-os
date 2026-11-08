//! `jsh`: la shell de la terminal, parecida a `bash`.
//!
//! Entiende tuberías (`|`), redirecciones (`>`, `>>`, `<`, `2>`), `&&`, `||`, `;`, comillas,
//! variables (`$HOME`, `$?`, `$1`…), sustituciones (`$(…)`), comodines (`*.txt`) y alias. Los
//! comandos (`ls`, `cat`, `grep`, `cp`…) están escritos acá mismo, sobre el FAT32 propio. Los
//! "programas" que se instalan con `apt` son scripts de `jsh` o, desde K11, programas de Linux
//! (ELF estáticos): esos corren en su propio proceso (ADR 0010) y la shell espera a que terminen
//! ([`Job::Process`]), mostrando su salida a medida que llega.
//!
//! **Usuarios** (ADR 0020): la shell corre como el usuario (uid 1000). `sudo` pide la contraseña
//! (la del asistente de instalación) y la recuerda 15 minutos, como `sudo-rs`; mientras corre lo
//! que se le pidió, el uid efectivo es 0 ([`Shell::euid`]). `su` deja la terminal como root
//! (`#` en el prompt) hasta `exit`.
//!
//! Los comandos que usan la red (`curl`, `wget`, `apt`, `ping`) no pueden esperar bloqueando (el
//! escritorio es un solo hilo): piden la descarga por el `Outbox` y la shell queda **en espera**
//! ([`Shell::waiting`]) hasta que llega la respuesta ([`Shell::net_response`]). Ahí sigue con el
//! resto de la tubería y de la línea.

pub mod apt;
pub mod binfmt;
mod cmds;
pub mod debian;
pub mod parse;
mod regex;
pub mod snap;
pub mod winget;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;

use crate::apps::Ctx;
use crate::files::join;
use crate::system::HttpResponse;
use parse::{Connector, Part, Pipeline, Redir, Word};

/// Colores ANSI (la terminal los entiende, como cualquier terminal de Linux).
pub mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RED: &str = "\x1b[91m";
    pub const GREEN: &str = "\x1b[92m";
    pub const YELLOW: &str = "\x1b[93m";
    pub const BLUE: &str = "\x1b[94m";
    pub const MAGENTA: &str = "\x1b[95m";
    pub const CYAN: &str = "\x1b[96m";
    pub const DIM: &str = "\x1b[90m";
    pub const DIR: &str = "\x1b[1;94m";
    pub const EXEC: &str = "\x1b[1;92m";
}

/// Carpeta personal (`~`). El disco de JARVIS-OS tiene todo en la raíz.
pub const HOME: &str = "/";
/// Dónde quedan los programas instalados con `apt`.
pub const BIN: &str = "/Programas/bin";
/// El `PATH`: los programas de `apt` (los de JARVIS-OS y los de Debian) y los de `snap`.
pub const PATH: &str = "/Programas/bin:/snap/bin:/usr/local/bin:/usr/bin:/bin";
const MAX_DEPTH: u32 = 8;

/// Cuánto recuerda `sudo` la contraseña (como el `timestamp_timeout` de sudo: 15 minutos).
pub const SUDO_TIMEOUT_MS: u64 = 15 * 60 * 1000;
/// Intentos de contraseña antes de rendirse (como sudo).
const SUDO_TRIES: u8 = 3;

/// Una contraseña que la terminal está pidiendo (sin mostrar lo que se tipea).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    /// `sudo …`: la línea entera se corre cuando la contraseña es correcta.
    Sudo { line: String, tries: u8 },
    /// `su`: la terminal pasa a ser de root.
    Su { tries: u8 },
    /// `passwd`: 0 = la actual, 1 = la nueva, 2 = la nueva otra vez.
    Passwd { step: u8, new: String },
}

/// Lo que devuelve un comando.
pub(crate) enum Res {
    Code(i32),
    /// Tiene que esperar (la red, `sleep`).
    Wait(Job),
}

/// Un trabajo en espera.
pub(crate) enum Job {
    /// `curl` / `wget`: al llegar, se guarda en `save` o sale por la salida estándar.
    Fetch {
        id: u32,
        url: String,
        save: Option<String>,
        quiet: bool,
        since: u64,
    },
    /// `ping`: se mide cuánto tarda la respuesta HTTP.
    Ping {
        id: u32,
        host: String,
        since: u64,
    },
    Apt(alloc::boxed::Box<apt::AptJob>),
    Snap(alloc::boxed::Box<snap::SnapJob>),
    Winget(alloc::boxed::Box<winget::WingetJob>),
    /// `antivirus actualizar`: bajando las firmas.
    Antivirus {
        id: u32,
    },
    Sleep {
        until: u64,
    },
    /// Un programa de Linux corriendo (K11). `buf`: su salida, si no va directo a la terminal
    /// (una tubería o una redirección).
    Process {
        pid: u32,
        buf: String,
    },
}

impl Job {
    fn net_id(&self) -> Option<u32> {
        match self {
            Job::Fetch { id, .. } | Job::Ping { id, .. } | Job::Antivirus { id } => Some(*id),
            Job::Apt(a) => a.waiting_id(),
            Job::Snap(s) => s.waiting_id(),
            Job::Winget(w) => w.waiting_id(),
            Job::Sleep { .. } | Job::Process { .. } => None,
        }
    }
}

/// Dónde va la salida de un comando que quedó esperando.
#[derive(Clone)]
enum OutTarget {
    Terminal,
    File(String, bool),
    Null,
}

struct Pending {
    job: Job,
    list: Vec<Pipeline>,
    index: usize,
    stage: usize,
    target: OutTarget,
}

/// Salidas: la terminal y, adentro de `$(…)`, lo que se captura.
pub(crate) struct Out {
    pub term: String,
    capture: Vec<String>,
}

impl Out {
    fn new() -> Out {
        Out {
            term: String::new(),
            capture: Vec::new(),
        }
    }

    /// Salida estándar final de una tubería.
    fn stdout(&mut self, s: &str) {
        match self.capture.last_mut() {
            Some(c) => c.push_str(s),
            None => self.term.push_str(s),
        }
    }

    /// Errores y mensajes de progreso: siempre a la terminal (en rojo los errores).
    pub fn err(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.term.push_str(ansi::RED);
        self.term.push_str(s);
        self.term.push_str(ansi::RESET);
        if !s.ends_with('\n') {
            self.term.push('\n');
        }
    }

    pub fn info(&mut self, s: &str) {
        self.term.push_str(s);
        if !s.ends_with('\n') {
            self.term.push('\n');
        }
    }
}

pub struct Shell {
    pub cwd: String,
    prev_dir: String,
    vars: Vec<(String, String)>,
    aliases: Vec<(String, String)>,
    pub history: Vec<String>,
    /// Código de salida del último comando (`$?`).
    pub status: i32,
    /// Argumentos del script que se está ejecutando (`$1`, `$@`).
    args: Vec<String>,
    pending: Option<Pending>,
    /// Adentro de `$(…)` o de un script: no se puede esperar a la red.
    no_wait: u32,
    depth: u32,
    /// La salida del comando actual va a la terminal (no a una tubería ni a un archivo).
    pub(crate) tty: bool,
    /// Columnas de la terminal (para `ls`).
    pub cols: usize,
    /// Pedidos para la terminal.
    pub clear: bool,
    pub exit: bool,
    /// Estado del generador de `$RANDOM` (xorshift).
    rng: core::cell::Cell<u64>,
    /// La entrada del script que se está ejecutando (`fortune | cowsay`): la reciben los
    /// comandos del script que no tienen otra.
    script_stdin: Option<String>,
    /// El usuario efectivo del comando que corre: 1000, o 0 adentro de `sudo` o después de `su`.
    pub euid: u32,
    /// El usuario de la terminal (0 después de `su`, hasta `exit`).
    base_uid: u32,
    /// Hasta cuándo `sudo` no vuelve a pedir la contraseña (0: la pide).
    sudo_until: u64,
    /// La contraseña que se está pidiendo.
    pub(crate) auth: Option<Auth>,
    /// La máscara de los archivos nuevos (`umask`).
    pub(crate) umask: u16,
}

impl Shell {
    pub fn new(user: &str, host: &str) -> Shell {
        let vars = [
            ("HOME", HOME),
            ("USER", user),
            ("LOGNAME", user),
            ("HOSTNAME", host),
            ("PATH", PATH),
            ("SHELL", "/bin/jsh"),
            ("TERM", "jarvis-256color"),
            ("LANG", "es_AR.UTF-8"),
            ("PWD", "/"),
        ];
        let aliases = [
            ("ll", "ls -l"),
            ("la", "ls -a"),
            ("l", "ls"),
            ("dir", "ls"),
            ("cls", "clear"),
            ("del", "rm"),
            ("copy", "cp"),
            ("move", "mv"),
            ("ipconfig", "ip a"),
            ("ifconfig", "ip a"),
            ("apt-get", "apt"),
            ("jpm", "apt"),
            ("vi", "nano"),
            ("vim", "nano"),
        ];
        Shell {
            cwd: HOME.into(),
            prev_dir: HOME.into(),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            aliases: aliases
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            history: Vec::new(),
            status: 0,
            args: Vec::new(),
            pending: None,
            no_wait: 0,
            depth: 0,
            tty: true,
            cols: 80,
            clear: false,
            exit: false,
            rng: core::cell::Cell::new(0x2545_f491_4f6c_dd1d),
            script_stdin: None,
            euid: crate::perms::USER,
            base_uid: crate::perms::USER,
            sudo_until: 0,
            auth: None,
            umask: 0o022,
        }
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn set_var(&mut self, name: &str, value: &str) {
        match self.vars.iter_mut().find(|(k, _)| k == name) {
            Some(v) => v.1 = value.into(),
            None => self.vars.push((name.into(), value.into())),
        }
    }

    pub(crate) fn unset_var(&mut self, name: &str) {
        self.vars.retain(|(k, _)| k != name);
    }

    pub fn user(&self) -> &str {
        self.var("USER").unwrap_or("roman")
    }

    /// El nombre del usuario efectivo (`root` adentro de `sudo`).
    pub fn effective_user(&self) -> &str {
        if self.euid == crate::perms::ROOT {
            "root"
        } else {
            self.user()
        }
    }

    /// ¿La terminal es de root (después de `su`)?
    pub fn is_root_shell(&self) -> bool {
        self.base_uid == crate::perms::ROOT
    }

    /// ¿Está pidiendo una contraseña? (La terminal no muestra lo que se tipea.)
    pub fn wants_password(&self) -> bool {
        self.auth.is_some()
    }

    pub fn host(&self) -> &str {
        self.var("HOSTNAME").unwrap_or("jarvis")
    }

    /// La carpeta actual como la muestra el prompt (`~` en la carpeta personal).
    pub fn short_cwd(&self) -> String {
        if self.cwd == HOME {
            "~".into()
        } else {
            self.cwd.clone()
        }
    }

    /// `roman@jarvis:~$ ` con colores, como Ubuntu (`root@jarvis:~# ` después de `su`).
    pub fn prompt(&self) -> String {
        let (user, sign) = if self.is_root_shell() {
            ("root", '#')
        } else {
            (self.user(), '$')
        };
        format!(
            "{}{}@{}{}:{}{}{}{sign} ",
            if self.is_root_shell() {
                ansi::RED
            } else {
                ansi::EXEC
            },
            user,
            self.host(),
            ansi::RESET,
            ansi::DIR,
            self.short_cwd(),
            ansi::RESET
        )
    }

    /// ¿Está esperando a la red (o a `sleep`, o una contraseña)?
    pub fn waiting(&self) -> bool {
        self.pending.is_some() || self.auth.is_some()
    }

    /// ¿`sudo` puede correr sin pedir la contraseña ahora? (Recién dada, sin contraseña
    /// configurada, o la terminal ya es de root.)
    pub(crate) fn sudo_ok<D: BlockDevice>(&self, ctx: &Ctx<'_, D>) -> bool {
        self.is_root_shell()
            || ctx.config.pin.is_none()
            || (self.sudo_until != 0 && ctx.now_ms < self.sudo_until)
    }

    /// ¿La línea usa `sudo` (en cualquiera de sus comandos)?
    fn uses_sudo(line: &str) -> bool {
        match parse::parse(line) {
            Ok(list) => list.iter().any(|p| {
                p.commands.iter().any(|c| {
                    c.words
                        .first()
                        .is_some_and(|w| matches!(w.as_slice(), [Part::Lit(s, _)] if s == "sudo"))
                })
            }),
            Err(_) => false,
        }
    }

    /// Lo que se tipeó mientras se pedía una contraseña. Devuelve lo que hay que mostrar.
    pub fn password<D: BlockDevice>(&mut self, typed: &str, ctx: &mut Ctx<'_, D>) -> String {
        let mut out = Out::new();
        out.term.push('\n');
        let Some(auth) = self.auth.take() else {
            return out.term;
        };
        let user = self.user().to_string();
        let right = ctx.config.pin.as_ref().is_none_or(|h| h.verify(typed));
        match auth {
            Auth::Sudo { line, tries } => {
                if right {
                    self.sudo_until = ctx.now_ms + SUDO_TIMEOUT_MS;
                    ctx.log.push("SUDO_OK".into());
                    self.run_text(&line, ctx, &mut out);
                } else if tries + 1 >= SUDO_TRIES {
                    out.err(&format!(
                        "sudo: {SUDO_TRIES} intentos de contraseña incorrectos"
                    ));
                    ctx.log.push("SUDO_FALLO".into());
                    self.status = 1;
                } else {
                    out.term.push_str("Lo siento, probá otra vez.\n");
                    out.term
                        .push_str(&format!("[sudo] contraseña para {user}: "));
                    self.auth = Some(Auth::Sudo {
                        line,
                        tries: tries + 1,
                    });
                }
            }
            Auth::Su { tries } => {
                if right {
                    self.base_uid = crate::perms::ROOT;
                    self.euid = crate::perms::ROOT;
                    self.status = 0;
                    ctx.log.push("SU_ROOT".into());
                } else if tries + 1 >= SUDO_TRIES {
                    out.err("su: fallo de autenticación");
                    self.status = 1;
                } else {
                    out.term.push_str("su: contraseña incorrecta\nContraseña: ");
                    self.auth = Some(Auth::Su { tries: tries + 1 });
                }
            }
            Auth::Passwd { step, new } => match step {
                0 if !right => {
                    out.err("passwd: fallo de autenticación\npasswd: no se cambió la contraseña");
                    self.status = 10;
                }
                0 => {
                    out.term.push_str("Nueva contraseña: ");
                    self.auth = Some(Auth::Passwd {
                        step: 1,
                        new: String::new(),
                    });
                }
                1 if typed.chars().count() < 4 => {
                    out.err("CONTRASEÑA INCORRECTA: tiene que tener al menos 4 caracteres\npasswd: no se cambió la contraseña");
                    self.status = 10;
                }
                1 => {
                    out.term.push_str("Vuelva a escribir la nueva contraseña: ");
                    self.auth = Some(Auth::Passwd {
                        step: 2,
                        new: typed.to_string(),
                    });
                }
                _ if typed != new => {
                    out.err("Las contraseñas no coinciden.\npasswd: no se cambió la contraseña");
                    self.status = 10;
                }
                _ => {
                    let mut cfg = ctx.config.clone();
                    let extra = ctx.now_ms.to_le_bytes();
                    cfg.pin = crate::pin::hash_pin(&new, &extra);
                    ctx.out.config = Some(cfg);
                    out.term
                        .push_str("passwd: contraseña actualizada correctamente\n");
                    ctx.log.push("PASSWD_OK".into());
                    self.status = 0;
                }
            },
        }
        if self.auth.is_none() && self.pending.is_none() {
            // Sin más que pedir: el prompt vuelve (lo agrega la terminal).
        }
        out.term
    }

    /// Ctrl+C mientras se pide una contraseña.
    pub fn cancel_password(&mut self) -> bool {
        if self.auth.take().is_some() {
            self.status = 1;
            true
        } else {
            false
        }
    }

    /// Empieza a pedir una contraseña.
    pub(crate) fn ask(&mut self, auth: Auth, prompt: &str, out: &mut Out) {
        out.term.push_str(prompt);
        self.auth = Some(auth);
    }

    /// `exit` en una terminal de root (después de `su`): vuelve al usuario.
    pub(crate) fn leave_root(&mut self) -> bool {
        if self.is_root_shell() {
            self.base_uid = crate::perms::USER;
            self.euid = crate::perms::USER;
            true
        } else {
            false
        }
    }

    pub(crate) fn sudo_reset(&mut self) {
        self.sudo_until = 0;
    }

    /// La tabla de permisos, cargada.
    pub(crate) fn perms<'b, D: BlockDevice>(
        ctx: &'b mut Ctx<'_, D>,
    ) -> &'b mut crate::perms::Perms {
        if let Some(fs) = ctx.fs.as_deref_mut() {
            ctx.perms.ensure_loaded(fs);
        }
        if ctx.perms.user != ctx.config.user {
            ctx.perms.user = ctx.config.user.clone();
        }
        ctx.perms
    }

    /// ¿El usuario efectivo NO puede hacer `a` sobre `path`? (`path` tiene que existir.)
    pub(crate) fn denied<D: BlockDevice>(
        &self,
        ctx: &mut Ctx<'_, D>,
        path: &str,
        a: crate::perms::Access,
    ) -> bool {
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return false;
        };
        let is_dir = path == "/" || fs.stat(path).is_ok_and(|s| s.is_dir);
        let uid = self.euid;
        !Self::perms(ctx).allowed(uid, path, is_dir, a)
    }

    /// ¿NO puede escribir `path`? Si existe, el archivo; si no, la carpeta donde se crearía.
    pub(crate) fn denied_write<D: BlockDevice>(&self, ctx: &mut Ctx<'_, D>, path: &str) -> bool {
        let exists = ctx.fs.as_deref_mut().is_some_and(|fs| fs.exists(path));
        if exists {
            self.denied(ctx, path, crate::perms::Access::Write)
        } else {
            self.denied(
                ctx,
                &crate::files::parent(path),
                crate::perms::Access::Write,
            )
        }
    }

    /// ¿NO puede crear, borrar o renombrar algo en la carpeta de `path`?
    pub(crate) fn denied_parent<D: BlockDevice>(&self, ctx: &mut Ctx<'_, D>, path: &str) -> bool {
        self.denied(
            ctx,
            &crate::files::parent(path),
            crate::perms::Access::Write,
        )
    }

    /// Ruta absoluta y normalizada (`..`, `.`, `~`).
    pub fn abs(&self, p: &str) -> String {
        let p = p.trim();
        let full = if let Some(rest) = p.strip_prefix('~') {
            format!("{HOME}/{rest}")
        } else if p.starts_with('/') {
            p.to_string()
        } else {
            format!("{}/{p}", self.cwd)
        };
        let mut parts: Vec<&str> = Vec::new();
        for seg in full.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        if parts.is_empty() {
            "/".into()
        } else {
            let mut s = String::new();
            for p in parts {
                s.push('/');
                s.push_str(p);
            }
            s
        }
    }

    pub(crate) fn set_cwd(&mut self, dir: String) {
        self.prev_dir = core::mem::replace(&mut self.cwd, dir);
        let cwd = self.cwd.clone();
        self.set_var("PWD", &cwd);
    }

    // --- ejecutar -----------------------------------------------------------------------------

    /// Ejecuta una línea escrita por el usuario. Devuelve lo que hay que mostrar.
    pub fn run<D: BlockDevice>(&mut self, line: &str, ctx: &mut Ctx<'_, D>) -> String {
        let mut out = Out::new();
        if self.pending.is_some() {
            out.err("jsh: todavía hay un comando en curso (Ctrl+C lo cancela)");
            return out.term;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            self.history.push(trimmed.to_string());
        }
        // El momento en que se escribe cada comando mezcla el generador de $RANDOM.
        self.rng
            .set(self.rng.get() ^ ctx.now_ms.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // `sudo` sin la contraseña reciente: se pide antes de correr la línea (como sudo, que la
        // pide al empezar).
        let expanded = self.expand_alias(trimmed);
        if !self.sudo_ok(ctx) && Self::uses_sudo(&expanded) {
            let user = self.user().to_string();
            self.ask(
                Auth::Sudo {
                    line: trimmed.to_string(),
                    tries: 0,
                },
                &format!("[sudo] contraseña para {user}: "),
                &mut out,
            );
            return out.term;
        }
        self.run_text(trimmed, ctx, &mut out);
        out.term
    }

    fn run_text<D: BlockDevice>(&mut self, line: &str, ctx: &mut Ctx<'_, D>, out: &mut Out) {
        let line = self.expand_alias(line);
        match parse::parse(&line) {
            Ok(list) => self.run_list(list, 0, None, ctx, out),
            Err(e) => {
                out.err(&format!("jsh: error de sintaxis: {e}"));
                self.status = 2;
            }
        }
    }

    /// Reemplaza el primer comando si es un alias (una sola vez, como bash).
    fn expand_alias(&self, line: &str) -> String {
        let trimmed = line.trim_start();
        let first = trimmed
            .split(|c: char| c.is_whitespace() || c == ';' || c == '|')
            .next()
            .unwrap_or("");
        match self.aliases.iter().find(|(k, _)| k == first) {
            Some((_, v)) => format!("{v}{}", &trimmed[first.len()..]),
            None => line.to_string(),
        }
    }

    fn run_list<D: BlockDevice>(
        &mut self,
        list: Vec<Pipeline>,
        mut i: usize,
        mut resume: Option<(usize, Option<String>)>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) {
        while i < list.len() && !self.exit {
            let run = resume.is_some()
                || i == 0
                || match list[i - 1].next {
                    Connector::Always => true,
                    Connector::And => self.status == 0,
                    Connector::Or => self.status != 0,
                };
            if run {
                let (stage, input) = resume.take().unwrap_or((0, None));
                if let Some((job, stage, target)) =
                    self.run_pipeline(&list[i], stage, input, ctx, out)
                {
                    self.pending = Some(Pending {
                        job,
                        list,
                        index: i,
                        stage,
                        target,
                    });
                    return;
                }
            }
            i += 1;
        }
    }

    /// Ejecuta una tubería desde el comando `start`. Si uno queda esperando, devuelve el
    /// trabajo, en qué comando quedó y a dónde va su salida.
    fn run_pipeline<D: BlockDevice>(
        &mut self,
        p: &Pipeline,
        start: usize,
        input: Option<String>,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Option<(Job, usize, OutTarget)> {
        let n = p.commands.len();
        let mut data = input;
        if start == 0 && data.is_none() {
            data = self.script_stdin.clone();
        }
        for k in start..n {
            let cmd = &p.commands[k];
            let last = k + 1 == n;
            let argv = match self.expand_words(&cmd.words, ctx, out) {
                Ok(v) => v,
                Err(e) => {
                    out.err(&format!("jsh: {e}"));
                    self.status = 1;
                    return None;
                }
            };
            let mut stdin = data.take();
            let mut target = if last {
                OutTarget::Terminal
            } else {
                OutTarget::Null
            };
            let mut err_target = OutTarget::Terminal;
            let mut err_to_out = false;
            for r in &cmd.redirs {
                let word = match r {
                    Redir::Out(w) | Redir::Append(w) | Redir::In(w) | Redir::Err(w) => {
                        match self.expand_words(core::slice::from_ref(w), ctx, out) {
                            Ok(v) if v.len() == 1 => Some(v[0].clone()),
                            _ => {
                                out.err("jsh: redirección ambigua");
                                self.status = 1;
                                return None;
                            }
                        }
                    }
                    Redir::ErrToOut => None,
                };
                let path = word.as_deref().map(|w| {
                    if w == "/dev/null" {
                        w.to_string()
                    } else {
                        self.abs(w)
                    }
                });
                match (r, path) {
                    (Redir::In(_), Some(p)) => {
                        let Some(fs) = ctx.fs.as_deref_mut() else {
                            out.err("jsh: no hay disco");
                            self.status = 1;
                            return None;
                        };
                        match fs.read_file(&p) {
                            Ok(b) => stdin = Some(String::from_utf8_lossy(&b).into_owned()),
                            Err(e) => {
                                out.err(&format!("jsh: {p}: {e}"));
                                self.status = 1;
                                return None;
                            }
                        }
                    }
                    (Redir::Out(_) | Redir::Append(_), Some(p)) => {
                        target = if p == "/dev/null" {
                            OutTarget::Null
                        } else {
                            OutTarget::File(p, matches!(r, Redir::Append(_)))
                        };
                    }
                    (Redir::Err(_), Some(p)) => {
                        err_target = if p == "/dev/null" {
                            OutTarget::Null
                        } else {
                            OutTarget::File(p, false)
                        };
                    }
                    (Redir::ErrToOut, _) => err_to_out = true,
                    _ => {}
                }
            }
            if argv.is_empty() {
                // Solo redirecciones: `> archivo` crea un archivo vacío.
                if let OutTarget::File(p, append) = &target {
                    self.write_target(p, "", *append, ctx, out);
                }
                continue;
            }
            self.tty = last && matches!(target, OutTarget::Terminal) && out.capture.is_empty();
            let mut o = String::new();
            let mut e = String::new();
            let res = self.exec(&argv, stdin.as_deref(), &mut o, &mut e, ctx, out);
            if err_to_out {
                o.push_str(&e);
            } else {
                match &err_target {
                    OutTarget::Terminal => out.err(&e),
                    OutTarget::File(p, _) => {
                        let p = p.clone();
                        self.write_target(&p, &e, false, ctx, out);
                    }
                    OutTarget::Null => {}
                }
            }
            match res {
                Res::Code(c) => self.status = c,
                Res::Wait(job) => {
                    if !o.is_empty() {
                        out.info(&o);
                    }
                    if self.no_wait > 0 {
                        out.err(&format!(
                            "jsh: {}: los comandos de red no se pueden usar adentro de $(...) ni en scripts (todavía)",
                            argv[0]
                        ));
                        self.status = 1;
                        return None;
                    }
                    return Some((job, k, target));
                }
            }
            if last {
                self.deliver(&o, &target, ctx, out);
            } else {
                data = Some(o);
            }
        }
        None
    }

    fn deliver<D: BlockDevice>(
        &mut self,
        text: &str,
        target: &OutTarget,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) {
        match target {
            OutTarget::Terminal => out.stdout(text),
            OutTarget::File(p, append) => {
                let p = p.clone();
                self.write_target(&p, text, *append, ctx, out);
            }
            OutTarget::Null => {}
        }
    }

    fn write_target<D: BlockDevice>(
        &mut self,
        path: &str,
        text: &str,
        append: bool,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) {
        let now = ctx.timestamp();
        if self.denied_write(ctx, path) {
            out.err(&format!("jsh: {path}: Permiso denegado"));
            self.status = 1;
            return;
        }
        let Some(fs) = ctx.fs.as_deref_mut() else {
            out.err("jsh: no hay disco");
            return;
        };
        let mut data = if append {
            fs.read_file(path).unwrap_or_default()
        } else {
            Vec::new()
        };
        data.extend_from_slice(text.as_bytes());
        if let Err(e) = fs.write_file(path, &data, now) {
            out.err(&format!("jsh: {path}: {e}"));
            self.status = 1;
        }
    }

    /// Ejecuta `line` y devuelve su salida estándar (para `$(…)`).
    fn capture<D: BlockDevice>(
        &mut self,
        line: &str,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> String {
        if self.depth >= MAX_DEPTH {
            out.err("jsh: demasiadas sustituciones anidadas");
            return String::new();
        }
        self.depth += 1;
        self.no_wait += 1;
        out.capture.push(String::new());
        let saved_tty = self.tty;
        self.run_text(line, ctx, out);
        self.tty = saved_tty;
        self.no_wait -= 1;
        self.depth -= 1;
        out.capture.pop().unwrap_or_default()
    }

    /// Ejecuta un script (un archivo de texto con comandos, uno por línea). Lo que escriben
    /// sus comandos queda en `o`: es la salida del script (sigue por la tubería, si hay).
    pub(crate) fn run_script<D: BlockDevice>(
        &mut self,
        text: &str,
        args: Vec<String>,
        stdin: Option<&str>,
        o: &mut String,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> i32 {
        if self.depth >= MAX_DEPTH {
            out.err("jsh: demasiados scripts anidados");
            return 1;
        }
        out.capture.push(String::new());
        self.depth += 1;
        self.no_wait += 1;
        let saved = core::mem::replace(&mut self.args, args);
        let saved_stdin = core::mem::replace(&mut self.script_stdin, stdin.map(String::from));
        self.status = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            self.run_text(line, ctx, out);
            if self.exit {
                // `exit` adentro de un script termina el script, no la terminal.
                self.exit = false;
                break;
            }
        }
        self.args = saved;
        self.script_stdin = saved_stdin;
        self.no_wait -= 1;
        self.depth -= 1;
        o.push_str(&out.capture.pop().unwrap_or_default());
        self.status
    }

    // --- expansión ----------------------------------------------------------------------------

    fn var_value(&self, name: &str) -> String {
        match name {
            "?" => self.status.to_string(),
            "#" => self.args.len().saturating_sub(1).to_string(),
            "@" => self.args.get(1..).unwrap_or(&[]).join(" "),
            "$" => "1".into(),
            "RANDOM" => {
                let mut x = self.rng.get();
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.rng.set(x);
                (x % 32768).to_string()
            }
            n if n.chars().all(|c| c.is_ascii_digit()) => n
                .parse::<usize>()
                .ok()
                .and_then(|i| self.args.get(i))
                .cloned()
                .unwrap_or_default(),
            n => self.var(n).unwrap_or("").to_string(),
        }
    }

    fn expand_words<D: BlockDevice>(
        &mut self,
        words: &[Word],
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) -> Result<Vec<String>, String> {
        let mut result = Vec::new();
        for w in words {
            // `~` al principio de una palabra sin comillas.
            let mut fields: Vec<String> = alloc::vec![String::new()];
            let mut produced = false; // hubo algo con comillas o literal (la palabra existe)
            // `NOMBRE=…`: una asignación. Lo que sigue no se parte en palabras.
            let assignment = matches!(w.first(), Some(Part::Lit(s, false)) if s
                .split_once('=')
                .is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')));
            for (i, part) in w.iter().enumerate() {
                match part {
                    Part::Lit(s, quoted) => {
                        produced = true;
                        let s = if i == 0 && !quoted && (s == "~" || s.starts_with("~/")) {
                            format!("{HOME}{}", &s[1..]).replace("//", "/")
                        } else {
                            s.clone()
                        };
                        fields.last_mut().unwrap().push_str(&s);
                    }
                    Part::Var(name, quoted) => {
                        let v = self.var_value(name);
                        if *quoted || assignment {
                            produced = true;
                            fields.last_mut().unwrap().push_str(&v);
                        } else {
                            push_split(&mut fields, &v, &mut produced);
                        }
                    }
                    Part::Sub(cmd, quoted) => {
                        let v = self.capture(cmd, ctx, out);
                        let v = v.trim_end_matches('\n');
                        if *quoted || assignment {
                            produced = true;
                            fields.last_mut().unwrap().push_str(v);
                        } else {
                            push_split(&mut fields, v, &mut produced);
                        }
                    }
                }
            }
            if !produced && fields.iter().all(|f| f.is_empty()) {
                continue;
            }
            if parse::has_glob(w) && fields.len() == 1 {
                let matches = self.glob(&fields[0], ctx);
                if !matches.is_empty() {
                    result.extend(matches);
                    continue;
                }
            }
            result.extend(fields);
        }
        Ok(result)
    }

    /// `Documentos/*.txt` → las rutas que coinciden (en el formato en que se escribieron).
    fn glob<D: BlockDevice>(&self, pattern: &str, ctx: &mut Ctx<'_, D>) -> Vec<String> {
        let (dir_part, pat) = match pattern.rsplit_once('/') {
            Some((d, p)) => (Some(if d.is_empty() { "/" } else { d }), p),
            None => (None, pattern),
        };
        let dir = self.abs(dir_part.unwrap_or("."));
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return Vec::new();
        };
        let mut names: Vec<String> = fs
            .list(&dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| (!e.name.starts_with('.') || pat.starts_with('.')) && !e.hidden)
            .filter(|e| parse::glob_match(pat, &e.name))
            .map(|e| match dir_part {
                Some("/") => format!("/{}", e.name),
                Some(d) => format!("{d}/{}", e.name),
                None => e.name,
            })
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    // --- red y esperas ------------------------------------------------------------------------

    /// Llegó una respuesta de la red. Devuelve lo que hay que mostrar (o `None` si no era
    /// para esta shell).
    pub fn net_response<D: BlockDevice>(
        &mut self,
        id: u32,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
    ) -> Option<String> {
        let p = self.pending.as_ref()?;
        if p.job.net_id() != Some(id) {
            return None;
        }
        let mut pending = self.pending.take()?;
        let mut out = Out::new();
        let finished = match &mut pending.job {
            Job::Fetch {
                url,
                save,
                quiet,
                since,
                ..
            } => {
                let r =
                    cmds::finish_fetch(url, save.as_deref(), *quiet, *since, result, ctx, &mut out);
                Some(r)
            }
            Job::Ping { host, since, .. } => {
                let ms = ctx.now_ms.saturating_sub(*since);
                Some(match result {
                    Ok(r) => {
                        out.info(&format!(
                            "Respuesta de {host}: HTTP {} en {ms} ms ({} bytes)\n\n--- {host}: 1 pedido, 1 respuesta, 0 % perdidos ---",
                            r.status,
                            r.body.len()
                        ));
                        (0, String::new())
                    }
                    Err(e) => {
                        out.err(&format!("ping: {host}: {e}"));
                        (1, String::new())
                    }
                })
            }
            Job::Apt(job) => job.on_response(id, result, ctx, &mut out),
            Job::Snap(job) => job.on_response(result, ctx, &mut out),
            Job::Winget(job) => job.on_response(result, ctx, &mut out),
            Job::Antivirus { .. } => Some(crate::antivirus::on_update(result, ctx, &mut out)),
            Job::Sleep { .. } | Job::Process { .. } => Some((0, String::new())),
        };
        match finished {
            None => self.pending = Some(pending), // apt sigue con otra descarga
            Some((code, data)) => self.resume(pending, code, data, ctx, &mut out),
        }
        Some(out.term)
    }

    /// Una vez por frame (para `sleep`).
    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) -> Option<String> {
        let Some(Pending {
            job: Job::Sleep { until },
            ..
        }) = &self.pending
        else {
            return None;
        };
        if ctx.now_ms < *until {
            return None;
        }
        let pending = self.pending.take()?;
        let mut out = Out::new();
        self.resume(pending, 0, String::new(), ctx, &mut out);
        Some(out.term)
    }

    /// El trabajo terminó: su salida sigue por la tubería y después el resto de la línea.
    fn resume<D: BlockDevice>(
        &mut self,
        p: Pending,
        code: i32,
        data: String,
        ctx: &mut Ctx<'_, D>,
        out: &mut Out,
    ) {
        self.status = code;
        let n = p.list[p.index].commands.len();
        if p.stage + 1 < n {
            self.run_list(p.list, p.index, Some((p.stage + 1, Some(data))), ctx, out);
        } else {
            self.deliver(&data, &p.target, ctx, out);
            self.run_list(p.list, p.index + 1, None, ctx, out);
        }
    }

    /// El programa de Linux que está corriendo en primer plano.
    pub fn process_pid(&self) -> Option<u32> {
        match &self.pending {
            Some(Pending {
                job: Job::Process { pid, .. },
                ..
            }) => Some(*pid),
            _ => None,
        }
    }

    /// Salida del programa `pid`. Si va a la terminal, se devuelve para mostrarla ya; si va a
    /// una tubería o a un archivo, se guarda hasta que termine.
    pub fn proc_output(&mut self, pid: u32, data: &[u8]) -> Option<String> {
        let p = self.pending.as_mut()?;
        let last = p.stage + 1 == p.list[p.index].commands.len();
        let direct = last && matches!(p.target, OutTarget::Terminal);
        let Job::Process { pid: running, buf } = &mut p.job else {
            return None;
        };
        if *running != pid {
            return None;
        }
        let text = String::from_utf8_lossy(data).into_owned();
        if direct {
            Some(text)
        } else {
            buf.push_str(&text);
            Some(String::new())
        }
    }

    /// Terminó el programa `pid`: la línea sigue (el resto de la tubería, `&&`…).
    pub fn proc_exit<D: BlockDevice>(
        &mut self,
        pid: u32,
        code: i32,
        why: Option<&str>,
        ctx: &mut Ctx<'_, D>,
    ) -> Option<String> {
        if self.process_pid() != Some(pid) {
            return None;
        }
        let mut pending = self.pending.take()?;
        let data = match &mut pending.job {
            Job::Process { buf, .. } => core::mem::take(buf),
            _ => String::new(),
        };
        let mut out = Out::new();
        if let Some(w) = why
            && code != 130
        {
            out.err(w);
        }
        self.resume(pending, code, data, ctx, &mut out);
        Some(out.term)
    }

    /// Ctrl+C: se deja de esperar (la respuesta, si llega, se ignora).
    pub fn cancel(&mut self) -> bool {
        if self.pending.take().is_some() {
            self.status = 130;
            true
        } else {
            false
        }
    }

    // --- completar con Tab --------------------------------------------------------------------

    /// Candidatos para completar la última palabra de `line`.
    pub fn complete<D: BlockDevice>(&self, line: &str, ctx: &mut Ctx<'_, D>) -> Vec<String> {
        let start = line
            .rfind(|c: char| c.is_whitespace() || c == '|' || c == ';' || c == '>')
            .map_or(0, |i| i + 1);
        let word = &line[start..];
        let first =
            line[..start].trim().is_empty() || line[..start].trim_end().ends_with(['|', ';', '&']);
        let mut out: Vec<String> = Vec::new();
        if first && !word.contains('/') {
            for name in cmds::NAMES.iter().map(|(n, _)| *n) {
                if name.starts_with(word) {
                    out.push(name.to_string());
                }
            }
            for (a, _) in &self.aliases {
                if a.starts_with(word) {
                    out.push(a.clone());
                }
            }
            if let Some(fs) = ctx.fs.as_deref_mut() {
                for dir in PATH.split(':') {
                    for e in fs.list(dir).unwrap_or_default() {
                        if e.name.starts_with(word) && !e.is_dir && !out.contains(&e.name) {
                            out.push(e.name);
                        }
                    }
                }
            }
        } else {
            let (dir_typed, prefix) = match word.rsplit_once('/') {
                Some((d, p)) => (Some(if d.is_empty() { "/" } else { d }), p),
                None => (None, word),
            };
            let dir = self.abs(dir_typed.unwrap_or("."));
            if let Some(fs) = ctx.fs.as_deref_mut() {
                for e in fs.list(&dir).unwrap_or_default() {
                    if e.name.starts_with('.') && !prefix.starts_with('.') {
                        continue;
                    }
                    if e.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                        let mut full = match dir_typed {
                            Some("/") => format!("/{}", e.name),
                            Some(d) => format!("{d}/{}", e.name),
                            None => e.name.clone(),
                        };
                        if e.is_dir {
                            full.push('/');
                        }
                        out.push(full);
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub(crate) fn aliases(&self) -> &[(String, String)] {
        &self.aliases
    }

    pub(crate) fn aliases_mut(&mut self) -> &mut Vec<(String, String)> {
        &mut self.aliases
    }

    pub(crate) fn vars(&self) -> &[(String, String)] {
        &self.vars
    }

    pub(crate) fn prev_dir(&self) -> &str {
        &self.prev_dir
    }
}

/// Agrega `v` partido en palabras (lo que hace la shell con una variable sin comillas).
fn push_split(fields: &mut Vec<String>, v: &str, produced: &mut bool) {
    let mut words = v.split_whitespace();
    if let Some(first) = words.next() {
        *produced = true;
        if v.starts_with(char::is_whitespace) && !fields.last().unwrap().is_empty() {
            fields.push(String::new());
        }
        fields.last_mut().unwrap().push_str(first);
        for w in words {
            fields.push(w.to_string());
        }
    }
}

/// Une una carpeta y un nombre (para los comandos).
pub(crate) fn child(dir: &str, name: &str) -> String {
    join(dir, name)
}
