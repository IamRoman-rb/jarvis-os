//! `cargo xtask vbox [MÁQUINA] [--iso] [--simulado] [--sin-ventana]`: JARVIS-OS en VirtualBox con el cerebro, como
//! `cargo xtask run` en QEMU.
//!
//! En QEMU el kernel recibe el puerto y el token del cerebro por un archivo de fw_cfg
//! (`opt/jarvis/cerebro`). VirtualBox 7 tiene el mismo dispositivo (`qemu-fw-cfg`) pero solo con
//! elementos fijos: se usa la "línea de comandos" (`jarvis.cerebro=puerto,token`, ver
//! kernel/src/fw_cfg.rs). Además, desde 7.0 la red NAT ya no deja llegar al `localhost` de la PC
//! (donde escucha el cerebro) salvo que se lo pida: `--nat-localhostreachable1 on`.
//!
//! Sin esto, los botones de Configuración → Asistente ("iniciar sesión con Google") no hacen
//! nada en la máquina virtual: es el cerebro, en la PC, el que abre el navegador.

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::{Result, cerebro};

const DEFAULT_VM: &str = "JARVISOS";
/// Donde el cerebro le deja el puerto y el token a la máquina.
const CMDLINE_KEY: &str = "VBoxInternal/Devices/qemu-fw-cfg/0/Config/CmdLine";

fn vboxmanage() -> PathBuf {
    if let Ok(dir) = env::var("VBOX_MSI_INSTALL_PATH") {
        let p = PathBuf::from(dir).join("VBoxManage.exe");
        if p.exists() {
            return p;
        }
    }
    let default = PathBuf::from(r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe");
    if default.exists() {
        default
    } else {
        PathBuf::from("VBoxManage")
    }
}

fn manage(args: &[&str]) -> Result<String> {
    let out = Command::new(vboxmanage())
        .args(args)
        .output()
        .map_err(|e| format!("no pude ejecutar VBoxManage ({e}): ¿está instalado VirtualBox?"))?;
    if !out.status.success() {
        return Err(format!(
            "VBoxManage {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// El estado de la máquina ("running", "poweroff", "aborted"...).
fn state(vm: &str) -> Result<String> {
    let info = manage(&["showvminfo", vm, "--machinereadable"])?;
    Ok(info
        .lines()
        .find_map(|l| l.strip_prefix("VMState="))
        .unwrap_or("")
        .trim_matches('"')
        .to_string())
}

pub fn run(image: Option<&std::path::Path>) -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let simulated = args.iter().any(|a| a == "--simulado");
    let vm = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_VM.into());
    let st = state(&vm).map_err(|e| {
        format!("{e}\n(la máquina se llama \"{vm}\"? uso: cargo xtask vbox [MÁQUINA] [--iso])")
    })?;
    if st == "running" || st == "paused" {
        return Err(format!(
            "\"{vm}\" está encendida: apagala y volvé a correr esto (el token se lee al arrancar)"
        ));
    }
    if let Some(iso) = image {
        let ctl = manage(&["showvminfo", &vm, "--machinereadable"])?
            .lines()
            .find_map(|l| l.strip_prefix("storagecontrollername0="))
            .map(|s| s.trim_matches('"').to_string())
            .ok_or("la máquina no tiene controladora de discos")?;
        let iso = iso.display().to_string();
        manage(&[
            "storageattach",
            &vm,
            "--storagectl",
            &ctl,
            "--port",
            "1",
            "--device",
            "0",
            "--type",
            "dvddrive",
            "--medium",
            &iso,
        ])?;
        println!("vbox: ISO montada ({iso})");
    }

    let brain = cerebro::start(cerebro::PORT, simulated);
    let Some((port, token)) = cerebro::fw_cfg().and_then(|s| {
        let (p, t) = s.split_once(' ')?;
        Some((p.to_string(), t.to_string()))
    }) else {
        return Err("no pude levantar el cerebro (`uv run jarvis serve`)".into());
    };
    manage(&[
        "setextradata",
        &vm,
        CMDLINE_KEY,
        &format!("jarvis.cerebro={port},{token}"),
    ])?;
    // La NAT de VirtualBox 7 no deja llegar a 127.0.0.1 de la PC (10.0.2.2) si no se pide.
    manage(&["modifyvm", &vm, "--nat-localhostreachable1", "on"])?;
    let window = if args.iter().any(|a| a == "--sin-ventana") {
        "headless"
    } else {
        "gui"
    };
    manage(&["startvm", &vm, "--type", window])?;
    println!(
        "vbox: \"{vm}\" arrancó con el cerebro en el puerto {port}{}. Cerrá la máquina para terminar.",
        if simulated { " (simulado)" } else { "" }
    );
    // El cerebro vive mientras la máquina esté prendida.
    loop {
        thread::sleep(Duration::from_secs(2));
        match state(&vm).as_deref() {
            Ok("running" | "paused" | "starting" | "restoring" | "stopping" | "saving") => {}
            _ => break,
        }
    }
    drop(brain);
    println!("vbox: la máquina se apagó; cerebro cerrado");
    Ok(())
}
