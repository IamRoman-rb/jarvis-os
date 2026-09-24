//! `cargo xtask <comando>`: arma la imagen booteable de JARVIS-OS y la corre en QEMU.
//!
//! - `build`       compila el kernel y crea `target/jarvis-os-uefi.img`
//! - `run`         abre QEMU con ventana
//! - `test`        arranca sin ventana y espera `JARVIS_BOOT_OK` por el puerto serie (CI)
//! - `screenshot`  arranca sin ventana y guarda la pantalla en `target/jarvis-os.png`
//! - `vdi`         convierte la imagen a `target/jarvis-os.vdi` para VirtualBox

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

const BOOT_MARKER: &str = "JARVIS_BOOT_OK";
const BOOT_TIMEOUT: Duration = Duration::from_secs(90);

fn main() -> ExitCode {
    let cmd = env::args().nth(1).unwrap_or_default();
    let result = match cmd.as_str() {
        "build" => build().map(|img| println!("imagen: {}", img.display())),
        "run" => build().and_then(|img| run(&img)),
        "test" => build().and_then(|img| test(&img)),
        "screenshot" => build().and_then(|img| screenshot(&img)),
        "vdi" => build().and_then(|img| vdi(&img)),
        _ => Err("uso: cargo xtask <build|run|test|screenshot|vdi>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask vive dentro del workspace")
        .into()
}

fn target_dir() -> PathBuf {
    workspace_root().join("target")
}

// --- build ------------------------------------------------------------------------------------

fn build() -> Result<PathBuf> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(workspace_root())
        .args([
            "build",
            "--package",
            "jarvis-kernel",
            "--target",
            "x86_64-unknown-none",
            "--release",
        ])
        .status()
        .map_err(|e| format!("no pude ejecutar cargo: {e}"))?;
    if !status.success() {
        return Err("falló la compilación del kernel".into());
    }
    let kernel = target_dir().join("x86_64-unknown-none/release/jarvis-kernel");
    let image = target_dir().join("jarvis-os-uefi.img");
    bootloader::UefiBoot::new(&kernel)
        .create_disk_image(&image)
        .map_err(|e| format!("no pude crear la imagen UEFI: {e}"))?;
    Ok(image)
}

// --- QEMU -------------------------------------------------------------------------------------

fn find_first(env_var: &str, candidates: &[&str]) -> Option<PathBuf> {
    if let Ok(p) = env::var(env_var) {
        return Some(p.into());
    }
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

fn qemu_binary() -> PathBuf {
    find_first("QEMU", &[r"C:\Program Files\qemu\qemu-system-x86_64.exe"])
        .unwrap_or_else(|| "qemu-system-x86_64".into())
}

/// Firmware UEFI (OVMF): la parte de código (solo lectura) y una copia escribible de las
/// variables, para no modificar el archivo original.
fn ovmf() -> Result<(PathBuf, Option<PathBuf>)> {
    let code = find_first(
        "OVMF_CODE",
        &[
            r"C:\Program Files\qemu\share\edk2-x86_64-code.fd",
            "/usr/share/OVMF/OVMF_CODE_4M.fd",
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/qemu/edk2-x86_64-code.fd",
        ],
    )
    .ok_or("no encontré el firmware OVMF: instalá QEMU (Windows) o el paquete `ovmf` (Linux), o definí OVMF_CODE")?;
    let vars_src = find_first(
        "OVMF_VARS",
        &[
            r"C:\Program Files\qemu\share\edk2-i386-vars.fd",
            "/usr/share/OVMF/OVMF_VARS_4M.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
            "/usr/share/qemu/edk2-i386-vars.fd",
        ],
    );
    let vars = match vars_src {
        Some(src) => {
            let dst = target_dir().join("ovmf-vars.fd");
            fs::copy(&src, &dst).map_err(|e| format!("no pude copiar {}: {e}", src.display()))?;
            Some(dst)
        }
        None => None,
    };
    Ok((code, vars))
}

fn qemu(image: &Path, headless: bool) -> Result<Command> {
    let (code, vars) = ovmf()?;
    let mut cmd = Command::new(qemu_binary());
    cmd.args([
        "-machine",
        "q35",
        "-m",
        "512M",
        "-rtc",
        "base=utc",
        "-no-reboot",
        "-serial",
        "stdio",
    ])
    .arg("-drive")
    .arg(format!(
        "if=pflash,format=raw,readonly=on,file={}",
        code.display()
    ));
    if let Some(vars) = vars {
        cmd.arg("-drive")
            .arg(format!("if=pflash,format=raw,file={}", vars.display()));
    }
    cmd.arg("-drive")
        .arg(format!("format=raw,file={}", image.display()));
    if headless {
        cmd.args(["-display", "none"]);
    }
    Ok(cmd)
}

fn run(image: &Path) -> Result<()> {
    let status = qemu(image, false)?
        .status()
        .map_err(|e| format!("no pude abrir QEMU ({}): {e}", qemu_binary().display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("QEMU terminó con {status}"))
    }
}

/// Arranca QEMU y espera el marcador de arranque en el puerto serie, mostrando cada línea.
fn boot_and_wait(mut cmd: Command) -> Result<Child> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("no pude abrir QEMU ({}): {e}", qemu_binary().display()))?;
    let stdout = child.stdout.take().ok_or("QEMU sin stdout")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + BOOT_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                println!("[serie] {line}");
                if line.contains(BOOT_MARKER) {
                    return Ok(child);
                }
                if line.contains("PANIC") {
                    let _ = child.kill();
                    return Err("el kernel entró en panic".into());
                }
            }
            Err(_) => {
                let _ = child.kill();
                return Err(format!(
                    "no llegó {BOOT_MARKER} en {} s",
                    BOOT_TIMEOUT.as_secs()
                ));
            }
        }
    }
}

fn test(image: &Path) -> Result<()> {
    let mut child = boot_and_wait(qemu(image, true)?)?;
    let _ = child.kill();
    let _ = child.wait();
    println!("ok: JARVIS-OS arrancó y dibujó el HUD");
    Ok(())
}

fn screenshot(image: &Path) -> Result<()> {
    // Monitor de QEMU por TCP en un puerto libre: desde ahí se pide la captura.
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map_err(|e| format!("sin puerto libre: {e}"))?
        .port();
    let mut cmd = qemu(image, true)?;
    cmd.arg("-monitor")
        .arg(format!("tcp:127.0.0.1:{port},server,nowait"));
    let mut child = boot_and_wait(cmd)?;

    let ppm = target_dir().join("screen.ppm");
    let _ = fs::remove_file(&ppm);
    let result = (|| -> Result<()> {
        let mut mon =
            TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("monitor: {e}"))?;
        mon.set_read_timeout(Some(Duration::from_millis(500))).ok();
        thread::sleep(Duration::from_millis(500)); // que termine de pintar
        writeln!(
            mon,
            "screendump {}",
            ppm.display().to_string().replace('\\', "/")
        )
        .map_err(|e| format!("monitor: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sink = [0u8; 4096];
        while !ppm.exists() || fs::metadata(&ppm).map(|m| m.len()).unwrap_or(0) == 0 {
            let _ = mon.read(&mut sink);
            if Instant::now() > deadline {
                return Err("QEMU no generó la captura".into());
            }
        }
        thread::sleep(Duration::from_millis(300));
        let _ = writeln!(mon, "quit");
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result?;

    let png = target_dir().join("jarvis-os.png");
    ppm_to_png(&ppm, &png)?;
    println!("captura: {}", png.display());
    Ok(())
}

/// Convierte el PPM binario (P6) que genera QEMU a PNG.
fn ppm_to_png(ppm: &Path, png_path: &Path) -> Result<()> {
    let data = fs::read(ppm).map_err(|e| format!("{}: {e}", ppm.display()))?;
    // Cabecera: "P6\n<ancho> <alto>\n<max>\n" y después los bytes RGB.
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if start == pos {
            return Err("PPM truncado".into());
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1;
    if fields[0] != "P6" {
        return Err(format!("formato PPM inesperado: {}", fields[0]));
    }
    let w: u32 = fields[1].parse().map_err(|_| "ancho inválido")?;
    let h: u32 = fields[2].parse().map_err(|_| "alto inválido")?;
    let pixels = data
        .get(pos..pos + (w * h * 3) as usize)
        .ok_or("PPM incompleto")?;

    let file = fs::File::create(png_path).map_err(|e| format!("{}: {e}", png_path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())?;
    Ok(())
}

// --- VirtualBox -------------------------------------------------------------------------------

fn vdi(image: &Path) -> Result<()> {
    let vbox = find_first(
        "VBOXMANAGE",
        &[r"C:\Program Files\Oracle\VirtualBox\VBoxManage.exe"],
    )
    .unwrap_or_else(|| "VBoxManage".into());
    let out = target_dir().join("jarvis-os.vdi");
    let _ = fs::remove_file(&out);
    let status = Command::new(&vbox)
        .arg("convertfromraw")
        .arg(image)
        .arg(&out)
        .args(["--format", "VDI"])
        .status()
        .map_err(|e| format!("no pude ejecutar VBoxManage: {e}"))?;
    if !status.success() {
        return Err("VBoxManage convertfromraw falló".into());
    }
    println!(
        "disco para VirtualBox: {} (creá una VM con EFI activado)",
        out.display()
    );
    Ok(())
}
