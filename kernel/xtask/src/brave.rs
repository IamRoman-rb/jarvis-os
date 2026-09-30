//! Puente de Brave (ADR 0007): Brave de verdad, dibujado en una ventana de JARVIS-OS.
//!
//! JARVIS-OS todavía no puede ejecutar programas de otros sistemas (eso es K10), así que Brave
//! corre en el anfitrión, **sin ventana** (`--headless=new`), y este puente hace de intermediario:
//!
//! ```text
//! JARVIS-OS ──TCP (remote.rs)──▶ 10.0.2.2:8119 ──▶ este puente ──DevTools (WebSocket)──▶ Brave
//! ```
//!
//! - **Imagen**: `Page.startScreencast` manda un JPEG cada vez que la página cambia. El puente lo
//!   decodifica, lo compara con el anterior en mosaicos de 64×64 y le manda al kernel solo los que
//!   cambiaron (RGB comprimido con LZ4). No manda otro cuadro hasta que el kernel confirma el
//!   anterior, y hasta entonces tampoco se lo confirma a Brave: así nadie se ahoga.
//! - **Entrada**: el mouse y el teclado del kernel se traducen a `Input.dispatch*`.
//! - **Pestañas**: cada pestaña es un "target" de DevTools; las ventanas emergentes también.
//!
//! El perfil (cookies, sesiones, historial) queda en `target/brave-perfil`, separado del Brave
//! que Roman usa en Windows. Por defecto solo escucha en 127.0.0.1; `cargo xtask brave --red
//! --token X` lo abre a la red local con un token (para un JARVIS instalado en otra máquina).

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use jarvis_desktop::remote::{
    self, Button, FromBrave, MouseKind, TILE, TabInfo, TabsState, Tile, ToBrave,
};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

/// Dónde puede estar Brave en Windows (instalado para todos o para el usuario).
fn brave_exe() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe"),
        PathBuf::from(r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe"),
        PathBuf::from("/usr/bin/brave-browser"),
        PathBuf::from("/usr/bin/brave"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.insert(
            0,
            PathBuf::from(local).join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
        );
    }
    if let Some(p) = std::env::var_os("JARVIS_BRAVE") {
        candidates.insert(0, PathBuf::from(p));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn installed() -> bool {
    brave_exe().is_some()
}

/// `cargo xtask brave --instalar`: lo instala con winget (Windows).
pub fn install() -> Result<(), String> {
    if let Some(p) = brave_exe() {
        println!("[brave] ya está instalado: {}", p.display());
        return Ok(());
    }
    let status = Command::new("winget")
        .args([
            "install",
            "-e",
            "--id",
            "Brave.Brave",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ])
        .status()
        .map_err(|e| format!("no se pudo ejecutar winget: {e}"))?;
    if !status.success() || brave_exe().is_none() {
        return Err("winget no pudo instalar Brave".into());
    }
    println!("[brave] instalado");
    Ok(())
}

/// Levanta el puente en un hilo (lo usan `run` y `screenshot`). Si el puerto está ocupado,
/// avisa y sigue sin Brave.
pub fn start(profile: PathBuf) {
    let listener = match TcpListener::bind(("127.0.0.1", remote::PORT)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "[brave] no pude escuchar en 127.0.0.1:{} ({e})",
                remote::PORT
            );
            return;
        }
    };
    match brave_exe() {
        Some(p) => println!(
            "[brave] puente en 127.0.0.1:{} con {}",
            remote::PORT,
            p.display()
        ),
        None => println!(
            "[brave] puente en 127.0.0.1:{} (Brave no está instalado: cargo xtask brave --instalar)",
            remote::PORT
        ),
    }
    thread::spawn(move || serve(listener, profile, String::new()));
}

/// `cargo xtask brave [--red --token X]`: el puente solo, en primer plano.
pub fn standalone(profile: PathBuf, lan: bool, token: String) -> Result<(), String> {
    if lan && token.len() < 8 {
        return Err("con --red hace falta --token de 8 caracteres o más".into());
    }
    let host = if lan { "0.0.0.0" } else { "127.0.0.1" };
    let listener = TcpListener::bind((host, remote::PORT))
        .map_err(|e| format!("no pude escuchar en {host}:{}: {e}", remote::PORT))?;
    println!(
        "[brave] puente en {host}:{} (Ctrl+C para cortar)",
        remote::PORT
    );
    serve(listener, profile, token);
    Ok(())
}

fn serve(listener: TcpListener, profile: PathBuf, token: String) {
    // Un solo Brave a la vez por perfil (Brave no deja abrir dos con la misma carpeta).
    let busy = Arc::new(Mutex::new(()));
    for stream in listener.incoming().flatten() {
        let (profile, token, busy) = (profile.clone(), token.clone(), busy.clone());
        thread::spawn(move || {
            let peer = stream
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let Ok(_guard) = busy.try_lock() else {
                let mut s = stream;
                let _ = s.write_all(
                    &FromBrave::Error("Brave ya está abierto en otra ventana de JARVIS".into())
                        .encode(),
                );
                return;
            };
            println!("[brave] conexión de {peer}");
            match session(stream, &profile, &token) {
                Ok(()) => println!("[brave] {peer} cerró"),
                Err(e) => eprintln!("[brave] {peer}: {e}"),
            }
        });
    }
}

// --- Una sesión: un JARVIS conectado, un Brave abierto -------------------------------------------

fn send(client: &mut TcpStream, m: &FromBrave) -> Result<(), String> {
    client
        .write_all(&m.encode())
        .map_err(|e| format!("el kernel se desconectó ({e})"))
}

/// Lee los mensajes del kernel en otro hilo y los pasa por un canal.
/// `framer` trae lo que ya llegó junto con el saludo.
fn reader(
    mut client: TcpStream,
    mut framer: remote::Framer,
    alive: Arc<AtomicBool>,
) -> Receiver<ToBrave> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            while let Some(m) = framer.next_message() {
                let Some(msg) = m.ok().and_then(|b| ToBrave::decode(&b)) else {
                    alive.store(false, Ordering::Relaxed);
                    return;
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
            let n = match client.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            framer.push(&buf[..n]);
        }
        alive.store(false, Ordering::Relaxed);
    });
    rx
}

fn session(mut client: TcpStream, profile: &PathBuf, token: &str) -> Result<(), String> {
    client.set_nodelay(true).ok();
    // Lo primero tiene que ser el saludo, y rápido.
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut framer = remote::Framer::default();
    let (w, h) = {
        let mut buf = [0u8; 1024];
        loop {
            if let Some(m) = framer.next_message() {
                match m.ok().and_then(|b| ToBrave::decode(&b)) {
                    Some(ToBrave::Hello { token: t, w, h }) => {
                        if !token.is_empty() && t != token {
                            send(&mut client, &FromBrave::Error("token incorrecto".into()))?;
                            return Err("token incorrecto".into());
                        }
                        break (w.clamp(200, 3840), h.clamp(150, 2160));
                    }
                    _ => return Err("no empezó con el saludo".into()),
                }
            }
            let n = client.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("se cerró antes del saludo".into());
            }
            framer.push(&buf[..n]);
        }
    };
    client.set_read_timeout(None).ok();

    let Some(exe) = brave_exe() else {
        send(
            &mut client,
            &FromBrave::Error(
                "Brave no está instalado en el anfitrión: corré cargo xtask brave --instalar"
                    .into(),
            ),
        )?;
        return Err("Brave no está instalado".into());
    };
    let mut brave = Brave::launch(&exe, profile, w, h)?;
    let alive = Arc::new(AtomicBool::new(true));
    let rx = reader(
        client.try_clone().map_err(|e| e.to_string())?,
        framer,
        alive.clone(),
    );
    let result = brave.run(&mut client, &rx, &alive, w, h);
    brave.quit();
    result
}

/// Una pestaña: un "target" de DevTools con su sesión.
struct Tab {
    target: String,
    session: String,
    title: String,
    url: String,
}

/// Para qué era cada pedido que todavía no tiene respuesta.
enum Pending {
    /// (target, título, dirección)
    Attach(String, String, String),
    History,
    /// `Target.getTargetInfo`: título y dirección al terminar de cargar.
    Info(String),
    Ignore,
}

struct Brave {
    child: Child,
    ws: WebSocket<TcpStream>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    tabs: Vec<Tab>,
    active: usize,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    size: (u16, u16),
    /// El último cuadro que mandó Brave (JPEG) y a qué sesión hay que confirmarlo.
    frame: Option<(Vec<u8>, String, u64)>,
    /// Lo que el kernel ya tiene en pantalla (para mandar solo lo que cambió).
    shown: Option<image::RgbImage>,
    /// El kernel todavía no confirmó el último cuadro.
    in_flight: bool,
    state_dirty: bool,
    /// Botón del mouse apretado (para los arrastres).
    held: Button,
    /// Dirección pedida antes de que hubiera una pestaña.
    pending_nav: Option<String>,
}

impl Brave {
    fn launch(exe: &PathBuf, profile: &PathBuf, w: u16, h: u16) -> Result<Brave, String> {
        std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
        kill_orphans(profile);
        set_aside_sessions(profile);
        let port_file = profile.join("DevToolsActivePort");
        let _ = std::fs::remove_file(&port_file);
        let child = Command::new(exe)
            .arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("--window-size={w},{h}"))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-features=Translate",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("no se pudo abrir Brave: {e}"))?;
        // Brave escribe en qué puerto quedó DevTools.
        let start = Instant::now();
        let (port, path) = loop {
            if let Ok(s) = std::fs::read_to_string(&port_file) {
                let mut l = s.lines();
                if let (Some(p), Some(path)) = (l.next(), l.next()) {
                    break (p.trim().to_string(), path.trim().to_string());
                }
            }
            if start.elapsed() > Duration::from_secs(20) {
                return Err("Brave no abrió DevTools a tiempo".into());
            }
            thread::sleep(Duration::from_millis(50));
        };
        let addr = format!("127.0.0.1:{port}");
        let tcp = TcpStream::connect(&addr).map_err(|e| format!("DevTools: {e}"))?;
        tcp.set_nodelay(true).ok();
        let (ws, _) = tungstenite::client(format!("ws://{addr}{path}"), tcp)
            .map_err(|e| format!("DevTools (WebSocket): {e}"))?;
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(5)))
            .map_err(|e| e.to_string())?;
        let mut b = Brave {
            child,
            ws,
            next_id: 0,
            pending: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            loading: false,
            can_back: false,
            can_forward: false,
            size: (w, h),
            frame: None,
            shown: None,
            in_flight: false,
            state_dirty: true,
            held: Button::None,
            pending_nav: None,
        };
        b.call(
            None,
            "Target.setDiscoverTargets",
            json!({"discover": true}),
            Pending::Ignore,
        )?;
        Ok(b)
    }

    fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
        pending: Pending,
    ) -> Result<(), String> {
        self.next_id += 1;
        let mut msg = json!({"id": self.next_id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.pending.insert(self.next_id, pending);
        self.ws
            .send(Message::Text(msg.to_string()))
            .map_err(|e| format!("DevTools: {e}"))
    }

    /// Un comando a la pestaña activa (si hay).
    fn tab_call(&mut self, method: &str, params: Value) -> Result<(), String> {
        let Some(s) = self.tabs.get(self.active).map(|t| t.session.clone()) else {
            return Ok(());
        };
        self.call(Some(&s), method, params, Pending::Ignore)
    }

    fn quit(&mut self) {
        let _ = self.call(None, "Browser.close", json!({}), Pending::Ignore);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }

    fn run(
        &mut self,
        client: &mut TcpStream,
        rx: &Receiver<ToBrave>,
        alive: &AtomicBool,
        w: u16,
        h: u16,
    ) -> Result<(), String> {
        self.size = (w, h);
        loop {
            if !alive.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                send(client, &FromBrave::Error("Brave se cerró".into()))?;
                return Err(format!("Brave terminó ({status})"));
            }
            loop {
                match rx.try_recv() {
                    Ok(m) => self.on_kernel(m)?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        self.on_brave(v)?;
                    }
                }
                Ok(Message::Close(_)) => return Err("DevTools se cerró".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(format!("DevTools: {e}")),
            }
            if !self.in_flight
                && let Some((jpeg, session, ack)) = self.frame.take()
            {
                if let Some(tiles) = self.diff(&jpeg) {
                    let (fw, fh) = self
                        .shown
                        .as_ref()
                        .map_or((0, 0), |i| (i.width() as u16, i.height() as u16));
                    send(
                        client,
                        &FromBrave::Frame {
                            w: fw,
                            h: fh,
                            tiles,
                        },
                    )?;
                    self.in_flight = true;
                }
                // Recién ahora Brave puede mandar el siguiente.
                self.call(
                    Some(&session),
                    "Page.screencastFrameAck",
                    json!({"sessionId": ack}),
                    Pending::Ignore,
                )?;
            }
            if self.state_dirty {
                self.state_dirty = false;
                send(client, &FromBrave::State(self.state()))?;
            }
        }
    }

    fn state(&self) -> TabsState {
        TabsState {
            tabs: self
                .tabs
                .iter()
                .map(|t| TabInfo {
                    title: t.title.clone(),
                    url: t.url.clone(),
                })
                .collect(),
            active: self.active as u16,
            loading: self.loading,
            can_back: self.can_back,
            can_forward: self.can_forward,
        }
    }

    /// Decodifica el JPEG y arma los mosaicos que cambiaron respecto de lo que ya se mostró.
    fn diff(&mut self, jpeg: &[u8]) -> Option<Vec<Tile>> {
        let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            .ok()?
            .to_rgb8();
        let (w, h) = (img.width(), img.height());
        let same_size = self
            .shown
            .as_ref()
            .is_some_and(|s| s.width() == w && s.height() == h);
        let mut tiles = Vec::new();
        let t = TILE as u32;
        for ty in (0..h).step_by(t as usize) {
            for tx in (0..w).step_by(t as usize) {
                let (tw, th) = (t.min(w - tx), t.min(h - ty));
                let changed = !same_size || {
                    let old = self.shown.as_ref()?;
                    (ty..ty + th).any(|y| {
                        let a = &img.as_raw()[((y * w + tx) * 3) as usize..][..(tw * 3) as usize];
                        let b = &old.as_raw()[((y * w + tx) * 3) as usize..][..(tw * 3) as usize];
                        a != b
                    })
                };
                if changed {
                    let mut rgb = Vec::with_capacity((tw * th * 3) as usize);
                    for y in ty..ty + th {
                        rgb.extend_from_slice(
                            &img.as_raw()[((y * w + tx) * 3) as usize..][..(tw * 3) as usize],
                        );
                    }
                    tiles.push(remote::compress_tile(
                        tx as u16, ty as u16, tw as u16, th as u16, &rgb,
                    ));
                }
            }
        }
        self.shown = Some(img);
        (!tiles.is_empty()).then_some(tiles)
    }

    // --- Lo que llega del kernel ---------------------------------------------------------------

    fn on_kernel(&mut self, m: ToBrave) -> Result<(), String> {
        match m {
            ToBrave::Hello { .. } => {}
            ToBrave::FrameAck => self.in_flight = false,
            ToBrave::Resize { w, h } => {
                self.size = (w.clamp(200, 3840), h.clamp(150, 2160));
                let sessions: Vec<String> = self.tabs.iter().map(|t| t.session.clone()).collect();
                for s in sessions {
                    self.metrics(&s)?;
                }
            }
            ToBrave::Mouse {
                kind,
                button,
                x,
                y,
                clicks,
                mods,
            } => {
                let (ty, btn) = match kind {
                    MouseKind::Move => ("mouseMoved", self.held),
                    MouseKind::Down => {
                        self.held = button;
                        ("mousePressed", button)
                    }
                    MouseKind::Up => {
                        self.held = Button::None;
                        ("mouseReleased", button)
                    }
                };
                let buttons = match self.held {
                    Button::Left => 1,
                    Button::Right => 2,
                    Button::Middle => 4,
                    Button::None => 0,
                };
                self.tab_call(
                    "Input.dispatchMouseEvent",
                    json!({"type": ty, "x": x, "y": y, "button": button_name(btn),
                           "buttons": buttons, "clickCount": clicks, "modifiers": mods}),
                )?;
            }
            ToBrave::Wheel { x, y, dx, dy } => self.tab_call(
                "Input.dispatchMouseEvent",
                json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}),
            )?,
            ToBrave::Key {
                key,
                vk,
                text,
                mods,
            } => {
                let typing = !text.is_empty() && mods & (remote::MOD_CTRL | remote::MOD_ALT) == 0;
                let down = if typing {
                    json!({"type": "keyDown", "key": key, "text": text, "unmodifiedText": text,
                           "windowsVirtualKeyCode": vk, "modifiers": mods})
                } else {
                    json!({"type": "rawKeyDown", "key": key, "windowsVirtualKeyCode": vk,
                           "modifiers": mods})
                };
                self.tab_call("Input.dispatchKeyEvent", down)?;
                self.tab_call(
                    "Input.dispatchKeyEvent",
                    json!({"type": "keyUp", "key": key, "windowsVirtualKeyCode": vk,
                           "modifiers": mods}),
                )?;
            }
            ToBrave::Navigate(url) => {
                if self.tabs.is_empty() {
                    // La primera pestaña todavía se está conectando.
                    self.pending_nav = Some(url);
                } else {
                    self.tab_call("Page.navigate", json!({"url": url}))?;
                }
            }
            ToBrave::Back | ToBrave::Forward => {
                let back = matches!(m, ToBrave::Back);
                self.tab_call(
                    "Runtime.evaluate",
                    json!({"expression": if back { "history.back()" } else { "history.forward()" }}),
                )?;
            }
            ToBrave::Reload => self.tab_call("Page.reload", json!({}))?,
            ToBrave::TabNew => self.call(
                None,
                "Target.createTarget",
                json!({"url": "https://search.brave.com/"}),
                Pending::Ignore,
            )?,
            ToBrave::TabClose(i) => {
                if let Some(t) = self.tabs.get(i as usize).map(|t| t.target.clone()) {
                    self.call(
                        None,
                        "Target.closeTarget",
                        json!({"targetId": t}),
                        Pending::Ignore,
                    )?;
                }
            }
            ToBrave::TabSelect(i) => self.select(i as usize)?,
        }
        Ok(())
    }

    fn metrics(&mut self, session: &str) -> Result<(), String> {
        let (w, h) = self.size;
        self.call(
            Some(session),
            "Emulation.setDeviceMetricsOverride",
            json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": false}),
            Pending::Ignore,
        )
    }

    /// Cambia de pestaña: el screencast pasa a la nueva.
    fn select(&mut self, i: usize) -> Result<(), String> {
        if i >= self.tabs.len() {
            return Ok(());
        }
        if let Some(old) = self.tabs.get(self.active).map(|t| t.session.clone())
            && self.active != i
        {
            self.call(
                Some(&old),
                "Page.stopScreencast",
                json!({}),
                Pending::Ignore,
            )?;
        }
        self.active = i;
        self.frame = None;
        self.shown = None; // la próxima imagen va entera
        let (target, session) = (self.tabs[i].target.clone(), self.tabs[i].session.clone());
        self.call(
            None,
            "Target.activateTarget",
            json!({"targetId": target}),
            Pending::Ignore,
        )?;
        self.call(
            Some(&session),
            "Page.startScreencast",
            json!({"format": "jpeg", "quality": 88, "everyNthFrame": 1}),
            Pending::Ignore,
        )?;
        self.call(
            Some(&session),
            "Page.getNavigationHistory",
            json!({}),
            Pending::History,
        )?;
        self.state_dirty = true;
        Ok(())
    }

    // --- Lo que llega de Brave -----------------------------------------------------------------

    fn on_brave(&mut self, v: Value) -> Result<(), String> {
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            let Some(p) = self.pending.remove(&id) else {
                return Ok(());
            };
            let r = &v["result"];
            match p {
                Pending::Attach(target, title, url) => {
                    if let Some(session) = r["sessionId"].as_str() {
                        self.attached(target, session.to_string(), title, url)?;
                    }
                }
                Pending::History => {
                    if let Some(entries) = r["entries"].as_array() {
                        let cur = r["currentIndex"].as_i64().unwrap_or(0);
                        self.can_back = cur > 0;
                        self.can_forward = (cur + 1) < entries.len() as i64;
                        self.state_dirty = true;
                    }
                }
                Pending::Info(target) => {
                    let info = &r["targetInfo"];
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.target == target) {
                        t.title = info["title"].as_str().unwrap_or("").to_string();
                        t.url = info["url"].as_str().unwrap_or("").to_string();
                        self.state_dirty = true;
                    }
                }
                Pending::Ignore => {}
            }
            return Ok(());
        }
        let method = v["method"].as_str().unwrap_or("");
        let p = &v["params"];
        let session = v["sessionId"].as_str().unwrap_or("").to_string();
        match method {
            "Target.targetCreated" => {
                let info = &p["targetInfo"];
                if info["type"] == "page" {
                    let target = info["targetId"].as_str().unwrap_or("").to_string();
                    let title = info["title"].as_str().unwrap_or("").to_string();
                    let url = info["url"].as_str().unwrap_or("").to_string();
                    self.call(
                        None,
                        "Target.attachToTarget",
                        json!({"targetId": target, "flatten": true}),
                        Pending::Attach(target.clone(), title, url),
                    )?;
                }
            }
            "Target.targetInfoChanged" => {
                let info = &p["targetInfo"];
                let id = info["targetId"].as_str().unwrap_or("");
                if let Some(i) = self.tabs.iter().position(|t| t.target == id) {
                    self.tabs[i].title = info["title"].as_str().unwrap_or("").to_string();
                    self.tabs[i].url = info["url"].as_str().unwrap_or("").to_string();
                    self.state_dirty = true;
                    if i == self.active {
                        let s = self.tabs[i].session.clone();
                        self.call(
                            Some(&s),
                            "Page.getNavigationHistory",
                            json!({}),
                            Pending::History,
                        )?;
                    }
                }
            }
            "Target.targetDestroyed" => {
                let id = p["targetId"].as_str().unwrap_or("");
                if let Some(i) = self.tabs.iter().position(|t| t.target == id) {
                    self.tabs.remove(i);
                    self.state_dirty = true;
                    if self.tabs.is_empty() {
                        // Sin pestañas no hay nada que mostrar: se abre una nueva.
                        self.call(
                            None,
                            "Target.createTarget",
                            json!({"url": "https://search.brave.com/"}),
                            Pending::Ignore,
                        )?;
                    } else if i <= self.active {
                        let next = self.active.saturating_sub(1).min(self.tabs.len() - 1);
                        self.active = usize::MAX;
                        self.select(next)?;
                    }
                }
            }
            "Page.screencastFrame" => {
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.session == session)
                {
                    let data = p["data"].as_str().unwrap_or("");
                    let ack = p["sessionId"].as_u64().unwrap_or(0);
                    if let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) {
                        // Si había uno esperando, se reemplaza (y se confirma el viejo).
                        if let Some((_, s, old)) = self.frame.replace((jpeg, session, ack)) {
                            self.call(
                                Some(&s),
                                "Page.screencastFrameAck",
                                json!({"sessionId": old}),
                                Pending::Ignore,
                            )?;
                        }
                    }
                }
            }
            "Page.frameStartedLoading" | "Page.frameStoppedLoading" => {
                let loading = method == "Page.frameStartedLoading";
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.session == session)
                    && self.loading != loading
                {
                    self.loading = loading;
                    self.state_dirty = true;
                }
                // El título definitivo recién está cuando termina de cargar.
                if !loading && let Some(t) = self.tabs.iter().find(|t| t.session == session) {
                    let target = t.target.clone();
                    self.call(
                        None,
                        "Target.getTargetInfo",
                        json!({"targetId": target}),
                        Pending::Info(target.clone()),
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Una pestaña nueva quedó conectada: se prepara y pasa a ser la activa.
    fn attached(
        &mut self,
        target: String,
        session: String,
        title: String,
        url: String,
    ) -> Result<(), String> {
        self.call(Some(&session), "Page.enable", json!({}), Pending::Ignore)?;
        self.metrics(&session)?;
        self.tabs.push(Tab {
            target,
            session,
            title,
            url,
        });
        self.select(self.tabs.len() - 1)?;
        if let Some(url) = self.pending_nav.take() {
            self.tab_call("Page.navigate", json!({"url": url}))?;
        }
        Ok(())
    }
}

/// Si un `xtask` anterior se cortó de golpe (Ctrl+C), su Brave sin ventana puede seguir vivo y
/// con el perfil tomado: el nuevo no arrancaría. Se cierran solo los que usan **este** perfil
/// (el Brave de Roman usa otro y no se toca).
fn kill_orphans(profile: &std::path::Path) {
    if !cfg!(windows) {
        return;
    }
    let dir = profile.display().to_string().replace('\'', "''");
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='brave.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*--user-data-dir={dir}*' }} | \
         ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Aparta las pestañas guardadas de la corrida anterior para que Brave no las restaure.
///
/// Desde Brave 154 (2026-09), restaurar varias pestañas en modo headless lo hace crashear al
/// poco de conectar DevTools (volcados en `Crashpad/reports`), y además se acumulaba una pestaña
/// por corrida. No hay un switch que lo apague: ni `--no-startup-window` sirve, porque Chromium
/// restaura igual al abrir la primera ventana. `Default/Sessions` guarda solo las pestañas; las
/// cookies y las sesiones iniciadas están en otros archivos y no se tocan. Se mueve (no se
/// borra) a `Sessions.anterior`, que guarda las de la última corrida por si hicieran falta.
fn set_aside_sessions(profile: &std::path::Path) {
    let dir = profile.join("Default");
    let sessions = dir.join("Sessions");
    if !sessions.exists() {
        return;
    }
    let previous = dir.join("Sessions.anterior");
    // Solo se reemplaza la copia que dejó este mismo puente en la corrida de antes.
    let _ = std::fs::remove_dir_all(&previous);
    if let Err(e) = std::fs::rename(&sessions, &previous) {
        eprintln!("[brave] no se pudieron apartar las pestañas guardadas: {e}");
    }
}

fn button_name(b: Button) -> &'static str {
    match b {
        Button::None => "none",
        Button::Left => "left",
        Button::Middle => "middle",
        Button::Right => "right",
    }
}
