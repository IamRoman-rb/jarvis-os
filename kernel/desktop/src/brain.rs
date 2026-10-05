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
    /// Roman le habló a JARVIS (el micrófono del anfitrión): se trata como si lo hubiera escrito.
    Heard(String),
    /// Empezó (o terminó) a escuchar un pedido.
    Listening(bool),
    /// Nivel del audio de la respuesta que está sonando (0..=100): mueve la esfera.
    VoiceLevel(u8),
    /// La voz para los parlantes de JARVIS-OS (K12): PCM mono de 16 bits a `rate` Hz. `end`: no
    /// llega más.
    Audio {
        rate: u32,
        pcm: Vec<i16>,
        end: bool,
    },
    /// Que se calle ya.
    Hush,
    /// El avance del agente de un proyecto (para la ventana Proyecto).
    Project {
        name: String,
        ev: String,
        text: String,
    },
    /// Un gesto de la mano frente a la cámara del anfitrión (Configuración → Gestos).
    Gesture(Gesture),
    /// El cerebro compiló los cambios que JARVIS le hizo al sistema: hay que apagar para que
    /// `cargo xtask run` arranque la versión nueva.
    Restart,
    /// Cambió la cuenta de Claude del anfitrión, un agente vinculado o el estado de un inicio
    /// de sesión (Configuración → Asistente se redibuja).
    Account,
}

/// Lo que hizo la mano frente a la cámara (lo reconoce el anfitrión: src/jarvis/gestures/).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// El índice señala: el puntero va ahí (0..=1000 de la pantalla, en cada eje).
    Move { x: u16, y: u16 },
    /// Pulgar e índice se juntaron.
    Click,
    /// Dos dedos arriba y la mano se movió: pasos de la rueda (positivo = hacia abajo).
    Scroll(i32),
    /// La palma abierta pasó rápido hacia un costado (`true` = a la derecha).
    Swipe(bool),
    /// La palma abierta quieta: el menú de inicio.
    Start,
}

/// Los agentes que pueden ser el cerebro de JARVIS: (código del protocolo, nombre). Claude es
/// el de siempre (la cuenta de Claude Code del anfitrión); los demás se vinculan desde
/// Configuración → Asistente, sin claves de API: Gemini y ChatGPT con la cuenta de Google y
/// DeepSeek en la PC (Ollama).
pub const AGENTS: [(&str, &str); 4] = [
    ("claude", "Claude"),
    ("gemini", "Gemini"),
    ("chatgpt", "ChatGPT"),
    ("deepseek", "DeepSeek"),
];

/// Un agente que se vincula (Gemini, ChatGPT, DeepSeek), como lo cuenta el cerebro.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiAgent {
    /// El código del protocolo ("gemini", "chatgpt", "deepseek").
    pub id: String,
    pub name: String,
    pub linked: bool,
    /// Cómo se vincula: "google" (la cuenta) o "local" (en la PC).
    pub method: String,
    /// Con qué cuenta entró, o el modelo que corre en la PC.
    pub detail: String,
    /// El inicio de sesión en curso o cómo terminó.
    pub state: String,
}

/// La cuenta con la que el cerebro usa Claude (la de Claude Code en el anfitrión).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// `None` = todavía no se sabe (el cerebro no lo dijo).
    pub logged_in: Option<bool>,
    pub email: String,
    /// "pro", "max", "api"...
    pub plan: String,
    /// El inicio de sesión en curso o cómo terminó (vacío = no hay uno).
    pub login: String,
}

/// Lo que la consola le pide al cerebro (por el [`Outbox`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrainOp {
    /// Un pedido; `true` si vino de la voz (la respuesta se dice en voz alta).
    Ask(String, bool),
    /// Escuchar un pedido sin la palabra de activación (Win+J).
    Listen,
    /// Decir un texto en voz alta (la respuesta local a una orden por voz).
    Speak(String),
    Cancel,
    /// Detener el agente del proyecto (Esc en la ventana Proyecto).
    StopProject,
    /// Iniciar sesión en Claude con Google: el anfitrión abre su navegador (Configuración →
    /// Asistente).
    Login,
    /// Volver a preguntar la cuenta.
    AccountStatus,
    /// Vincular un agente (por su código): con Google el anfitrión abre su navegador; DeepSeek
    /// se prepara en la PC (Ollama).
    AgentLink(String),
    AgentUnlink(String),
    /// Volver a preguntar el estado de los agentes.
    AgentsStatus,
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
    /// El cerebro tiene voz (lo dice en `listo`).
    pub voice: bool,
    /// La frecuencia de los parlantes de JARVIS-OS (0: no hay): se la dice al cerebro, que
    /// entonces manda la voz como audio (K12).
    pub speakers: u32,
    pub account: Account,
    /// Gemini, ChatGPT y DeepSeek (en el orden de [`AGENTS`], sin Claude).
    pub agents: Vec<AiAgent>,
    /// El agente principal (índice de [`AGENTS`]) y si opina el consejo de los demás: los
    /// elige Roman en Configuración y viajan en `hola` y en `agentes_modo`.
    pub lead: usize,
    pub council: bool,
    /// El saludo del arranque que falta pedirle al cerebro: (momento, nombre). Se manda en
    /// cuanto se conecta (con el id 0, que no usan los pedidos).
    pub greeting: Option<(&'static str, String)>,
    /// Gestos con la cámara activados (Configuración): viaja en `hola` y en `gestos`.
    pub gestures: bool,
    /// Qué dice el anfitrión de la cámara ("" = nada todavía).
    pub camera: String,
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
            voice: false,
            speakers: 0,
            account: Account::default(),
            agents: AGENTS[1..]
                .iter()
                .map(|(id, name)| AiAgent {
                    id: (*id).into(),
                    name: (*name).into(),
                    ..AiAgent::default()
                })
                .collect(),
            lead: 0,
            council: false,
            greeting: None,
            gestures: false,
            camera: String::new(),
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

    /// Se prendieron o apagaron los gestos: el anfitrión abre o cierra la cámara.
    pub fn set_gestures(&mut self, on: bool, out: &mut Outbox) {
        if on == self.gestures {
            return;
        }
        self.gestures = on;
        self.logs.push(format!("CEREBRO_GESTOS {on}"));
        if self.ready {
            self.send(out, format!("{{\"t\":\"gestos\",\"activo\":{on}}}"));
        }
    }

    /// Cambió el agente principal o el consejo (Configuración): se le avisa al cerebro.
    pub fn set_mode(&mut self, lead: usize, council: bool, out: &mut Outbox) {
        let lead = lead.min(AGENTS.len() - 1);
        if (lead, council) == (self.lead, self.council) {
            return;
        }
        (self.lead, self.council) = (lead, council);
        self.logs
            .push(format!("CEREBRO_MODO {} {}", AGENTS[lead].0, council));
        if self.ready {
            self.send(
                out,
                format!(
                    "{{\"t\":\"agentes_modo\",\"principal\":\"{}\",\"consejo\":{council}}}",
                    AGENTS[lead].0
                ),
            );
        }
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
            BrainOp::Listen => self.send(out, "{\"t\":\"escuchar\"}".into()),
            BrainOp::Speak(text) => self.send(
                out,
                format!("{{\"t\":\"decir\",\"texto\":{}}}", quote(&text)),
            ),
            BrainOp::Ask(text, by_voice) => {
                self.next_id += 1;
                self.current = Some(self.next_id);
                self.answer.clear();
                self.send(
                    out,
                    format!(
                        "{{\"t\":\"pedido\",\"id\":{},\"texto\":{},\"origen\":\"{}\"}}",
                        self.next_id,
                        quote(&text),
                        if by_voice { "voz" } else { "consola" }
                    ),
                );
            }
            BrainOp::Cancel => {
                if let Some(id) = self.current {
                    self.send(out, format!("{{\"t\":\"cancelar\",\"id\":{id}}}"));
                }
            }
            BrainOp::StopProject => self.send(out, "{\"t\":\"proyecto_detener\"}".into()),
            BrainOp::Login => {
                self.account.login = crate::i18n::tr("Abriendo el navegador de la PC...").into();
                self.logs.push("CEREBRO_LOGIN".into());
                self.send(
                    out,
                    "{\"t\":\"iniciar_sesion\",\"metodo\":\"google\"}".into(),
                );
            }
            BrainOp::AccountStatus => self.send(out, "{\"t\":\"cuenta\"}".into()),
            BrainOp::AgentLink(id) => {
                self.set_agent_state(&id, crate::i18n::tr("Vinculando en el anfitrión..."));
                self.logs.push(format!("CEREBRO_VINCULAR {id}"));
                self.send(
                    out,
                    format!("{{\"t\":\"vincular\",\"agente\":{}}}", quote(&id)),
                );
            }
            BrainOp::AgentUnlink(id) => {
                self.logs.push(format!("CEREBRO_DESVINCULAR {id}"));
                self.send(
                    out,
                    format!("{{\"t\":\"desvincular\",\"agente\":{}}}", quote(&id)),
                );
            }
            BrainOp::AgentsStatus => self.send(out, "{\"t\":\"agentes\"}".into()),
        }
        true
    }

    fn set_agent_state(&mut self, id: &str, state: &str) {
        if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
            a.state = state.into();
        }
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
                        "{{\"t\":\"hola\",\"token\":{},\"equipo\":{},\"parlantes\":{},\"principal\":\"{}\",\"consejo\":{},\"gestos\":{}}}",
                        quote(&self.token),
                        quote(&self.equipo),
                        self.speakers,
                        AGENTS[self.lead].0,
                        self.council,
                        self.gestures
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
                if self.ready
                    && let Some((moment, name)) = self.greeting.take()
                {
                    // El saludo del arranque (ver [`Self::greeting`]): llega como una respuesta.
                    self.current = Some(0);
                    self.answer.clear();
                    self.logs.push(format!("CEREBRO_SALUDO {moment}"));
                    self.send(
                        out,
                        format!(
                            "{{\"t\":\"saludo\",\"id\":0,\"momento\":\"{moment}\",\"nombre\":{}}}",
                            quote(&name)
                        ),
                    );
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
                self.voice = matches!(msg.get("voz"), Some(Json::Bool(true)));
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
            "oido" => {
                let text = msg.get("texto").and_then(Json::str).unwrap_or("").trim();
                if !text.is_empty() {
                    self.logs.push(format!("VOZ_OIDO {text}"));
                    events.push(BrainEvent::Heard(text.to_string()));
                }
            }
            "escuchando" => {
                let on = matches!(msg.get("activo"), Some(Json::Bool(true)));
                self.logs
                    .push(format!("VOZ_ESCUCHANDO {}", if on { "si" } else { "no" }));
                events.push(BrainEvent::Listening(on));
            }
            "voz" => {
                let n = num(msg.get("nivel")).unwrap_or(0).min(100) as u8;
                events.push(BrainEvent::VoiceLevel(n));
            }
            "audio" => {
                let end = matches!(msg.get("fin"), Some(Json::Bool(true)));
                let rate = num(msg.get("tasa")).unwrap_or(22_050).clamp(8000, 48_000);
                let bytes = msg
                    .get("pcm")
                    .and_then(Json::str)
                    .and_then(base64)
                    .unwrap_or_default();
                let pcm = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|s| i16::from_le_bytes(*s))
                    .collect();
                events.push(BrainEvent::Audio { rate, pcm, end });
            }
            "callar" => events.push(BrainEvent::Hush),
            "camara" => {
                self.camera = msg
                    .get("estado")
                    .and_then(Json::str)
                    .unwrap_or("")
                    .to_string();
                self.logs.push(format!("CEREBRO_CAMARA {}", self.camera));
                events.push(BrainEvent::Account);
            }
            "gesto" => {
                let n = |k: &str| num(msg.get(k)).unwrap_or(0);
                let g = match msg.get("tipo").and_then(Json::str).unwrap_or("") {
                    "mover" => Gesture::Move {
                        x: n("x").min(1000) as u16,
                        y: n("y").min(1000) as u16,
                    },
                    "clic" => Gesture::Click,
                    "desplazar" => {
                        // Los números del parser de JSON son enteros sin signo: el sentido va aparte.
                        let d = n("pasos").min(20) as i32;
                        Gesture::Scroll(if matches!(msg.get("arriba"), Some(Json::Bool(true))) {
                            -d
                        } else {
                            d
                        })
                    }
                    "deslizar" => {
                        Gesture::Swipe(msg.get("dir").and_then(Json::str) == Some("derecha"))
                    }
                    "inicio" => Gesture::Start,
                    _ => return,
                };
                events.push(BrainEvent::Gesture(g));
            }
            "reiniciar" => {
                self.logs.push("CEREBRO_REINICIAR".into());
                events.push(BrainEvent::Restart);
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
            "cuenta" => {
                let s = |k: &str| msg.get(k).and_then(Json::str).unwrap_or("").to_string();
                self.account = Account {
                    logged_in: match msg.get("sesion") {
                        Some(Json::Bool(b)) => Some(*b),
                        _ => None,
                    },
                    email: s("email"),
                    plan: s("plan"),
                    login: s("estado"),
                };
                self.logs.push(format!(
                    "CEREBRO_CUENTA {} {}",
                    match self.account.logged_in {
                        Some(true) => "si",
                        Some(false) => "no",
                        None => "?",
                    },
                    self.account.login
                ));
                events.push(BrainEvent::Account);
            }
            "agente" => {
                let s = |k: &str| msg.get(k).and_then(Json::str).unwrap_or("").to_string();
                let id = s("id");
                let Some(a) = self.agents.iter_mut().find(|a| a.id == id) else {
                    return;
                };
                a.linked = matches!(msg.get("vinculado"), Some(Json::Bool(true)));
                a.method = s("metodo");
                a.detail = s("detalle");
                a.state = s("estado");
                self.logs.push(format!(
                    "CEREBRO_AGENTE {id} {} {}",
                    if a.linked { "si" } else { "no" },
                    a.state
                ));
                events.push(BrainEvent::Account);
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

/// Decodifica base64 (el estándar, con `=` al final). `None` si no es base64 válido.
fn base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let t = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(t.len() * 3 / 4);
    for group in t.chunks(4) {
        if group.len() == 1 {
            return None;
        }
        let mut acc = 0u32;
        for (i, &c) in group.iter().enumerate() {
            acc |= value(c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..group.len()]);
    }
    Some(out)
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

    #[test]
    fn la_voz_llega_como_audio_en_base64() {
        assert_eq!(base64("AQACAA==").unwrap(), [1, 0, 2, 0]);
        assert_eq!(base64("aG9sYQ").unwrap(), b"hola");
        assert_eq!(base64(""), Some(Vec::new()));
        assert!(base64("a").is_none());
        assert!(base64("??").is_none());
        let mut b = BrainService::default();
        let mut events = Vec::new();
        let msg = json::parse(r#"{"t":"audio","tasa":22050,"pcm":"AQACAA=="}"#).unwrap();
        b.on_message(&msg, &mut events);
        let msg = json::parse(r#"{"t":"audio","fin":true}"#).unwrap();
        b.on_message(&msg, &mut events);
        let msg = json::parse(r#"{"t":"callar"}"#).unwrap();
        b.on_message(&msg, &mut events);
        assert_eq!(
            events,
            [
                BrainEvent::Audio {
                    rate: 22050,
                    pcm: alloc::vec![1, 2],
                    end: false
                },
                BrainEvent::Audio {
                    rate: 22050,
                    pcm: Vec::new(),
                    end: true
                },
                BrainEvent::Hush
            ]
        );
    }
}
