//! El cerebro de JARVIS (`jarvis serve`, en Python) para QEMU (ADR 0008).
//!
//! Se genera un token al azar por sesión: el servicio lo recibe por una variable de entorno y el
//! kernel por fw_cfg (`opt/jarvis/cerebro` = "puerto token"), así solo esta máquina virtual puede
//! hablarle. El servicio escucha solo en 127.0.0.1 y se cierra cuando termina `xtask`.

use std::hash::{BuildHasher, RandomState};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::workspace_root;

/// Puerto de `cargo xtask run`; los tests usan otro, así no chocan con uno abierto.
pub const PORT: u16 = 8121;
pub const TEST_PORT: u16 = 8122;

/// Lo que va por fw_cfg ("puerto token"), si el cerebro arrancó.
static FW_CFG: Mutex<Option<String>> = Mutex::new(None);

/// El proceso del cerebro: se cierra al soltarlo.
pub struct Brain(Child);

impl Drop for Brain {
    fn drop(&mut self) {
        // `uv run` lanza Python como otro proceso; en Windows, matar a `uv` no lo cierra (y queda
        // ocupando el puerto). taskkill /T cierra el árbol entero.
        if cfg!(windows) {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 32 caracteres al azar (RandomState se siembra con el generador del sistema operativo).
fn token() -> String {
    let s = RandomState::new();
    (0..2u64)
        .map(|i| format!("{:016x}", s.hash_one((i, std::process::id()))))
        .collect()
}

/// Para `qemu()`: el archivo de fw_cfg con el puerto y el token.
pub fn fw_cfg() -> Option<String> {
    FW_CFG.lock().ok()?.clone()
}

/// Cierra los cerebros que quedaron de otra corrida (por ejemplo, si se cerró la terminal de
/// golpe): son `jarvis serve` lanzados por xtask, nunca otro programa.
fn kill_orphans() {
    if !cfg!(windows) {
        return;
    }
    let script = "Get-CimInstance Win32_Process | Where-Object { $_.Name -in 'python.exe','jarvis.exe','uv.exe' -and $_.CommandLine -match 'jarvis(\\.exe)?\"? serve --puerto' } | ForEach-Object { $_.ProcessId }";
    let Ok(out) = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    else {
        return;
    };
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        if pid.parse::<u32>().is_ok() {
            eprintln!("cerebro: cierro uno viejo que quedó abierto (proceso {pid})");
            let _ = Command::new("taskkill")
                .args(["/PID", pid, "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Un puerto libre, empezando por `port`.
fn free_port(port: u16) -> u16 {
    (port..port + 20)
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .unwrap_or(port)
}

/// Levanta `uv run jarvis serve` (con `simulated`, respuestas fijas sin Claude). Sin `uv` avisa
/// y sigue: JARVIS arranca igual, con "CEREBRO sin conectar".
pub fn start(port: u16, simulated: bool) -> Option<Brain> {
    let repo = workspace_root().parent()?.to_path_buf();
    if !simulated {
        kill_orphans();
    }
    let port = free_port(port);
    let token = token();
    let mut cmd = Command::new("uv");
    cmd.current_dir(&repo).args(["run", "--quiet"]);
    if !simulated {
        // La voz es un extra: un `uv sync` suelto lo desinstala y JARVIS quedaba sordo y mudo sin
        // avisar. Pedirlo acá lo reinstala si falta (el simulado no usa la voz).
        cmd.args(["--extra", "voice"]);
    }
    cmd.args(["jarvis", "serve", "--puerto"])
        .arg(port.to_string())
        .env("JARVIS_CEREBRO_TOKEN", &token)
        // Si xtask muere sin cerrarlo (Ctrl+C, cerrar la terminal), el cerebro se cierra solo.
        .env("JARVIS_PADRE", std::process::id().to_string())
        // Para que JARVIS se modifique a sí mismo: el repo, la marca para que `run` recompile y
        // reinicie, y dónde contarle cómo salió (ver `crate::run` y src/jarvis/update.py).
        .env("JARVIS_REPO", &repo)
        .env("JARVIS_REINICIO", crate::restart_marker())
        .env("JARVIS_ACTUALIZACION", crate::update_result())
        .stdin(Stdio::null());
    if simulated {
        // Proyectos de mentira: el simulado no toca los de verdad.
        let demo = crate::target_dir().join("proyectos-prueba");
        let _ = std::fs::create_dir_all(demo.join("demo"));
        cmd.arg("--simulado").arg("--proyectos").arg(demo);
    }
    match cmd.spawn() {
        Ok(child) => {
            if let Ok(mut f) = FW_CFG.lock() {
                *f = Some(format!("{port} {token}"));
            }
            Some(Brain(child))
        }
        Err(e) => {
            eprintln!(
                "cerebro: no pude ejecutar `uv run jarvis serve` ({e}); JARVIS sigue sin Claude"
            );
            None
        }
    }
}
