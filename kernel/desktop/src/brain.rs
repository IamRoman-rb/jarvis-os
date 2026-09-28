//! La conexión con el cerebro de JARVIS (`jarvis serve`, en el anfitrión; ADR 0008).
//!
//! Una conexión larga con un mensaje JSON por renglón. El primero es `hola` con el token de la
//! sesión (el kernel lo recibe del anfitrión por fw_cfg). Después, la consola manda `pedido`s y
//! la respuesta llega de a pedazos (`texto`), hasta `fin` o `error`. Si se corta, reconecta solo.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::config::parse_server;
use crate::system::{Outbox, StreamEvent};
use crate::web::json::{self, Json};

/// Dónde está el cerebro visto desde QEMU (el anfitrión es 10.0.2.2).
pub const DEFAULT_ADDR: &str = "10.0.2.2:8121";
/// La etiqueta de la conexión para el firewall.
pub const APP: &str = "jarvis";
const MAX_LINE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Sin token: el anfitrión no levantó el cerebro.
    Off,
    Connecting,
    Online,
    Offline,
}

/// Lo que la consola tiene que mostrar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrainEvent {
    Text(String),
    End,
    Error(String),
    /// El cerebro pide ejecutar una tool (ya aprobada): el escritorio contesta con
    /// [`BrainService::result`].
    Action {
        call: u32,
        tool: String,
        args: Vec<(String, String)>,
    },
    /// Pide confirmar una acción de nivel 2 o 3: el escritorio contesta con
    /// [`BrainService::confirmation`].
    Confirm {
        call: u32,
        level: u8,
        desc: String,
    },
    /// El avance del agente de un proyecto (para la ventana Proyecto).
    Project {
        name: String,
        ev: String,
        text: String,
    },
}

/// Lo que la consola le pide al cerebro (por el [`Outbox`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrainOp {
    Ask(String),
    Cancel,
    /// Detener el agente del proyecto (Esc en la ventana Proyecto).
    StopProject,
}

pub struct BrainService {
    token: String,
    addr: String,
    equipo: String,
    stream: Option<u32>,
    ready: bool,
    buf: Vec<u8>,
    next_try: u64,
    backoff: u64,
    next_id: u32,
    current: Option<u32>,
    /// La respuesta en curso (para la esfera y el mensaje del escritorio).
    pub answer: String,
    pub status: Status,
    pub logs: Vec<String>,
}

impl Default for BrainService {
    fn default() -> BrainService {
        BrainService {
            token: String::new(),
            addr: DEFAULT_ADDR.into(),
            equipo: String::new(),
            stream: None,
            ready: false,
            buf: Vec::new(),
            next_try: 0,
            backoff: 1000,
            next_id: 0,
            current: None,
            answer: String::new(),
            status: Status::Off,
            logs: Vec::new(),
        }
    }
}

/// Un texto como string de JSON.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(j: Option<&Json>) -> Option<u32> {
    match j? {
        Json::Num(n) => n.parse().ok(),
        _ => None,
    }
}

impl BrainService {
    /// El puerto del cerebro en el anfitrión, el token de la sesión y el nombre del equipo
    /// (el kernel los recibe por fw_cfg al arrancar).
    pub fn configure(&mut self, port: u16, token: &str, equipo: &str) {
        self.addr = format!("10.0.2.2:{port}");
        self.token = token.to_string();
        self.equipo = equipo.to_string();
        if !token.is_empty() && self.status == Status::Off {
            self.status = Status::Offline;
        }
    }

    pub fn online(&self) -> bool {
        self.ready
    }

    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    fn send(&self, out: &mut Outbox, line: String) {
        if let Some(id) = self.stream {
            let mut data = line.into_bytes();
            data.push(b'\n');
            out.send(id, data);
        }
    }

    /// Una vez por segundo: conectar si hace falta.
    pub fn tick(&mut self, now_ms: u64, out: &mut Outbox) {
        if self.token.is_empty() || self.stream.is_some() || now_ms < self.next_try {
            return;
        }
        let Some((host, port)) = parse_server(&self.addr) else {
            return;
        };
        self.stream = Some(out.connect_as(host, port, APP));
        self.status = Status::Connecting;
        self.next_try = now_ms + self.backoff;
        self.backoff = (self.backoff * 2).min(30_000);
    }

    /// Manda un pedido. `false` si el cerebro no está conectado.
    pub fn handle(&mut self, op: BrainOp, out: &mut Outbox) -> bool {
        if !self.ready {
            return false;
        }
        match op {
            BrainOp::Ask(text) => {
                self.next_id += 1;
                self.current = Some(self.next_id);
                self.answer.clear();
                self.send(
                    out,
                    format!(
                        "{{\"t\":\"pedido\",\"id\":{},\"texto\":{},\"origen\":\"consola\"}}",
                        self.next_id,
                        quote(&text)
                    ),
                );
            }
            BrainOp::Cancel => {
                if let Some(id) = self.current {
                    self.send(out, format!("{{\"t\":\"cancelar\",\"id\":{id}}}"));
                }
            }
            BrainOp::StopProject => self.send(out, "{\"t\":\"proyecto_detener\"}".into()),
        }
        true
    }

    /// Un evento de conexión: `None` si no era la del cerebro.
    pub fn stream_event(
        &mut self,
        id: u32,
        event: &StreamEvent,
        now_ms: u64,
        out: &mut Outbox,
    ) -> Option<Vec<BrainEvent>> {
        if self.stream != Some(id) {
            return None;
        }
        let mut events = Vec::new();
        match event {
            StreamEvent::Connected => {
                self.buf.clear();
                self.send(
                    out,
                    format!(
                        "{{\"t\":\"hola\",\"token\":{},\"equipo\":{}}}",
                        quote(&self.token),
                        quote(&self.equipo)
                    ),
                );
            }
            StreamEvent::Data(data) => {
                self.buf.extend_from_slice(data);
                while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = self.buf.drain(..=nl).collect();
                    if let Ok(s) = core::str::from_utf8(&line[..line.len() - 1])
                        && let Some(msg) = json::parse(s)
                    {
                        self.on_message(&msg, &mut events);
                    }
                }
                if self.buf.len() > MAX_LINE {
                    self.buf.clear();
                    out.close_stream(id);
                }
            }
            StreamEvent::Closed(why) => {
                self.stream = None;
                if self.ready {
                    self.logs.push(format!(
                        "CEREBRO_DESCONECTADO {}",
                        why.as_deref().unwrap_or("")
                    ));
                }
                self.ready = false;
                self.status = Status::Offline;
                self.next_try = self.next_try.max(now_ms + 1000);
                if self.current.take().is_some() {
                    events.push(BrainEvent::Error(
                        crate::i18n::tr("Se cortó la conexión con el cerebro.").into(),
                    ));
                }
            }
        }
        Some(events)
    }

    /// El resultado de una acción.
    pub fn result(&mut self, call: u32, ok: bool, datos: &str, out: &mut Outbox) {
        self.logs.push(format!(
            "CEREBRO_ACCION_FIN {call} {}",
            if ok { "ok" } else { "error" }
        ));
        self.send(
            out,
            format!(
                "{{\"t\":\"resultado\",\"llamada\":{call},\"ok\":{ok},\"datos\":{}}}",
                quote(datos)
            ),
        );
    }

    /// Lo que Roman decidió en el diálogo de confirmación.
    pub fn confirmation(&mut self, call: u32, ok: bool, out: &mut Outbox) {
        self.logs.push(format!(
            "CEREBRO_CONFIRMACION {call} {}",
            if ok { "si" } else { "no" }
        ));
        self.send(
            out,
            format!("{{\"t\":\"confirmacion\",\"llamada\":{call},\"ok\":{ok}}}"),
        );
    }

    fn on_message(&mut self, msg: &Json, events: &mut Vec<BrainEvent>) {
        let t = msg.get("t").and_then(Json::str).unwrap_or("");
        let for_current = num(msg.get("id")).is_some_and(|id| Some(id) == self.current);
        match t {
            "listo" => {
                self.ready = true;
                self.status = Status::Online;
                self.backoff = 1000;
                self.logs.push("CEREBRO_CONECTADO".into());
            }
            "texto" if for_current => {
                let delta = msg.get("delta").and_then(Json::str).unwrap_or("");
                self.answer.push_str(delta);
                events.push(BrainEvent::Text(delta.to_string()));
            }
            "fin" if for_current => {
                self.current = None;
                self.logs.push(format!(
                    "CEREBRO_RESPUESTA {}",
                    self.answer.replace('\n', " ")
                ));
                events.push(BrainEvent::End);
            }
            "accion" => {
                let (Some(call), Some(tool)) =
                    (num(msg.get("llamada")), msg.get("tool").and_then(Json::str))
                else {
                    return;
                };
                let args = match msg.get("args") {
                    Some(Json::Obj(kv)) => kv
                        .iter()
                        .map(|(k, v)| {
                            let v = match v {
                                Json::Str(s) | Json::Num(s) => s.clone(),
                                Json::Bool(b) => b.to_string(),
                                _ => String::new(),
                            };
                            (k.clone(), v)
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                self.logs.push(format!("CEREBRO_ACCION {tool}"));
                events.push(BrainEvent::Action {
                    call,
                    tool: tool.to_string(),
                    args,
                });
            }
            "proyecto" => {
                let s = |k: &str| msg.get(k).and_then(Json::str).unwrap_or("").to_string();
                let (name, ev, text) = (s("nombre"), s("ev"), s("texto"));
                if ev != "texto" {
                    self.logs
                        .push(format!("PROYECTO_{} {name}", ev.to_uppercase()));
                }
                events.push(BrainEvent::Project { name, ev, text });
            }
            "confirmar" => {
                let Some(call) = num(msg.get("llamada")) else {
                    return;
                };
                let level = num(msg.get("nivel")).unwrap_or(3).clamp(2, 3) as u8;
                let desc = msg.get("descripcion").and_then(Json::str).unwrap_or("?");
                self.logs.push(format!("CEREBRO_CONFIRMAR {level} {desc}"));
                events.push(BrainEvent::Confirm {
                    call,
                    level,
                    desc: desc.to_string(),
                });
            }
            "error" if for_current => {
                self.current = None;
                let m = msg.get("msg").and_then(Json::str).unwrap_or("error");
                self.logs.push(format!("CEREBRO_ERROR {m}"));
                events.push(BrainEvent::Error(m.to_string()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comillas_de_json() {
        assert_eq!(quote("a \"b\"\n\\c"), "\"a \\\"b\\\"\\n\\\\c\"");
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
        let parsed = json::parse(&quote("línea \"1\"\nlínea 2")).unwrap();
        assert_eq!(parsed.str(), Some("línea \"1\"\nlínea 2"));
    }
}
