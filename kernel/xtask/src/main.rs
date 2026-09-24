//! `cargo xtask <comando>`: arma la imagen booteable de JARVIS-OS y la corre en QEMU.
//!
//! - `build`       compila el kernel y crea `target/jarvis-os-uefi.img`
//! - `run`         abre QEMU con ventana
//! - `test`        arranca sin ventana, espera `JARVIS_BOOT_OK`, aprieta Espacio y espera `JARVIS_HABLA` (CI)
//! - `screenshot`  capturas en reposo y hablando: `target/jarvis-os.png` y `jarvis-os-hablando.png`
//! - `vdi`         convierte la imagen a `target/jarvis-os.vdi` para VirtualBox
//! - `disk`        crea el disco virtual `target/disco.img` si no existe (`--reset` lo regenera)

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
        "run" => build().and_then(|img| run(&img, &disk_image(false)?)),
        "test" => build().and_then(|img| test(&img, &fresh_disk("disco-test.img")?)),
        "screenshot" => build().and_then(|img| screenshot(&img, &fresh_disk("disco-captura.img")?)),
        "vdi" => build().and_then(|img| vdi(&img)),
        "disk" => {
            let reset = env::args().any(|a| a == "--reset");
            disk_image(reset).map(|d| println!("disco: {}", d.display()))
        }
        _ => Err("uso: cargo xtask <build|run|test|screenshot|vdi|disk [--reset]>".into()),
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

/// Carpeta de salida: respeta `CARGO_TARGET_DIR` (útil para compilar mientras otra instancia de
/// QEMU tiene abierta la imagen de `target/`).
fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(|d| workspace_root().join(d))
        .unwrap_or_else(|| workspace_root().join("target"))
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

fn qemu(image: &Path, disk: &Path, headless: bool) -> Result<Command> {
    let (code, vars) = ovmf()?;
    let mut cmd = Command::new(qemu_binary());
    if cfg!(windows) {
        // Aceleración por hardware de Windows (Hyper-V): ~9 veces más rápido que emular. Si no
        // está disponible, QEMU sigue con el siguiente acelerador (tcg: emulación por software).
        cmd.args(["-accel", "whpx,kernel-irqchip=off", "-accel", "tcg"]);
    }
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
    // Disco de datos: virtio-blk con la interfaz legacy (por puertos de E/S), que es la que
    // implementa el driver del kernel.
    cmd.arg("-drive")
        .arg(format!(
            "if=none,id=disco,format=raw,file={}",
            disk.display()
        ))
        .args(["-device", "virtio-blk-pci,drive=disco,disable-modern=on"]);
    if headless {
        cmd.args(["-display", "none"]);
    }
    Ok(cmd)
}

fn run(image: &Path, disk: &Path) -> Result<()> {
    let status = qemu(image, disk, false)?
        .status()
        .map_err(|e| format!("no pude abrir QEMU ({}): {e}", qemu_binary().display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("QEMU terminó con {status}"))
    }
}

/// Una ejecución de QEMU sin ventana, con acceso al puerto serie (lo que imprime el kernel) y al
/// monitor de QEMU (para apretar teclas y sacar capturas). Al soltarla, QEMU se cierra.
struct Session {
    child: Child,
    lines: mpsc::Receiver<String>,
    monitor: TcpStream,
}

impl Session {
    fn start(image: &Path, disk: &Path) -> Result<Session> {
        // Monitor de QEMU por TCP en un puerto libre.
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("sin puerto libre: {e}"))?
            .port();
        let mut cmd = qemu(image, disk, true)?;
        cmd.arg("-monitor")
            .arg(format!("tcp:127.0.0.1:{port},server,nowait"));
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("no pude abrir QEMU ({}): {e}", qemu_binary().display()))?;
        let stdout = child.stdout.take().ok_or("QEMU sin stdout")?;
        let (tx, lines) = mpsc::channel();
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
        let deadline = Instant::now() + Duration::from_secs(10);
        let monitor = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(m) => break m,
                Err(e) if Instant::now() > deadline => {
                    let _ = child.kill();
                    return Err(format!("no pude conectarme al monitor de QEMU: {e}"));
                }
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        };
        monitor
            .set_read_timeout(Some(Duration::from_millis(200)))
            .ok();
        Ok(Session {
            child,
            lines,
            monitor,
        })
    }

    /// Espera una línea del puerto serie que contenga `marker`, mostrando todo lo que llega.
    fn wait_for(&mut self, marker: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    println!("[serie] {line}");
                    if line.contains(marker) {
                        return Ok(());
                    }
                    if line.contains("PANIC") {
                        return Err("el kernel entró en panic".into());
                    }
                }
                Err(_) => return Err(format!("no llegó {marker} en {} s", timeout.as_secs())),
            }
        }
    }

    fn monitor(&mut self, command: &str) -> Result<()> {
        writeln!(self.monitor, "{command}").map_err(|e| format!("monitor: {e}"))?;
        // Vaciar lo que responde el monitor, para que no se llene su buffer.
        let mut sink = [0u8; 4096];
        let _ = self.monitor.read(&mut sink);
        Ok(())
    }

    /// Captura la pantalla de QEMU y la guarda como PNG.
    fn screenshot(&mut self, png: &Path) -> Result<()> {
        let ppm = png.with_extension("ppm");
        let _ = fs::remove_file(&ppm);
        self.monitor(&format!(
            "screendump {}",
            ppm.display().to_string().replace('\\', "/")
        ))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while fs::metadata(&ppm).map(|m| m.len()).unwrap_or(0) == 0 {
            if Instant::now() > deadline {
                return Err("QEMU no generó la captura".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_millis(300)); // que termine de escribir el archivo
        ppm_to_png(&ppm, png)?;
        let _ = fs::remove_file(&ppm);
        println!("captura: {}", png.display());
        Ok(())
    }
}

impl Session {
    /// Tipea `text` tecla por tecla (letras minúsculas, dígitos y guiones).
    fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            let key = if c == '-' {
                "minus".to_string()
            } else {
                c.to_string()
            };
            self.monitor(&format!("sendkey {key}"))?;
        }
        Ok(())
    }

    /// Cierra QEMU de forma ordenada (el disco queda escrito).
    fn quit(&mut self) {
        let _ = writeln!(self.monitor, "quit");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const STEP: Duration = Duration::from_secs(20);

/// Prueba de punta a punta, como la usaría una persona:
/// 1. arranca y dibuja el HUD; Espacio → JARVIS habla (IRQ1 → asistente);
/// 2. Tab → se abre Archivos y el kernel lee la raíz del disco (virtio-blk + FAT32);
/// 3. F7, "prueba", Enter → se crea una carpeta (escritura en el disco);
/// 4. el mouse se mueve y hace clic en una fila (IRQ12 → paquetes → selección);
/// 5. se cierra QEMU y `fatfs` verifica en el archivo del disco que `/prueba` quedó escrita.
fn test(image: &Path, disk: &Path) -> Result<()> {
    let mut s = Session::start(image, disk)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.monitor("sendkey spc")?;
    s.wait_for("JARVIS_HABLA", STEP)?;
    s.monitor("sendkey tab")?;
    s.wait_for("ARCHIVOS_ABIERTO /", STEP)?;
    s.monitor("sendkey f7")?;
    s.type_text("prueba")?;
    s.monitor("sendkey ret")?;
    s.wait_for("ARCHIVOS_CREADO /prueba", STEP)?;
    // El cursor arranca en el centro (640, 400); la fila 2 de la lista está en y ≈ 350.
    s.monitor("mouse_move -40 -50")?;
    thread::sleep(Duration::from_millis(200));
    s.monitor("mouse_button 1")?;
    s.monitor("mouse_button 0")?;
    s.wait_for("ARCHIVOS_SELECCION", STEP)?;
    s.quit();
    drop(s);
    verify_dir_on_disk(disk, "prueba")?;
    println!("ok: arranque, teclado, mouse, Archivos y escritura en disco verificados");
    Ok(())
}

/// Abre el disco con `fatfs` (independiente de nuestro FAT32) y verifica que exista la carpeta.
fn verify_dir_on_disk(disk: &Path, dir: &str) -> Result<()> {
    let io = |e: std::io::Error| format!("disco {}: {e}", disk.display());
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(disk)
        .map_err(io)?;
    let fs = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(io)?;
    fs.root_dir()
        .open_dir(dir)
        .map_err(|e| format!("la carpeta /{dir} no quedó en el disco: {e}"))?;
    println!("[disco] /{dir} existe (verificado con fatfs)");
    Ok(())
}

/// Capturas: JARVIS en reposo y hablando, el gestor de archivos y un diálogo.
fn screenshot(image: &Path, disk: &Path) -> Result<()> {
    let mut s = Session::start(image, disk)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.wait_for("JARVIS_REPOSO", Duration::from_secs(30))?;
    thread::sleep(Duration::from_millis(600)); // que termine de apagarse el brillo
    s.screenshot(&target_dir().join("jarvis-os.png"))?;
    s.monitor("sendkey spc")?;
    s.wait_for("JARVIS_HABLA", STEP)?;
    thread::sleep(Duration::from_millis(1200)); // a mitad de la frase
    s.screenshot(&target_dir().join("jarvis-os-hablando.png"))?;

    // Archivos: /Documentos con "Bienvenida.txt" seleccionado (vista previa en el inspector).
    s.monitor("sendkey tab")?;
    s.wait_for("ARCHIVOS_ABIERTO /", STEP)?;
    s.monitor("sendkey ret")?;
    s.wait_for("ARCHIVOS_ABIERTO /Documentos", STEP)?;
    s.monitor("sendkey down")?;
    s.wait_for("ARCHIVOS_SELECCION Bienvenida.txt", STEP)?;
    s.monitor("mouse_move 150 80")?; // que se vea el cursor
    thread::sleep(Duration::from_millis(500));
    s.screenshot(&target_dir().join("jarvis-os-archivos.png"))?;

    s.monitor("sendkey f7")?;
    s.type_text("tareas")?;
    thread::sleep(Duration::from_millis(500));
    s.screenshot(&target_dir().join("jarvis-os-dialogo.png"))
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

// --- disco virtual ---------------------------------------------------------------------------

const DISK_MIB: u64 = 64;
/// Carpetas que se crean aunque estén vacías (git no guarda carpetas vacías).
const EMPTY_DIRS: [&str; 2] = ["Papelera", "Facultad/Algoritmos"];

/// El disco persistente de `run`. Si ya existe no se toca: así lo que hagas queda guardado.
fn disk_image(reset: bool) -> Result<PathBuf> {
    let path = target_dir().join("disco.img");
    if path.exists() && !reset {
        return Ok(path);
    }
    create_disk(&path)?;
    println!("disco nuevo: {}", path.display());
    Ok(path)
}

/// Un disco recién creado (tests y capturas: siempre parten del mismo estado).
fn fresh_disk(name: &str) -> Result<PathBuf> {
    let path = target_dir().join(name);
    create_disk(&path)?;
    Ok(path)
}

/// Crea un disco FAT32 de 64 MiB con `fatfs` y le copia `kernel/rootfs/`.
fn create_disk(path: &Path) -> Result<()> {
    let io = |e: std::io::Error| format!("disco {}: {e}", path.display());
    fs::create_dir_all(target_dir()).map_err(io)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(io)?;
    file.set_len(DISK_MIB * 1024 * 1024).map_err(io)?;
    let opts = fatfs::FormatVolumeOptions::new()
        .fat_type(fatfs::FatType::Fat32)
        .bytes_per_cluster(512)
        .volume_label(*b"JARVIS     ");
    fatfs::format_volume(&mut file, opts).map_err(io)?;
    let fs = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(io)?;
    copy_tree(&fs.root_dir(), &workspace_root().join("rootfs"))?;
    for dir in EMPTY_DIRS {
        let mut current = fs.root_dir();
        for part in dir.split('/') {
            current = current.create_dir(part).map_err(io)?;
        }
    }
    fs.unmount().map_err(io)
}

fn copy_tree<T: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<'_, T>, src: &Path) -> Result<()> {
    let io = |e: std::io::Error| format!("{}: {e}", src.display());
    let mut entries: Vec<_> = fs::read_dir(src)
        .map_err(io)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&dir.create_dir(&name).map_err(io)?, &path)?;
        } else {
            let data = fs::read(&path).map_err(io)?;
            let mut file = dir.create_file(&name).map_err(io)?;
            file.truncate().map_err(io)?;
            file.write_all(&data).map_err(io)?;
        }
    }
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
