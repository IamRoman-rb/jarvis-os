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

/// Levanta `uv run jarvis serve` (con `simulated`, respuestas fijas sin Claude). Sin `uv` avisa
/// y sigue: JARVIS arranca igual, con "CEREBRO sin conectar".
pub fn start(port: u16, simulated: bool) -> Option<Brain> {
    let repo = workspace_root().parent()?.to_path_buf();
    let token = token();
    let mut cmd = Command::new("uv");
    cmd.current_dir(&repo)
        .args(["run", "--quiet", "jarvis", "serve", "--puerto"])
        .arg(port.to_string())
        .env("JARVIS_CEREBRO_TOKEN", &token)
        .stdin(Stdio::null());
    if simulated {
        cmd.arg("--simulado");
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
