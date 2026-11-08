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

// --- `cargo xtask vbox-instalar`: instalar en VirtualBox, de punta a punta --------------------

/// La máquina descartable de la prueba. Nunca se toca otra (la de Roman es "JARVISOS").
const TEST_VM: &str = "JARVISOS-prueba";
const TEST_USER: &str = "roman";
const TEST_PASSWORD: &str = "jarvis2026";

/// Una combinación de hardware de VirtualBox para probar.
struct Variant {
    name: &'static str,
    chipset: &'static str,
    bus: &'static str,
    controller: &'static str,
    nic: &'static str,
}

const VARIANTS: [Variant; 2] = [
    // Como la VM de Roman: PIIX3, IDE y la placa de red AMD PCnet.
    Variant {
        name: "ide-pcnet",
        chipset: "piix3",
        bus: "ide",
        controller: "PIIX4",
        nic: "Am79C973",
    },
    // La de un VirtualBox nuevo: ICH9, SATA (AHCI) e Intel PRO/1000.
    Variant {
        name: "sata-e1000",
        chipset: "ich9",
        bus: "sata",
        controller: "IntelAhci",
        nic: "82540EM",
    },
];

/// Espera a que el registro del puerto serie (un archivo) diga `marker`.
fn wait_serial(log: &std::path::Path, marker: &str, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        let text = std::fs::read(log)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        if text.contains(marker) {
            println!("vbox: {marker}");
            return Ok(());
        }
        if start.elapsed() > timeout {
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(25)..].join("\n");
            return Err(format!(
                "VirtualBox: no apareció «{marker}» en {} s. Lo último del registro:\n{tail}",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn key_enter(vm: &str) -> Result<()> {
    manage(&["controlvm", vm, "keyboardputscancode", "1c", "9c"])?;
    thread::sleep(Duration::from_millis(400));
    Ok(())
}

fn type_text(vm: &str, text: &str) -> Result<()> {
    manage(&["controlvm", vm, "keyboardputstring", text])?;
    thread::sleep(Duration::from_millis(400));
    Ok(())
}

fn wait_off(vm: &str) {
    for _ in 0..80 {
        if !matches!(
            state(vm).as_deref(),
            Ok("running" | "stopping" | "paused" | "starting")
        ) {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    thread::sleep(Duration::from_secs(1));
}

/// Apaga la VM de prueba (si está prendida) y la borra con sus discos.
fn remove_test_vm() {
    if state(TEST_VM).is_ok() {
        let _ = manage(&["controlvm", TEST_VM, "poweroff"]);
        wait_off(TEST_VM);
        let _ = manage(&["unregistervm", TEST_VM, "--delete"]);
    }
}

/// `cargo xtask vbox-instalar [variante]`: con la ISO, crea una VM descartable por cada
/// variante de hardware, instala JARVIS-OS con el asistente, saca la ISO, arranca desde el
/// disco, inicia sesión con la contraseña y borra la VM. Las capturas quedan en
/// `target/vbox-prueba/`.
pub fn install_test(iso: &std::path::Path) -> Result<()> {
    let only = env::args().skip(2).find(|a| !a.starts_with("--"));
    let work = crate::target_dir().join("vbox-prueba");
    let _ = std::fs::create_dir_all(&work);
    let mut done = Vec::new();
    for v in VARIANTS
        .iter()
        .filter(|v| only.as_deref().is_none_or(|o| o == v.name))
    {
        remove_test_vm();
        let r = install_variant(v, iso, &work);
        let shot = work.join(format!("{}-final.png", v.name));
        let _ = manage(&[
            "controlvm",
            TEST_VM,
            "screenshotpng",
            &shot.display().to_string(),
        ]);
        remove_test_vm();
        r?;
        done.push(v.name);
    }
    println!(
        "ok: JARVIS-OS se instaló en VirtualBox ({}) con el asistente, arrancó del disco y pidió la contraseña",
        done.join(", ")
    );
    Ok(())
}

fn install_variant(v: &Variant, iso: &std::path::Path, work: &std::path::Path) -> Result<()> {
    println!(
        "vbox: variante {} ({}, {}, red {})",
        v.name, v.chipset, v.bus, v.nic
    );
    let base = work.display().to_string();
    let disk = work.join(format!("{}.vdi", v.name));
    let _ = std::fs::remove_file(&disk);
    let log = work.join(format!("{}-serie.log", v.name));
    let _ = std::fs::remove_file(&log);
    manage(&[
        "createvm",
        "--name",
        TEST_VM,
        "--ostype",
        "Other_64",
        "--basefolder",
        &base,
        "--register",
    ])?;
    manage(&[
        "modifyvm",
        TEST_VM,
        "--firmware",
        "efi64",
        "--memory",
        "2048",
        "--cpus",
        "2",
        "--chipset",
        v.chipset,
        "--graphicscontroller",
        "vboxsvga",
        "--vram",
        "32",
        "--nic1",
        "nat",
        "--nictype1",
        v.nic,
        "--usbxhci",
        "on",
        "--audio-enabled",
        "off",
        "--uart1",
        "0x3F8",
        "4",
        "--uartmode1",
        "file",
        &log.display().to_string(),
    ])?;
    manage(&[
        "storagectl",
        TEST_VM,
        "--name",
        "Discos",
        "--add",
        v.bus,
        "--controller",
        v.controller,
    ])?;
    let disk_s = disk.display().to_string();
    manage(&[
        "createmedium",
        "disk",
        "--filename",
        &disk_s,
        "--size",
        "512",
        "--format",
        "VDI",
    ])?;
    let iso_s = iso.display().to_string();
    let attach = |port: &str, kind: &str, medium: &str| {
        manage(&[
            "storageattach",
            TEST_VM,
            "--storagectl",
            "Discos",
            "--port",
            port,
            "--device",
            "0",
            "--type",
            kind,
            "--medium",
            medium,
        ])
    };
    attach("0", "hdd", &disk_s)?;
    attach("1", "dvddrive", &iso_s)?;
    manage(&["startvm", TEST_VM, "--type", "headless"])?;
    let boot = Duration::from_secs(300);
    let step = Duration::from_secs(60);
    wait_serial(&log, "MODO_EN_VIVO", boot)?;
    wait_serial(&log, "ESCRITORIO_MENU instalador", boot)?;
    wait_serial(&log, "JARVIS_BOOT_OK", boot)?;
    thread::sleep(Duration::from_secs(2));
    key_enter(TEST_VM)?; // bienvenida → usuario
    type_text(TEST_VM, TEST_USER)?;
    key_enter(TEST_VM)?;
    type_text(TEST_VM, TEST_PASSWORD)?;
    key_enter(TEST_VM)?;
    type_text(TEST_VM, TEST_PASSWORD)?;
    key_enter(TEST_VM)?; // → disco (el único vacío)
    key_enter(TEST_VM)?; // → confirmar
    key_enter(TEST_VM)?; // instalar
    wait_serial(&log, "INSTALAR_PEDIDO", step)?;
    wait_serial(&log, "INSTALAR_LISTO", Duration::from_secs(300))?;
    wait_serial(&log, "INSTALADOR_LISTO", step)?;
    thread::sleep(Duration::from_secs(1));
    let shot = work.join(format!("{}-instalado.png", v.name));
    manage(&[
        "controlvm",
        TEST_VM,
        "screenshotpng",
        &shot.display().to_string(),
    ])?;

    // Apagar, sacar la ISO y arrancar desde el disco.
    manage(&["controlvm", TEST_VM, "poweroff"])?;
    wait_off(TEST_VM);
    attach("1", "dvddrive", "emptydrive")?;
    let log2 = work.join(format!("{}-serie-disco.log", v.name));
    let _ = std::fs::remove_file(&log2);
    manage(&[
        "modifyvm",
        TEST_VM,
        "--uartmode1",
        "file",
        &log2.display().to_string(),
    ])?;
    manage(&["startvm", TEST_VM, "--type", "headless"])?;
    wait_serial(&log2, "DISCO_SISTEMA", boot)?;
    wait_serial(&log2, "JARVIS_BOOT_OK", boot)?;
    thread::sleep(Duration::from_secs(2));
    type_text(TEST_VM, "otra-clave")?;
    key_enter(TEST_VM)?;
    wait_serial(&log2, "ESCRITORIO_PIN_INCORRECTO", step)?;
    type_text(TEST_VM, TEST_PASSWORD)?;
    key_enter(TEST_VM)?;
    wait_serial(&log2, &format!("SESION_INICIADA {TEST_USER}"), step)?;
    wait_serial(&log2, "JARVIS_HABLA: Bienvenido a JARVIS-OS", step)?;
    thread::sleep(Duration::from_secs(3));
    let shot = work.join(format!("{}-sesion.png", v.name));
    manage(&[
        "controlvm",
        TEST_VM,
        "screenshotpng",
        &shot.display().to_string(),
    ])?;
    println!("vbox: {} ok (capturas en {})", v.name, work.display());
    Ok(())
}
