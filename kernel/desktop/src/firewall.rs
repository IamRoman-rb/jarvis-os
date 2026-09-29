//! Firewall: qué conexiones se permiten y cuáles no.
//!
//! En JARVIS-OS las apps no abren conexiones por su cuenta: le piden al kernel que baje una
//! dirección (el `Outbox`). Por ahí pasa **todo** lo que sale a la red, así que ahí se controla:
//! antes de que el pedido llegue al kernel, se compara con las reglas y, si una lo prohíbe, no
//! sale ni un paquete (ni siquiera la consulta DNS). La app recibe un error que dice qué regla
//! lo bloqueó.
//!
//! Lo que **entra**: el kernel no escucha en ningún puerto (no ofrece servicios), así que la
//! pila TCP/IP rechaza cualquier conexión que no haya abierto JARVIS-OS. La política de entrada
//! queda en "denegar" y las reglas de entrada se guardan (para cuando haya servicios).
//!
//! Las reglas se evalúan en orden y gana la primera que coincide, como en `ufw` de Ubuntu (el
//! comando `ufw` de la terminal usa la misma sintaxis). Cada regla puede pedir un sitio
//! (`ejemplo.com` vale también para sus subdominios; `*.ejemplo.com` solo para los
//! subdominios; o una IP), un puerto y una app (`navegador`, `terminal`, `apt`, `snap`…).
//!
//! Se guarda en `/Sistema/config.ini`, con el resto de la configuración:
//!
//! ```text
//! firewall=si
//! firewall_saliente=permitir
//! firewall_regla=denegar salida a facebook.com puerto 443
//! ```

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Las apps que se pueden nombrar en una regla.
pub const APPS: [(&str, &str); 11] = [
    ("navegador", "Navegador web"),
    ("brave", "Brave"),
    ("sync", "Sincronización"),
    ("terminal", "Terminal (wget, curl)"),
    ("apt", "Paquetes de JARVIS-OS (apt, dpkg)"),
    ("snap", "Tienda de snaps"),
    ("winget", "Programas de Windows (winget)"),
    ("configuracion", "Configuración (prueba de red)"),
    ("jarvis", "Consola de JARVIS"),
    ("programas", "Programas de Linux (sockets)"),
    ("sistema", "El resto del sistema"),
];

pub const LOG_PATH: &str = "/Sistema/firewall.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

impl Action {
    fn word(self) -> &'static str {
        match self {
            Action::Allow => "permitir",
            Action::Deny => "denegar",
        }
    }

    fn ufw(self) -> &'static str {
        match self {
            Action::Allow => "ALLOW",
            Action::Deny => "DENY",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub action: Action,
    pub dir: Dir,
    /// Sitio o IP (`None` = cualquiera).
    pub host: Option<String>,
    pub port: Option<u16>,
    pub app: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firewall {
    pub enabled: bool,
    pub default_in: Action,
    pub default_out: Action,
    /// Anotar lo que se bloquea en [`LOG_PATH`].
    pub log: bool,
    pub rules: Vec<Rule>,
}

impl Default for Firewall {
    fn default() -> Self {
        Firewall {
            enabled: true,
            default_in: Action::Deny,
            default_out: Action::Allow,
            log: true,
            rules: Vec::new(),
        }
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    let (p, h) = (pattern.to_ascii_lowercase(), host.to_ascii_lowercase());
    if let Some(suffix) = p.strip_prefix("*.") {
        return h.ends_with(&format!(".{suffix}"));
    }
    if let Some((net, bits)) = p.split_once('/') {
        // Una red: 10.0.0.0/8
        let (Some(n), Ok(bits), Some(ip)) = (parse_ip(net), bits.parse::<u32>(), parse_ip(&h))
        else {
            return false;
        };
        let mask = if bits == 0 {
            0
        } else {
            u32::MAX << (32 - bits.min(32))
        };
        return (n & mask) == (ip & mask);
    }
    h == p || h.ends_with(&format!(".{p}"))
}

fn parse_ip(s: &str) -> Option<u32> {
    let parts: Vec<u8> = s
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() == 4).then(|| u32::from_be_bytes([parts[0], parts[1], parts[2], parts[3]]))
}

impl Rule {
    fn matches(&self, dir: Dir, host: &str, port: u16, app: &str) -> bool {
        self.dir == dir
            && self.host.as_deref().is_none_or(|p| host_matches(p, host))
            && self.port.is_none_or(|p| p == port)
            && self
                .app
                .as_deref()
                .is_none_or(|a| a.eq_ignore_ascii_case(app))
    }

    /// Como se guarda: `denegar salida a facebook.com puerto 443 app navegador`.
    pub fn to_line(&self) -> String {
        let mut s = format!(
            "{} {}",
            self.action.word(),
            if self.dir == Dir::Out {
                "salida"
            } else {
                "entrada"
            }
        );
        if let Some(h) = &self.host {
            s.push_str(&format!(" a {h}"));
        }
        if let Some(p) = self.port {
            s.push_str(&format!(" puerto {p}"));
        }
        if let Some(a) = &self.app {
            s.push_str(&format!(" app {a}"));
        }
        s
    }

    /// Lee una regla en castellano (la del archivo) o en la sintaxis de `ufw`
    /// (`deny out to facebook.com port 443`).
    pub fn parse(s: &str) -> Result<Rule, String> {
        let words: Vec<String> = s
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        let mut it = words.iter().map(String::as_str).peekable();
        let action = match it.next() {
            Some("permitir" | "allow" | "limit") => Action::Allow,
            Some("denegar" | "deny" | "reject" | "bloquear" | "rechazar") => Action::Deny,
            other => {
                return Err(format!(
                    "acción desconocida: {} (allow, deny o reject)",
                    other.unwrap_or("")
                ));
            }
        };
        let mut rule = Rule {
            action,
            dir: Dir::In,
            host: None,
            port: None,
            app: None,
        };
        let mut explicit_dir = false;
        while let Some(w) = it.next() {
            match w {
                "out" | "salida" | "saliente" => {
                    rule.dir = Dir::Out;
                    explicit_dir = true;
                }
                "in" | "entrada" | "entrante" => {
                    rule.dir = Dir::In;
                    explicit_dir = true;
                }
                "to" | "a" | "hacia" | "from" | "desde" => {
                    let target = it.next().ok_or("falta el sitio después de 'to'")?;
                    if target != "any" && target != "cualquiera" {
                        rule.host = Some(target.to_string());
                    }
                    if !explicit_dir && matches!(w, "to" | "a" | "hacia") && rule.dir == Dir::In {
                        // `deny to x.com` sin decir la dirección: es algo que sale.
                        rule.dir = Dir::Out;
                    }
                }
                "port" | "puerto" => {
                    let p = it.next().ok_or("falta el número de puerto")?;
                    rule.port = Some(p.parse().map_err(|_| format!("puerto inválido: {p}"))?);
                }
                "app" | "aplicacion" | "aplicación" => {
                    let a = it.next().ok_or("falta el nombre de la app")?;
                    if !APPS.iter().any(|(n, _)| *n == a) {
                        return Err(format!("app desconocida: {a} (ver: ufw app list)"));
                    }
                    rule.app = Some(a.to_string());
                    if !explicit_dir {
                        rule.dir = Dir::Out;
                    }
                }
                "proto" => {
                    it.next(); // solo hay TCP
                }
                other => {
                    // `allow 22`, `deny 443/tcp`, `deny out 80`
                    let num = other.split('/').next().unwrap_or(other);
                    match num.parse::<u16>() {
                        Ok(p) => rule.port = Some(p),
                        Err(_) if other.contains('.') => {
                            rule.host = Some(other.to_string());
                            if !explicit_dir {
                                rule.dir = Dir::Out;
                            }
                        }
                        Err(_) => return Err(format!("no entiendo '{other}'")),
                    }
                }
            }
        }
        if rule.host.is_none() && rule.port.is_none() && rule.app.is_none() {
            return Err("una regla necesita un sitio, un puerto o una app".into());
        }
        Ok(rule)
    }

    /// Las tres columnas de `ufw status`: (hacia, acción, desde).
    fn columns(&self) -> (String, String, String) {
        let mut to = self.host.clone().unwrap_or_else(|| "Cualquiera".into());
        if let Some(p) = self.port {
            to = if self.host.is_some() {
                format!("{to} {p}")
            } else {
                format!("{p}/tcp")
            };
        }
        let from = match &self.app {
            Some(a) => format!("app {a}"),
            None => "Cualquiera".into(),
        };
        let action = format!(
            "{} {}",
            self.action.ufw(),
            if self.dir == Dir::Out { "OUT" } else { "IN" }
        );
        (to, action, from)
    }
}

impl Firewall {
    /// ¿Puede `app` conectarse a `host:port`? `Err` dice por qué no.
    pub fn check_out(&self, host: &str, port: u16, app: &str) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        for (i, r) in self.rules.iter().enumerate() {
            if r.matches(Dir::Out, host, port, app) {
                return match r.action {
                    Action::Allow => Ok(()),
                    Action::Deny => Err(format!("regla {}: {}", i + 1, r.to_line())),
                };
            }
        }
        match self.default_out {
            Action::Allow => Ok(()),
            Action::Deny => Err("política por defecto: denegar lo saliente".into()),
        }
    }

    /// Lee las claves `firewall*` de la configuración. Devuelve `false` si la clave no es suya.
    pub fn parse_key(&mut self, k: &str, v: &str) -> bool {
        let yes = matches!(v, "si" | "sí" | "1" | "true" | "on");
        let action = |v: &str| {
            if v.starts_with("den") || v == "deny" {
                Action::Deny
            } else {
                Action::Allow
            }
        };
        match k {
            "firewall" => self.enabled = yes,
            "firewall_entrante" => self.default_in = action(v),
            "firewall_saliente" => self.default_out = action(v),
            "firewall_registro" => self.log = yes,
            "firewall_regla" => {
                if let Ok(r) = Rule::parse(v)
                    && self.rules.len() < 200
                {
                    self.rules.push(r);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn serialize(&self) -> Vec<String> {
        let yn = |b: bool| if b { "si" } else { "no" };
        let mut out = alloc::vec![
            format!("firewall={}", yn(self.enabled)),
            format!("firewall_entrante={}", self.default_in.word()),
            format!("firewall_saliente={}", self.default_out.word()),
            format!("firewall_registro={}", yn(self.log)),
        ];
        for r in &self.rules {
            out.push(format!("firewall_regla={}", r.to_line()));
        }
        out
    }

    /// `ufw status` (con `numbered`, las reglas numeradas; con `verbose`, más detalle).
    pub fn status(&self, numbered: bool, verbose: bool) -> String {
        let mut s = format!(
            "Estado: {}\n",
            if self.enabled { "activo" } else { "inactivo" }
        );
        if !self.enabled {
            return s;
        }
        if verbose {
            s.push_str(&format!(
                "Registro: {}\nPor defecto: {} (entrante), {} (saliente)\n\
                 Servicios escuchando: ninguno (lo entrante que JARVIS-OS no pidió se rechaza)\n",
                if self.log { "activado" } else { "desactivado" },
                self.default_in.word(),
                self.default_out.word()
            ));
        }
        if self.rules.is_empty() {
            return s;
        }
        let rows: Vec<(String, String, String)> = self.rules.iter().map(Rule::columns).collect();
        let w0 = rows
            .iter()
            .map(|r| r.0.chars().count())
            .max()
            .unwrap_or(5)
            .max(5)
            + 2;
        let w1 = rows
            .iter()
            .map(|r| r.1.chars().count())
            .max()
            .unwrap_or(6)
            .max(6)
            + 2;
        let pad = |t: &str, w: usize| {
            let mut x = t.to_string();
            for _ in t.chars().count()..w {
                x.push(' ');
            }
            x
        };
        let prefix = if numbered { "     " } else { "" };
        s.push('\n');
        s.push_str(&format!(
            "{prefix}{}{}Desde\n{prefix}{}{}-----\n",
            pad("Hacia", w0),
            pad("Acción", w1),
            pad("-----", w0),
            pad("------", w1)
        ));
        for (i, (to, act, from)) in rows.iter().enumerate() {
            let n = if numbered {
                pad(&format!("[{:>2}]", i + 1), 5)
            } else {
                String::new()
            };
            s.push_str(&format!("{n}{}{}{from}\n", pad(to, w0), pad(act, w1)));
        }
        s
    }

    /// Ejecuta `ufw <args>`. Devuelve (salida, ¿cambió algo?). `Err` = error de uso.
    pub fn ufw(&mut self, args: &[&str]) -> Result<(String, bool), String> {
        let lower: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
        let a: Vec<&str> = lower.iter().map(String::as_str).collect();
        match a.as_slice() {
            [] | ["help"] | ["--help"] => Ok((HELP.into(), false)),
            ["version"] | ["--version"] => Ok(("ufw 0.36 (JARVIS-OS)\n".into(), false)),
            ["status"] => Ok((self.status(false, false), false)),
            ["status", "numbered"] => Ok((self.status(true, false), false)),
            ["status", "verbose"] => Ok((self.status(false, true), false)),
            ["enable"] => {
                self.enabled = true;
                Ok((
                    "El firewall está activo y se habilitará al arrancar el sistema\n".into(),
                    true,
                ))
            }
            ["disable"] => {
                self.enabled = false;
                Ok((
                    "El firewall está detenido y deshabilitado al arrancar el sistema\n".into(),
                    true,
                ))
            }
            ["reload"] => Ok(("Firewall recargado\n".into(), false)),
            ["reset"] | ["reset", "--force"] => {
                *self = Firewall::default();
                Ok((
                    "Reglas borradas; se volvió a la configuración inicial\n".into(),
                    true,
                ))
            }
            ["logging", v] => {
                self.log = *v != "off";
                Ok((
                    format!(
                        "Registro {}\n",
                        if self.log { "activado" } else { "desactivado" }
                    ),
                    true,
                ))
            }
            ["default", act, rest @ ..] => {
                let action = match *act {
                    "allow" => Action::Allow,
                    "deny" | "reject" => Action::Deny,
                    _ => return Err("uso: ufw default allow|deny [incoming|outgoing]".into()),
                };
                let outgoing = rest.first().is_some_and(|d| d.starts_with("out"));
                if outgoing {
                    self.default_out = action;
                } else {
                    self.default_in = action;
                }
                Ok((
                    format!(
                        "Política por defecto para lo {} cambiada a '{}'\n",
                        if outgoing { "saliente" } else { "entrante" },
                        action.word()
                    ),
                    true,
                ))
            }
            ["delete", n] if n.parse::<usize>().is_ok() => {
                let i: usize = n.parse().unwrap_or(0);
                if i == 0 || i > self.rules.len() {
                    return Err(format!("no existe la regla {i} (ver: ufw status numbered)"));
                }
                let r = self.rules.remove(i - 1);
                Ok((format!("Regla borrada: {}\n", r.to_line()), true))
            }
            ["delete", rest @ ..] => {
                let r = Rule::parse(&rest.join(" "))?;
                match self.rules.iter().position(|x| *x == r) {
                    Some(i) => {
                        self.rules.remove(i);
                        Ok(("Regla borrada\n".into(), true))
                    }
                    None => Err("no hay una regla así".into()),
                }
            }
            ["insert", n, rest @ ..] => {
                let i: usize = n.parse().map_err(|_| "uso: ufw insert N <regla>")?;
                let r = Rule::parse(&rest.join(" "))?;
                let at = i.saturating_sub(1).min(self.rules.len());
                self.rules.insert(at, r);
                Ok(("Regla insertada\n".into(), true))
            }
            ["app", "list"] => {
                let mut s = String::from("Apps disponibles:\n");
                for (n, d) in APPS {
                    s.push_str(&format!("  {n:<15}{d}\n"));
                }
                Ok((s, false))
            }
            ["show", "listening"] => Ok((
                "JARVIS-OS no escucha en ningún puerto: no hay servicios.\n".into(),
                false,
            )),
            ["show", "added"] => {
                let mut s = String::from("Reglas agregadas:\n");
                for r in &self.rules {
                    let (to, act, from) = r.columns();
                    s.push_str(&format!("ufw {} {to} ({from})\n", act.to_ascii_lowercase()));
                }
                Ok((s, false))
            }
            [act, ..] if matches!(*act, "allow" | "deny" | "reject" | "limit") => {
                let r = Rule::parse(&a.join(" "))?;
                if self.rules.contains(&r) {
                    return Ok(("Se omitió: la regla ya existe\n".into(), false));
                }
                self.rules.push(r);
                Ok(("Regla agregada\n".into(), true))
            }
            _ => Err(format!("comando inválido: ufw {}", args.join(" "))),
        }
    }
}

const HELP: &str = "Uso: ufw COMANDO

  enable | disable            prende o apaga el firewall
  status [numbered|verbose]   muestra el estado y las reglas
  default allow|deny outgoing política para lo que sale
  allow|deny [out] [to SITIO] [port N] [app APP]
                              agrega una regla (la primera que coincide gana)
  insert N <regla>            agrega una regla en el lugar N
  delete N | delete <regla>   borra una regla
  reset                       borra todas las reglas
  logging on|off              anota lo bloqueado en /Sistema/firewall.log
  app list                    apps que se pueden nombrar en las reglas
  show added|listening        reglas agregadas / servicios escuchando

Ejemplos:
  ufw deny out to facebook.com
  ufw deny out to *.doubleclick.net
  ufw deny out port 80 app navegador
  ufw default deny outgoing && ufw allow out to wikipedia.org
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reglas_en_orden_y_sitios() {
        let mut fw = Firewall::default();
        assert!(fw.check_out("facebook.com", 443, "navegador").is_ok());
        fw.ufw(&["deny", "out", "to", "facebook.com"]).unwrap();
        fw.ufw(&["deny", "out", "port", "80", "app", "navegador"])
            .unwrap();
        fw.ufw(&["deny", "to", "*.ads.net"]).unwrap();
        let e = fw
            .check_out("www.facebook.com", 443, "terminal")
            .unwrap_err();
        assert!(e.contains("regla 1"), "{e}");
        assert!(fw.check_out("example.com", 80, "navegador").is_err());
        assert!(fw.check_out("example.com", 80, "terminal").is_ok());
        assert!(fw.check_out("x.ads.net", 443, "apt").is_err());
        assert!(
            fw.check_out("ads.net", 443, "apt").is_ok(),
            "*. es solo subdominios"
        );
        // Una regla de permitir antes gana.
        fw.ufw(&["insert", "1", "allow", "out", "to", "m.facebook.com"])
            .unwrap();
        assert!(fw.check_out("m.facebook.com", 443, "navegador").is_ok());
        // Política por defecto.
        fw.ufw(&["default", "deny", "outgoing"]).unwrap();
        assert!(fw.check_out("wikipedia.org", 443, "navegador").is_err());
        fw.ufw(&["disable"]).unwrap();
        assert!(fw.check_out("wikipedia.org", 443, "navegador").is_ok());
        // Redes.
        let mut fw = Firewall::default();
        fw.ufw(&["deny", "out", "to", "10.0.0.0/8"]).unwrap();
        assert!(fw.check_out("10.1.2.3", 80, "terminal").is_err());
        assert!(fw.check_out("11.1.2.3", 80, "terminal").is_ok());
    }

    #[test]
    fn estado_borrado_y_guardado() {
        let mut fw = Firewall::default();
        fw.ufw(&["allow", "22"]).unwrap();
        fw.ufw(&["deny", "out", "to", "tiktok.com", "port", "443"])
            .unwrap();
        let st = fw.status(true, false);
        assert!(st.contains("Estado: activo"));
        assert!(st.contains("[ 2] tiktok.com 443"), "{st}");
        assert!(st.contains("DENY OUT"));
        assert!(fw.ufw(&["delete", "9"]).is_err());
        assert!(fw.ufw(&["bailar"]).is_err());
        assert!(fw.ufw(&["deny", "app", "tostadora"]).is_err());
        // Ida y vuelta por la configuración.
        let mut back = Firewall::default();
        for line in fw.serialize() {
            let (k, v) = line.split_once('=').unwrap();
            assert!(back.parse_key(k, v));
        }
        assert_eq!(back, fw);
        fw.ufw(&["delete", "1"]).unwrap();
        assert_eq!(fw.rules.len(), 1);
    }
}
