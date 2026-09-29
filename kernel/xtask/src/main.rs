//! `cargo xtask <comando>`: arma la imagen booteable de JARVIS-OS y la corre en QEMU.
//!
//! - `build`       compila el kernel y crea `target/jarvis-os-uefi.img`
//! - `run`         abre QEMU con ventana, el disco persistente `target/disco.img`, red, sonido y
//!   el puente HTTPS del navegador (ver `puente.rs`)
//! - `test`        de punta a punta sin ventana: teclado, mouse, ventanas, disco y red (CI)
//! - `screenshot`  capturas del escritorio, las apps y los menús (en `target/`)
//! - `vdi`         convierte la imagen a `target/jarvis-os.vdi` para VirtualBox
//! - `disk`        crea el disco virtual `target/disco.img` si no existe (`--reset` lo regenera)
//! - `brave`       el puente de Brave solo (`--instalar` lo instala con winget, `--red --token X`
//!   lo abre a la red local, `--probar URL` hace de kernel y guarda la página en un PNG)

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

mod brave;
mod cerebro;
mod iso;
mod puente;
mod sincro;

type Result<T> = std::result::Result<T, String>;

const BOOT_MARKER: &str = "JARVIS_BOOT_OK";
const BOOT_TIMEOUT: Duration = Duration::from_secs(90);

fn main() -> ExitCode {
    let cmd = env::args().nth(1).unwrap_or_default();
    let result = match cmd.as_str() {
        "build" => build().map(|img| println!("imagen: {}", img.display())),
        "run" => build_user()
            .and_then(|_| build())
            .and_then(|img| run(&img, &disk_image(false)?)),
        "test" => build_user()
            .and_then(|_| build())
            .and_then(|img| test(&img, &fresh_disk("disco-test.img")?)),
        "usuario" => build_user().map(|d| println!("programas de Linux: {}", d.display())),
        "screenshot" => build().and_then(|img| screenshot(&img, &fresh_disk("disco-captura.img")?)),
        "vdi" => build().and_then(|img| vdi(&img)),
        "disk" => {
            let reset = env::args().any(|a| a == "--reset");
            disk_image(reset).map(|d| println!("disco: {}", d.display()))
        }
        "brave" => brave_cmd(),
        "pantallas" => build().and_then(|img| screens(&img, &fresh_disk("disco-pantallas.img")?)),
        "relay" => sincro::relay_cmd(),
        "iso" => build().and_then(|img| iso_cmd(&img)),
        "run2" => build().and_then(|img| sincro::run2(&img)),
        "sincronizar" => build().and_then(|img| sincro::e2e(&img)),
        _ => Err(
            "uso: cargo xtask <build|run|test|usuario|screenshot|pantallas|relay [--publico] [puerto]|run2|sincronizar|iso [--probar|--abrir]|vdi|disk [--reset]|brave [--instalar|--red --token X|--probar URL]>"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn brave_profile() -> PathBuf {
    target_dir().join("brave-perfil")
}

fn arg_after(flag: &str) -> Option<String> {
    let args: Vec<String> = env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn brave_cmd() -> Result<()> {
    if env::args().any(|a| a == "--instalar") {
        return brave::install();
    }
    if let Some(url) = arg_after("--probar") {
        brave::start(brave_profile());
        let png = target_dir().join("brave-prueba.png");
        return brave_probe(&url, &png);
    }
    let lan = env::args().any(|a| a == "--red");
    brave::standalone(
        brave_profile(),
        lan,
        arg_after("--token").unwrap_or_default(),
    )
}

/// Hace de kernel: se conecta al puente de Brave, abre `url`, arma la imagen con los mosaicos
/// que llegan y la guarda en `png` cuando la página deja de cargar.
fn brave_probe(url: &str, png: &Path) -> Result<()> {
    use jarvis_desktop::remote::{self, FromBrave, ToBrave};
    let (w, h) = (1024u16, 700u16);
    let mut s = TcpStream::connect(("127.0.0.1", remote::PORT)).map_err(|e| e.to_string())?;
    let hello = ToBrave::Hello {
        token: String::new(),
        w,
        h,
    };
    s.write_all(&hello.encode()).map_err(|e| e.to_string())?;
    s.write_all(&ToBrave::Navigate(url.into()).encode())
        .map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let mut framer = remote::Framer::default();
    let mut rgb = vec![0u8; w as usize * h as usize * 3];
    let (mut fw, mut fh) = (w as usize, h as usize);
    let start = Instant::now();
    let (mut frames, mut tiles_total, mut bytes) = (0, 0, 0usize);
    let mut last_frame = Instant::now();
    let mut loaded = false;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match s.read(&mut buf) {
            Ok(0) => return Err("el puente cerró".into()),
            Ok(n) => {
                bytes += n;
                framer.push(&buf[..n]);
            }
            Err(_) => {}
        }
        while let Some(m) = framer.next_message() {
            match FromBrave::decode(&m?).ok_or("mensaje inválido")? {
                FromBrave::Frame {
                    w: nw,
                    h: nh,
                    tiles,
                } => {
                    if (nw as usize, nh as usize) != (fw, fh) {
                        (fw, fh) = (nw as usize, nh as usize);
                        rgb = vec![0; fw * fh * 3];
                    }
                    for t in &tiles {
                        let px = remote::tile_pixels(t).ok_or("mosaico roto")?;
                        for row in 0..t.h as usize {
                            let dst = ((t.y as usize + row) * fw + t.x as usize) * 3;
                            rgb[dst..dst + t.w as usize * 3]
                                .copy_from_slice(&px[row * t.w as usize * 3..][..t.w as usize * 3]);
                        }
                    }
                    frames += 1;
                    tiles_total += tiles.len();
                    last_frame = Instant::now();
                    s.write_all(&ToBrave::FrameAck.encode())
                        .map_err(|e| e.to_string())?;
                }
                FromBrave::State(st) => {
                    let tab = st.tabs.get(st.active as usize);
                    println!(
                        "[probar] estado: {} pestaña(s), {:?}, cargando={}",
                        st.tabs.len(),
                        tab.map(|t| (&t.title, &t.url)),
                        st.loading
                    );
                    if !st.loading && tab.is_some_and(|t| t.url.starts_with("http")) {
                        loaded = true;
                    }
                }
                FromBrave::Error(e) => return Err(e),
            }
        }
        let quiet = last_frame.elapsed() > Duration::from_millis(1500);
        if (loaded && frames > 0 && quiet) || start.elapsed() > Duration::from_secs(40) {
            break;
        }
    }
    // Cortar y darle tiempo al puente para cerrar Brave (si no, queda con el perfil tomado).
    let _ = s.shutdown(std::net::Shutdown::Both);
    thread::sleep(Duration::from_secs(4));
    let file = fs::File::create(png).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), fw as u32, fh as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgb))
        .map_err(|e| e.to_string())?;
    println!(
        "[probar] {frames} cuadros, {tiles_total} mosaicos, {} KiB recibidos en {:.1} s -> {}",
        bytes / 1024,
        start.elapsed().as_secs_f32(),
        png.display()
    );
    Ok(())
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

/// Los programas de Linux de `kernel/usuario/` (K11): se compilan para musl (estáticos) y quedan
/// en `target/usuario/`, de donde los sirve el puente (`http://paquetes.jarvis/usuario/`).
const USER_PROGRAMS: [&str; 5] = ["hola-linux", "eco", "pruebas", "red", "js"];

fn build_user() -> Result<PathBuf> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build_dir = target_dir().join("usuario-build");
    let status = Command::new(cargo)
        .current_dir(workspace_root().join("usuario"))
        // El `cargo xtask` de afuera deja variables de su compilación que no son para esta.
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .arg("build")
        .arg("--release")
        .arg("--target-dir")
        .arg(&build_dir)
        .status()
        .map_err(|e| format!("no pude ejecutar cargo: {e}"))?;
    if !status.success() {
        return Err("falló la compilación de los programas de usuario (kernel/usuario)".into());
    }
    let out = target_dir().join("usuario");
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for p in USER_PROGRAMS {
        let from = build_dir.join("x86_64-unknown-linux-musl/release").join(p);
        fs::copy(&from, out.join(p)).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(out)
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
            // Una copia por QEMU abierto (run2 y la prueba de sincronización abren dos a la vez, y
            // QEMU bloquea el archivo).
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dst = target_dir().join(if n == 0 {
                "ovmf-vars.fd".to_string()
            } else {
                format!("ovmf-vars-{}.fd", n + 1)
            });
            fs::copy(&src, &dst).map_err(|e| format!("no pude copiar {}: {e}", src.display()))?;
            Some(dst)
        }
        None => None,
    };
    Ok((code, vars))
}

fn qemu(image: &Path, disk: &Path, headless: bool) -> Result<Command> {
    // Sonido (el parlante de la PC): solo con ventana. En Windows, DirectSound; en otros
    // sistemas, el que diga QEMU_AUDIO (por ejemplo "pa" o "alsa"), o ninguno.
    // Sin ventana: un audio vacío (silencio), así el micrófono virtual existe igual en las pruebas.
    let audio = if headless {
        Some("none".to_string())
    } else if let Ok(driver) = env::var("QEMU_AUDIO") {
        Some(driver)
    } else if cfg!(windows) {
        Some("dsound".to_string())
    } else {
        None
    };
    let (code, vars) = ovmf()?;
    let mut cmd = Command::new(qemu_binary());
    if cfg!(windows) {
        // Aceleración por hardware de Windows (Hyper-V): ~9 veces más rápido que emular. Si no
        // está disponible, QEMU sigue con el siguiente acelerador (tcg: emulación por software).
        cmd.args(["-accel", "whpx,kernel-irqchip=off", "-accel", "tcg"]);
    }
    let machine = match &audio {
        Some(driver) => {
            cmd.arg("-audiodev").arg(format!("{driver},id=sonido"));
            // Micrófono (y parlantes) de JARVIS-OS: virtio-sound, conectado al audio del
            // anfitrión (ver kernel/src/virtio_sound.rs).
            cmd.args(["-device", "virtio-sound-pci,audiodev=sonido"]);
            "q35,pcspk-audiodev=sonido"
        }
        None => "q35",
    };
    cmd.args([
        "-machine", machine, "-m", "2G", "-rtc", "base=utc", "-serial", "stdio",
    ]);
    if headless {
        // En los tests, un reinicio es un error (triple fault): mejor que QEMU termine.
        cmd.arg("-no-reboot");
    }
    cmd.arg("-drive").arg(format!(
        "if=pflash,format=raw,readonly=on,file={}",
        code.display()
    ));
    if let Some(vars) = vars {
        cmd.arg("-drive")
            .arg(format!("if=pflash,format=raw,file={}", vars.display()));
    }
    // El cerebro: su puerto y el token de esta sesión (ver cerebro.rs).
    if let Some(v) = cerebro::fw_cfg() {
        cmd.arg("-fw_cfg")
            .arg(format!("name=opt/jarvis/cerebro,string={v}"));
    }
    if image.extension().is_some_and(|e| e == "iso") {
        // Desde la ISO, como un CD y sin disco: modo en vivo (el disco se ignora).
        cmd.arg("-cdrom").arg(image);
    } else {
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
    }
    // Placa de video: virtio-vga (una virtio-gpu que arranca como VGA común, así el firmware
    // tiene dónde dibujar) con una salida por monitor del anfitrión. JARVIS_MONITORES la cambia.
    // Con ventana, cada salida arranca del tamaño del monitor del anfitrión (sin esto, QEMU
    // informa el de su ventana al arrancar: 640×480). JARVIS_RESOLUCION=1920x1080 lo cambia.
    let monitors = monitor_count();
    let mut video = format!("virtio-vga,id=video,max_outputs={monitors}");
    // Sin ventana solo si se pide con JARVIS_RESOLUCION (así los tests siguen en 1280×800).
    let forced = env::var("JARVIS_RESOLUCION").is_ok();
    if (!headless || forced)
        && let Some((w, h)) = host_resolution()
    {
        video.push_str(&format!(",xres={w},yres={h}"));
        // GTK igual informa el tamaño de su ventana; el kernel lee esta (ver kernel/src/fw_cfg.rs).
        cmd.arg("-fw_cfg")
            .arg(format!("name=opt/jarvis/resolucion,string={w}x{h}"));
        if !headless {
            // La ventana se ajusta a la pantalla (Ctrl+Alt+F: pantalla completa).
            cmd.args(["-display", "gtk,zoom-to-fit=on"]);
        }
    }
    cmd.args(["-vga", "none", "-device", &video]);
    // Placa de red virtio-net con la red "user" de QEMU: DHCP (10.0.2.15), DNS (10.0.2.3) y
    // salida a internet por el anfitrión, que se ve como 10.0.2.2.
    cmd.args([
        "-netdev",
        "user,id=red",
        "-device",
        "virtio-net-pci,netdev=red,disable-modern=on",
    ]);
    if headless {
        cmd.args(["-display", "none"]);
    }
    Ok(cmd)
}

/// Cantidad de monitores forzada por un comando (`pantallas`); 0 = la de siempre.
static MONITORS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Cuántos monitores tiene el anfitrión (JARVIS_MONITORES manda; si no, se preguntan a Windows).
/// La resolución del monitor del anfitrión, o `JARVIS_RESOLUCION` (`ANCHOxALTO`).
fn host_resolution() -> Option<(u32, u32)> {
    let parse = |v: &str| {
        let (w, h) = v.trim().split_once(['x', 'X'])?;
        let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
        (640..=7680).contains(&w).then_some(())?;
        (480..=4320).contains(&h).then_some((w, h))
    };
    if let Ok(v) = env::var("JARVIS_RESOLUCION") {
        return parse(&v);
    }
    if !cfg!(windows) {
        return None;
    }
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            // La resolución real (la de Forms viene dividida por la escala de Windows).
            "$v = Get-CimInstance Win32_VideoController | ? CurrentHorizontalResolution | select -First 1; \"$($v.CurrentHorizontalResolution)x$($v.CurrentVerticalResolution)\"",
        ])
        .output()
        .ok()?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

fn monitor_count() -> u32 {
    let forced = MONITORS.load(std::sync::atomic::Ordering::Relaxed);
    if forced > 0 {
        return forced;
    }
    if let Some(n) = env::var("JARVIS_MONITORES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        return n.clamp(1, 4);
    }
    if !cfg!(windows) {
        return 1;
    }
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::AllScreens.Count",
        ])
        .output();
    out.ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(1)
        .clamp(1, 4)
}

/// `cargo xtask iso`: arma la ISO; `--probar` la arranca sin ventana y espera el escritorio en
/// modo en vivo; `--abrir` la abre con ventana.
fn iso_cmd(image: &Path) -> Result<()> {
    let iso = iso::build(image)?;
    println!("iso: {}", iso.display());
    if env::args().any(|a| a == "--probar") {
        boot_iso(&iso)?;
    } else if env::args().any(|a| a == "--abrir") {
        let status = qemu(&iso, Path::new(""), false)?
            .status()
            .map_err(|e| format!("no pude abrir QEMU: {e}"))?;
        if !status.success() {
            return Err(format!("QEMU terminó con {status}"));
        }
    }
    Ok(())
}

/// Arranca la ISO como un CD, sin disco, y espera el escritorio en modo en vivo.
fn boot_iso(iso: &Path) -> Result<()> {
    let mut s = Session::start(iso, Path::new(""))?;
    s.wait_for("segmentos con W^X", BOOT_TIMEOUT)?;
    s.wait_for("MODO_EN_VIVO", BOOT_TIMEOUT)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.quit();
    println!("ok: la ISO arranca (modo en vivo, FAT32 en RAM)");
    Ok(())
}

fn run(image: &Path, disk: &Path) -> Result<()> {
    puente::start();
    brave::start(brave_profile());
    // El cerebro con Claude (se cierra al terminar).
    let _brain = cerebro::start(cerebro::PORT, false);
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
    /// Todo lo que llegó por el puerto serie (para `saw_or_wait`).
    history: Vec<String>,
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
            history: Vec::new(),
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
                    let found = line.contains(marker);
                    let panic = line.contains("PANIC");
                    self.history.push(line);
                    if found {
                        return Ok(());
                    }
                    if panic {
                        return Err("el kernel entró en panic".into());
                    }
                }
                Err(_) => return Err(format!("no llegó {marker} en {} s", timeout.as_secs())),
            }
        }
    }

    /// Como `wait_for`, pero la línea puede haber llegado antes (desde K9 hay tareas que
    /// corren a la vez: la red puede tener IP antes de que el escritorio dibuje su primer cuadro).
    fn saw_or_wait(&mut self, marker: &str, timeout: Duration) -> Result<()> {
        if self.history.iter().any(|l| l.contains(marker)) {
            return Ok(());
        }
        self.wait_for(marker, timeout)
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
        self.screenshot_head(png, None)
    }

    /// Captura una salida (un monitor) de la placa de video.
    fn screenshot_head(&mut self, png: &Path, head: Option<u32>) -> Result<()> {
        let ppm = png.with_extension("ppm");
        let _ = fs::remove_file(&ppm);
        // `screendump archivo [dispositivo [salida]]` (el dispositivo es la placa, id "video").
        let target = match head {
            Some(h) => format!(" video {h}"),
            None => String::new(),
        };
        let command = format!(
            "screendump {}{target}",
            ppm.display().to_string().replace('\\', "/")
        );
        writeln!(self.monitor, "{command}").map_err(|e| format!("monitor: {e}"))?;
        // Lo que contesta el monitor (si falla, dice por qué).
        let mut reply = Vec::new();
        let mut buf = [0u8; 4096];
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            if let Ok(n) = self.monitor.read(&mut buf) {
                reply.extend_from_slice(&buf[..n]);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while fs::metadata(&ppm).map(|m| m.len()).unwrap_or(0) == 0 {
            if Instant::now() > deadline {
                // Sin los códigos de terminal del monitor; lo último que dijo.
                let text: String = String::from_utf8_lossy(&reply)
                    .split('\u{1b}')
                    .map(|p| p.trim_start_matches(|c: char| "[0123456789;KDC".contains(c)))
                    .collect();
                let tail: String = text.lines().rev().take(4).collect::<Vec<_>>().join(" | ");
                return Err(format!("QEMU no generó la captura ({command}): {tail}"));
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
    /// Tipea `text` tecla por tecla (letras, dígitos, espacios y `- _ . : /`).
    fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            let key = match c {
                '-' => "minus".to_string(),
                '.' => "dot".to_string(),
                '/' => "slash".to_string(),
                ':' => "shift-semicolon".to_string(),
                ' ' => "spc".to_string(),
                '_' => "shift-minus".to_string(),
                '>' => "shift-dot".to_string(),
                '|' => "shift-backslash".to_string(),
                '"' => "shift-apostrophe".to_string(),
                '=' => "equal".to_string(),
                '?' => "shift-slash".to_string(),
                '&' => "shift-7".to_string(),
                '+' => "shift-equal".to_string(),
                '*' => "shift-8".to_string(),
                '(' => "shift-9".to_string(),
                ')' => "shift-0".to_string(),
                '!' => "shift-1".to_string(),
                '@' => "shift-2".to_string(),
                '#' => "shift-3".to_string(),
                '$' => "shift-4".to_string(),
                '%' => "shift-5".to_string(),
                ',' => "comma".to_string(),
                ';' => "semicolon".to_string(),
                '\'' => "apostrophe".to_string(),
                '<' => "shift-comma".to_string(),
                c if c.is_ascii_uppercase() => format!("shift-{}", c.to_ascii_lowercase()),
                c => c.to_string(),
            };
            self.monitor(&format!("sendkey {key}"))?;
            thread::sleep(Duration::from_millis(20));
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

/// Página que sirve el anfitrión en los tests y las capturas del navegador.
const TEST_PAGE: &str = "<!DOCTYPE html><html><head><title>Red de JARVIS-OS</title></head><body>\
<h1>La red de JARVIS-OS funciona</h1>\
<p>Esta página viajó desde la computadora anfitriona hasta el kernel por la placa de red \
<b>virtio-net</b>, la pila <b>TCP/IP</b> (smoltcp) y el cliente <b>HTTP</b> propio.</p>\
<h2>Qué pasó para que la veas</h2><ol>\
<li>El kernel pidió una dirección IP por DHCP (10.0.2.15).</li>\
<li>Abrió una conexión TCP con el anfitrión (10.0.2.2).</li>\
<li>Mandó <b>GET /</b> y juntó la respuesta.</li>\
<li>El navegador convirtió el HTML en texto con títulos, listas y enlaces.</li></ol>\
<h2>Enlaces</h2><ul><li><a href=\"http://example.com/\">example.com</a></li>\
<li><a href=\"https://es.wikipedia.org/wiki/Kernel\">Wikipedia: Kernel</a></li></ul>\
<blockquote>Todo esto corre sobre un kernel escrito desde cero en Rust.</blockquote>\
</body></html>";

/// Prueba de punta a punta, como la usaría una persona:
/// 1. arranca, dibuja el HUD y consigue una IP por DHCP (virtio-net + smoltcp);
/// 2. Espacio → JARVIS habla (IRQ1 → asistente);
/// 3. Tab → Archivos lee la raíz del disco; F7, "prueba", Enter → crea una carpeta;
/// 4. el mouse hace clic en una fila (IRQ12 → paquetes → selección);
/// 5. Win+R → Consola; "ir http://10.0.2.2:PUERTO/" → el navegador descarga la página que
///    sirve este mismo xtask (DNS no: es una IP; TCP y HTTP sí);
/// 6. Alt+Tab cambia de ventana; Win+D muestra el escritorio; Impr Pant guarda una captura;
/// 7. se cierra QEMU y `fatfs` verifica en el disco que `/prueba` y la captura quedaron escritas.
fn test(image: &Path, disk: &Path) -> Result<()> {
    let port = puente::test_server(TEST_PAGE)?;
    // El puente sirve el repositorio de paquetes (apt).
    puente::start();
    brave::start(brave_profile());
    // El cerebro simulado (sin Claude): respuestas fijas.
    let brain = cerebro::start(cerebro::TEST_PORT, true);
    let mut s = Session::start(image, disk)?;
    // K8: el kernel cambió a sus propias tablas de páginas (con W^X) y siguió andando.
    s.wait_for("segmentos con W^X", BOOT_TIMEOUT)?;
    // K10: hubo entropía suficiente para generar claves.
    s.wait_for("ENTROPIA_LISTA", BOOT_TIMEOUT)?;
    // K10: el cliente TLS arma las claves efímeras y el ClientHello dentro del kernel.
    s.wait_for("TLS_LISTO", BOOT_TIMEOUT)?;
    // K9: el escritorio, la red y la ociosa son tareas aparte.
    s.wait_for("MULTITAREA 3 tareas", BOOT_TIMEOUT)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.saw_or_wait("RED_IP 10.0.2.15", STEP)?;
    // La tarea de la red se despierta con la interrupción de la placa, no dando vueltas.
    s.saw_or_wait("RED_POR_INTERRUPCION", STEP)?;
    if brain.is_some() {
        s.wait_for("CEREBRO_CONECTADO", STEP)?;
    }
    s.monitor("sendkey spc")?;
    s.wait_for("JARVIS_HABLA", STEP)?;
    s.monitor("sendkey tab")?;
    s.wait_for("ARCHIVOS_ABIERTO /", STEP)?;
    s.monitor("sendkey f7")?;
    s.type_text("prueba")?;
    s.monitor("sendkey ret")?;
    s.wait_for("ARCHIVOS_CREADO /prueba", STEP)?;
    // Escribir en el disco durmió a la tarea hasta la interrupción del disco (K9).
    s.saw_or_wait("DISCO_POR_INTERRUPCION", STEP)?;
    // El cursor arranca en el centro (640, 400); ahí hay una fila de la lista de Archivos.
    s.monitor("mouse_move -40 -50")?;
    thread::sleep(Duration::from_millis(200));
    s.monitor("mouse_button 1")?;
    s.monitor("mouse_button 0")?;
    s.wait_for("ARCHIVOS_SELECCION", STEP)?;

    s.monitor("sendkey meta_l-r")?;
    s.wait_for("VENTANA_ABIERTA Consola JARVIS", STEP)?;
    if brain.is_some() {
        // Lo que no es una orden local va al cerebro, y la respuesta vuelve.
        s.type_text("hola jarvis")?;
        s.monitor("sendkey ret")?;
        s.wait_for("CEREBRO_RESPUESTA Hola, Roman.", STEP)?;
    }
    s.type_text(&format!("ir http://10.0.2.2:{port}/"))?;
    s.monitor("sendkey ret")?;
    s.wait_for("VENTANA_ABIERTA Navegador", STEP)?;
    s.wait_for("RED_RESPUESTA 200", STEP)?;
    s.monitor("sendkey alt-tab")?;
    s.wait_for("VENTANA_FOCO", STEP)?;
    s.monitor("sendkey meta_l-d")?;
    s.wait_for("ESCRITORIO_MOSTRAR", STEP)?;

    // Terminal (Ctrl+Alt+T): una redirección escribe en el disco.
    s.monitor("sendkey ctrl-alt-t")?;
    s.wait_for("VENTANA_ABIERTA Terminal", STEP)?;
    s.type_text("echo hola terminal > saludo.txt")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // apt: instala un paquete del repositorio (lo sirve el puente) y se ejecuta.
    s.type_text("apt install hola")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO hola", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("hola")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL hola", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // snap: un programa de la tienda de JARVIS-OS (también la sirve el puente).
    s.type_text("snap install saludo")?;
    s.monitor("sendkey ret")?;
    s.wait_for("SNAP_INSTALADO saludo", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Firewall: una regla de ufw frena a wget antes de que salga un solo paquete.
    s.type_text("sudo ufw deny out to example.com")?;
    s.monitor("sendkey ret")?;
    s.wait_for("FIREWALL ufw deny out to example.com", STEP)?;
    s.type_text("wget http://example.com/")?;
    s.monitor("sendkey ret")?;
    s.wait_for("FIREWALL_BLOQUEO terminal example.com", STEP)?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    // K10: HTTPS con el TLS del kernel, contra sitios de verdad (hace falta internet, por eso
    // es opcional). Uno válido tiene que llegar; uno con el certificado vencido, no.
    if std::env::var_os("JARVIS_TEST_INTERNET").is_some() {
        s.saw_or_wait("RED_HTTPS TLS del kernel", STEP)?;
        s.type_text("wget https://example.org/")?;
        s.monitor("sendkey ret")?;
        s.wait_for("RED_RESPUESTA 200 https://example.org/", STEP)?;
        s.wait_for("TERMINAL_FIN", STEP)?;
        s.type_text("wget https://expired.badssl.com/")?;
        s.monitor("sendkey ret")?;
        s.wait_for(
            "RED_ERROR conexión segura: el certificado del sitio no es válido",
            STEP,
        )?;
        s.wait_for("TERMINAL_FIN", STEP)?;
    }
    // K11: programas de Linux de verdad (ELF estáticos de musl), en el anillo 3. Se instalan con
    // apt (el puente los sirve desde target/usuario/).
    s.type_text("apt install programas-linux")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO programas-linux", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("hola-linux uno dos")?;
    s.monitor("sendkey ret")?;
    s.wait_for("Hola desde Linux! argumentos: [\"uno\", \"dos\"]", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Archivos, carpetas, 32 MiB de memoria, punto flotante y pila que crece.
    s.type_text("pruebas")?;
    s.monitor("sendkey ret")?;
    s.wait_for("PRUEBAS_OK", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // La entrada de una tubería llega como su entrada estándar.
    s.type_text("echo hola mundo | eco")?;
    s.monitor("sendkey ret")?;
    s.wait_for("HOLA MUNDO", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Un programa que lee un puntero nulo termina él; el sistema sigue.
    s.type_text("pruebas segv")?;
    s.monitor("sendkey ret")?;
    s.wait_for("PROCESO_SEGV", STEP)?;
    s.wait_for("TERMINAL_FIN 139", STEP)?;
    // Uno que calcula sin parar se reparte la CPU con el escritorio y Ctrl+C lo termina.
    s.type_text("pruebas bucle")?;
    s.monitor("sendkey ret")?;
    s.wait_for("calculando para siempre", STEP)?;
    s.monitor("sendkey ctrl-c")?;
    s.wait_for("TERMINAL_FIN 130", STEP)?;
    // Sockets: un cliente HTTP con TcpStream contra el servidor de prueba, y el firewall.
    s.type_text(&format!("red 10.0.2.2:{port}"))?;
    s.monitor("sendkey ret")?;
    s.wait_for("RED_PROGRAMA HTTP/1.1 200", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("sudo ufw deny out to 10.0.2.2 app programas")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text(&format!("red 10.0.2.2:{port}"))?;
    s.monitor("sendkey ret")?;
    s.wait_for("FIREWALL_BLOQUEO programas 10.0.2.2", STEP)?;
    s.wait_for("RED_PROGRAMA_ERROR", STEP)?;
    s.wait_for("TERMINAL_FIN 1", STEP)?;
    // Un intérprete de JavaScript (Boa) como programa de Linux: código suelto, un archivo y la
    // consola interactiva (lo tipeado es su entrada estándar).
    s.type_text("apt install js")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO js", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("js -e \"Array.from('abc', c => c.toUpperCase()).join('-')\"")?;
    s.monitor("sendkey ret")?;
    s.wait_for("A-B-C", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("echo \"console.log('desde un archivo', 6 * 7)\" > prueba.js && js prueba.js")?;
    s.monitor("sendkey ret")?;
    s.wait_for("desde un archivo 42", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("js")?;
    s.monitor("sendkey ret")?;
    s.wait_for("JavaScript (motor Boa)", STEP)?;
    s.type_text("let x = 20; (x + 22) * 1000 + 7")?;
    s.monitor("sendkey ret")?;
    s.wait_for("42007", STEP)?;
    s.monitor("sendkey ctrl-d")?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // K10: un PNG y un JPEG progresivo del repositorio, sin convertir: los decodifica el
    // kernel (el visor, con jarvis-image).
    for file in ["fondos/aurora.png", "pruebas/aurora-progresivo.jpg"] {
        s.type_text(&format!("wget http://paquetes.jarvis/{file}"))?;
        s.monitor("sendkey ret")?;
        s.wait_for("TERMINAL_FIN 0", STEP)?;
    }
    s.type_text("open aurora-progresivo.jpg && open aurora.png")?;
    s.monitor("sendkey ret")?;
    s.wait_for("VISOR_IMAGEN 640x400", STEP)?;
    s.wait_for("VISOR_IMAGEN 1280x800", STEP)?;
    // Configuración (Win+I).
    s.monitor("sendkey meta_l-i")?;
    s.wait_for("VENTANA_ABIERTA Configuración", STEP)?;

    // Brave (ADR 0007): la app se conecta con el puente por una conexión larga. Con Brave
    // instalado llega la página; sin Brave (la CI), el puente lo dice y la app lo muestra.
    // La consola ya estaba abierta: Win+R la trae adelante.
    s.monitor("sendkey meta_l-r")?;
    thread::sleep(Duration::from_millis(500));
    s.type_text("abrir brave")?;
    s.monitor("sendkey ret")?;
    s.wait_for("VENTANA_ABIERTA Brave", STEP)?;
    s.wait_for("BRAVE_CONECTADO 10.0.2.2:8119", STEP)?;
    let brave_ok = if brave::installed() {
        s.wait_for("BRAVE_FRAME", Duration::from_secs(40))?;
        true
    } else {
        s.wait_for("BRAVE_ERROR", STEP)?;
        false
    };

    let t0 = Instant::now();
    s.monitor("sendkey print")?;
    s.wait_for("CAPTURA /Imágenes/", STEP)?;
    println!(
        "[tiempo] captura de pantalla: {} ms",
        t0.elapsed().as_millis()
    );

    // Energía: suspender (el mouse despierta) y cerrar sesión (una tecla vuelve a entrar).
    s.monitor("sendkey meta_l-d")?;
    thread::sleep(Duration::from_millis(300));
    s.monitor("sendkey alt-f4")?;
    s.wait_for("ESCRITORIO_MENU apagado", STEP)?;
    s.monitor("sendkey right")?;
    s.monitor("sendkey right")?;
    s.monitor("sendkey ret")?;
    s.wait_for("SUSPENDIDO", STEP)?;
    thread::sleep(Duration::from_millis(500));
    s.monitor("mouse_move 20 0")?;
    s.wait_for("DESPIERTO", STEP)?;
    s.monitor("sendkey alt-f4")?;
    s.wait_for("ESCRITORIO_MENU apagado", STEP)?;
    for _ in 0..3 {
        s.monitor("sendkey right")?;
    }
    s.monitor("sendkey ret")?;
    s.wait_for("SESION_CERRADA", STEP)?;
    s.monitor("sendkey a")?;
    s.wait_for("SESION_INICIADA", STEP)?;
    if brain.is_some() {
        s.monitor("sendkey meta_l-r")?;
        thread::sleep(Duration::from_millis(500));
        // Cerebro: una acción de nivel 1 (sin confirmar) y una de nivel 3 (el diálogo; Esc rechaza).
        s.type_text("abri el navegador y busca rust")?;
        s.monitor("sendkey ret")?;
        s.wait_for("CEREBRO_ACCION buscar_web", STEP)?;
        s.wait_for("CEREBRO_ACCION_FIN 1 ok", STEP)?;
        s.monitor("sendkey meta_l-r")?;
        thread::sleep(Duration::from_millis(500));
        s.type_text("borra /prueba")?;
        s.monitor("sendkey ret")?;
        s.wait_for("CEREBRO_CONFIRMAR 3", STEP)?;
        thread::sleep(Duration::from_millis(500));
        s.screenshot_head(&target_dir().join("jarvis-os-confirmar.png"), None)?;
        s.monitor("sendkey esc")?;
        s.wait_for("CEREBRO_CONFIRMACION 2 no", STEP)?;
        s.wait_for("CEREBRO_RESPUESTA No lo hice", STEP)?;
        // Un proyecto: se permite abrirlo (nivel 2, con el teclado), se abre la ventana Proyecto
        // y la edición que pide el agente se rechaza.
        s.monitor("sendkey meta_l-r")?;
        thread::sleep(Duration::from_millis(500));
        s.type_text("abri el proyecto demo")?;
        s.monitor("sendkey ret")?;
        s.wait_for("CEREBRO_CONFIRMAR 2 Abrir el proyecto demo", STEP)?;
        s.monitor("sendkey left")?;
        s.monitor("sendkey ret")?;
        s.wait_for("PROYECTO_INICIO demo", STEP)?;
        s.wait_for("CEREBRO_CONFIRMAR 2 [demo] Edit README.md", STEP)?;
        s.monitor("sendkey esc")?;
        s.wait_for("PROYECTO_FIN demo", STEP)?;
        thread::sleep(Duration::from_millis(500));
        s.screenshot_head(&target_dir().join("jarvis-os-proyecto.png"), None)?;
    }
    s.quit();
    drop(s);
    verify_dir_on_disk(disk, "prueba")?;
    verify_capture_on_disk(disk)?;
    verify_file_on_disk(disk, "saludo.txt", b"hola terminal\n")?;
    verify_file_exists(disk, "Programas/bin/hola")?;
    verify_file_exists(disk, "snap/bin/saludo")?;
    verify_file_exists(disk, "Sistema/firewall.log")?;
    println!(
        "ok: arranque, red, teclado, mouse, ventanas, navegador, terminal, apt, snap, firewall, configuración, Brave ({}), suspender, cerrar sesión y disco verificados",
        if brave_ok {
            "con página"
        } else {
            "sin Brave en el anfitrión: aviso"
        }
    );
    // Y una vez desde la ISO, como un CD y sin disco.
    boot_iso(&iso::build(image)?)
}

fn open_fatfs(disk: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(disk)
        .map_err(|e| format!("disco {}: {e}", disk.display()))
}

/// Verifica con `fatfs` el contenido de un archivo.
fn verify_file_on_disk(disk: &Path, path: &str, want: &[u8]) -> Result<()> {
    let mut file = open_fatfs(disk)?;
    let fs =
        fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    fs.root_dir()
        .open_file(path)
        .map_err(|e| format!("no quedó /{path}: {e}"))?
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data != want {
        return Err(format!(
            "/{path} tiene {:?}, se esperaba {:?}",
            String::from_utf8_lossy(&data),
            String::from_utf8_lossy(want)
        ));
    }
    println!(
        "[disco] /{path} = {:?} (verificado con fatfs)",
        String::from_utf8_lossy(&data)
    );
    Ok(())
}

fn verify_file_exists(disk: &Path, path: &str) -> Result<()> {
    let mut file = open_fatfs(disk)?;
    let fs =
        fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    fs.root_dir()
        .open_file(path)
        .map_err(|e| format!("no quedó /{path}: {e}"))?;
    println!("[disco] /{path} existe (verificado con fatfs)");
    Ok(())
}

/// Verifica con `fatfs` que en /Imágenes haya una captura BMP.
fn verify_capture_on_disk(disk: &Path) -> Result<()> {
    let io = |e: std::io::Error| format!("disco {}: {e}", disk.display());
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(disk)
        .map_err(io)?;
    let fs = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(io)?;
    let dir = fs
        .root_dir()
        .open_dir("Imágenes")
        .map_err(|e| format!("no hay /Imágenes: {e}"))?;
    let capture = dir
        .iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().starts_with("Captura") && e.file_name().ends_with(".bmp"))
        .ok_or("no quedó la captura en /Imágenes")?;
    let mut data = Vec::new();
    capture.to_file().read_to_end(&mut data).map_err(io)?;
    if !data.starts_with(b"BM") {
        return Err("la captura no es un BMP".into());
    }
    println!(
        "[disco] /Imágenes/{} ({} bytes, verificado con fatfs)",
        capture.file_name(),
        data.len()
    );
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

/// Capturas: JARVIS en reposo y hablando, Archivos y un diálogo, el monitor, el navegador, el
/// menú de inicio, Alt+Tab y la vista de tareas.
fn screenshot(image: &Path, disk: &Path) -> Result<()> {
    let port = puente::test_server(TEST_PAGE)?;
    puente::start();
    brave::start(brave_profile());
    let shot = |s: &mut Session, name: &str| s.screenshot(&target_dir().join(name));
    let mut s = Session::start(image, disk)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.wait_for("JARVIS_REPOSO", Duration::from_secs(30))?;
    thread::sleep(Duration::from_millis(1500)); // que termine de apagarse el brillo
    shot(&mut s, "jarvis-os.png")?;
    s.monitor("sendkey spc")?;
    s.wait_for("JARVIS_HABLA", STEP)?;
    thread::sleep(Duration::from_millis(1200)); // a mitad de la frase
    shot(&mut s, "jarvis-os-hablando.png")?;

    // Archivos: /Documentos con "Bienvenida.txt" seleccionado (vista previa en el inspector).
    s.monitor("sendkey tab")?;
    s.wait_for("ARCHIVOS_ABIERTO /", STEP)?;
    s.type_text("doc")?; // buscar tipeando: /Documentos (antes está /Descargas)
    s.monitor("sendkey ret")?;
    s.wait_for("ARCHIVOS_ABIERTO /Documentos", STEP)?;
    s.monitor("sendkey down")?;
    s.wait_for("ARCHIVOS_SELECCION Bienvenida.txt", STEP)?;
    s.monitor("mouse_move 150 80")?; // que se vea el cursor
    thread::sleep(Duration::from_millis(500));
    shot(&mut s, "jarvis-os-archivos.png")?;
    s.monitor("sendkey f7")?;
    s.type_text("tareas")?;
    thread::sleep(Duration::from_millis(500));
    shot(&mut s, "jarvis-os-dialogo.png")?;
    s.monitor("sendkey esc")?;
    // Varios archivos a la vez (Ctrl+E).
    s.monitor("sendkey ctrl-e")?;
    s.wait_for("ARCHIVOS_SELECCION_TODO", STEP)?;
    thread::sleep(Duration::from_millis(500));
    shot(&mut s, "jarvis-os-seleccion.png")?;
    s.monitor("sendkey esc")?;

    // Monitor del sistema (Ctrl+Shift+Esc), con unos segundos de historia en los gráficos.
    s.monitor("sendkey ctrl-shift-esc")?;
    s.wait_for("VENTANA_ABIERTA Monitor", STEP)?;
    thread::sleep(Duration::from_secs(6));
    shot(&mut s, "jarvis-os-monitor.png")?;
    // Distribuciones (Win+Z), con el Monitor enfocado: la segunda zona de los tercios.
    s.monitor("sendkey meta_l-z")?;
    s.wait_for("ESCRITORIO_MENU distribuciones", STEP)?;
    s.monitor("sendkey down")?;
    s.monitor("sendkey right")?;
    thread::sleep(Duration::from_millis(500));
    shot(&mut s, "jarvis-os-distribuciones.png")?;
    s.monitor("sendkey esc")?;

    // Navegador con la página de prueba.
    s.monitor("sendkey meta_l-r")?;
    s.wait_for("VENTANA_ABIERTA Consola JARVIS", STEP)?;
    s.type_text(&format!("ir http://10.0.2.2:{port}/"))?;
    s.monitor("sendkey ret")?;
    s.wait_for("RED_RESPUESTA 200", STEP)?;
    thread::sleep(Duration::from_millis(800));
    shot(&mut s, "jarvis-os-navegador.png")?;

    // Internet de verdad (si hay): http:// directo (DNS + TCP) y https:// por el puente.
    // Si no hay conexión, se sigue igual: estas capturas son opcionales. El navegador,
    // maximizado (como se usa para leer).
    s.monitor("sendkey meta_l-up")?;
    thread::sleep(Duration::from_millis(300));
    for (url, name) in [
        (
            "http://info.cern.ch/hypertext/WWW/TheProject.html",
            "jarvis-os-web-http.png",
        ),
        (
            "https://es.wikipedia.org/wiki/Sistema_operativo",
            "jarvis-os-web-https.png",
        ),
    ] {
        s.monitor("sendkey ctrl-l")?;
        s.type_text(url)?;
        s.monitor("sendkey ret")?;
        match s.wait_for("RED_RESPUESTA", Duration::from_secs(30)) {
            Ok(()) => {
                // Hojas de estilo, imágenes y la maquetación.
                thread::sleep(Duration::from_secs(10));
                shot(&mut s, name)?;
            }
            Err(e) => println!("(sin internet para {url}: {e})"),
        }
    }

    // YouTube: se arma con JavaScript; el navegador usa los datos que trae la página.
    s.monitor("sendkey ctrl-l")?;
    s.type_text("https://www.youtube.com/results?search_query=rust+kernel")?;
    s.monitor("sendkey ret")?;
    match s.wait_for("RED_RESPUESTA", Duration::from_secs(40)) {
        Ok(()) => {
            thread::sleep(Duration::from_secs(12));
            shot(&mut s, "jarvis-os-youtube.png")?;
        }
        Err(e) => println!("(sin internet para YouTube: {e})"),
    }

    // Google con sus estilos (internet de verdad, opcional).
    s.monitor("sendkey ctrl-l")?;
    s.type_text("https://www.google.com/")?;
    s.monitor("sendkey ret")?;
    match s.wait_for("RED_RESPUESTA", Duration::from_secs(30)) {
        Ok(()) => {
            thread::sleep(Duration::from_secs(4)); // hojas de estilo e imágenes
            shot(&mut s, "jarvis-os-google.png")?;
        }
        Err(e) => println!("(sin internet para Google: {e})"),
    }

    // GitHub: unas 20 hojas de estilo y variables de CSS (internet de verdad, opcional).
    s.monitor("sendkey ctrl-l")?;
    s.type_text("https://github.com/rust-lang/rust")?;
    s.monitor("sendkey ret")?;
    match s.wait_for("RED_RESPUESTA", Duration::from_secs(30)) {
        Ok(()) => {
            thread::sleep(Duration::from_secs(15));
            shot(&mut s, "jarvis-os-github.png")?;
        }
        Err(e) => println!("(sin internet para GitHub: {e})"),
    }

    // Brave de verdad (si está instalado en el anfitrión y hay internet), maximizado.
    if brave::installed() {
        s.monitor("sendkey meta_l-r")?;
        thread::sleep(Duration::from_millis(500));
        s.type_text("abrir brave")?;
        s.monitor("sendkey ret")?;
        s.wait_for("VENTANA_ABIERTA Brave", STEP)?;
        match s.wait_for("BRAVE_FRAME", Duration::from_secs(40)) {
            Ok(()) => {
                s.monitor("sendkey meta_l-up")?;
                thread::sleep(Duration::from_millis(500));
                s.monitor("sendkey ctrl-l")?;
                s.type_text("https://www.youtube.com/results?search_query=rust+kernel")?;
                s.monitor("sendkey ret")?;
                thread::sleep(Duration::from_secs(12));
                shot(&mut s, "jarvis-os-brave.png")?;
                // Se cierra, para que no tape las capturas que siguen.
                s.monitor("sendkey alt-f4")?;
                s.wait_for("CONEXION_CERRADA", STEP)?;
            }
            Err(e) => println!("(Brave no mostró la página: {e})"),
        }
    }

    // Terminal: apt instala programas y fondos del repositorio; neofetch y cowsay.
    s.monitor("sendkey ctrl-alt-t")?;
    s.wait_for("VENTANA_ABIERTA Terminal", STEP)?;
    s.type_text("apt install esenciales fondos")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO fondos", Duration::from_secs(60))?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    s.type_text("clear")?;
    s.monitor("sendkey ret")?;
    s.type_text("neofetch")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    s.type_text("fortune | cowsay")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-terminal.png")?;
    // snap y el firewall desde la terminal.
    s.type_text("clear")?;
    s.monitor("sendkey ret")?;
    s.type_text("snap install saludo --beta && saludo && snap list")?;
    s.monitor("sendkey ret")?;
    s.wait_for("SNAP_INSTALADO saludo", STEP)?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    s.type_text("sudo ufw deny out to tiktok.com && ufw status numbered")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-snap-ufw.png")?;

    // Configuración: Personalización, con un fondo de pantalla instalado.
    s.monitor("sendkey meta_l-i")?;
    s.wait_for("VENTANA_ABIERTA Configuración", STEP)?;
    // Sistema → Pantallas → Personalización.
    s.monitor("sendkey pgdn")?;
    s.monitor("sendkey pgdn")?;
    for _ in 0..7 {
        s.monitor("sendkey right")?;
        thread::sleep(Duration::from_millis(150));
    }
    s.wait_for("CONFIG_GUARDADA", STEP)?;
    thread::sleep(Duration::from_millis(800));
    shot(&mut s, "jarvis-os-configuracion.png")?;
    // La sección Firewall (la última: RePág desde Personalización, tres veces, da la vuelta).
    for _ in 0..3 {
        s.monitor("sendkey pgup")?;
    }
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-firewall.png")?;
    // Apariencia (Firewall → AvPág da la vuelta: Sistema, Pantallas, Personalización,
    // Apariencia): el tema claro, y después se vuelve al HUD para las capturas que siguen.
    for _ in 0..4 {
        s.monitor("sendkey pgdn")?;
    }
    s.monitor("sendkey right")?;
    s.wait_for("CONFIG_GUARDADA", STEP)?;
    thread::sleep(Duration::from_millis(800));
    shot(&mut s, "jarvis-os-claro.png")?;
    s.monitor("sendkey left")?;
    s.wait_for("CONFIG_GUARDADA", STEP)?;

    // Paneles: Win+X, Win+A, Win+N (sobre el escritorio con el fondo nuevo).
    s.monitor("sendkey meta_l-d")?;
    s.wait_for("ESCRITORIO_MOSTRAR", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-fondo.png")?;
    for (key, marker, name) in [
        ("meta_l-x", "enlaces", "jarvis-os-win-x.png"),
        ("meta_l-a", "rapida", "jarvis-os-win-a.png"),
        ("meta_l-n", "notificaciones", "jarvis-os-win-n.png"),
    ] {
        s.monitor(&format!("sendkey {key}"))?;
        s.wait_for(&format!("ESCRITORIO_MENU {marker}"), STEP)?;
        thread::sleep(Duration::from_millis(600));
        shot(&mut s, name)?;
        s.monitor("sendkey esc")?;
        thread::sleep(Duration::from_millis(300));
    }
    s.monitor("sendkey meta_l-d")?;
    s.wait_for("ESCRITORIO_MOSTRAR", STEP)?;

    // Alt+Tab con Alt apretado (sendkey con tiempo de espera: mantiene las teclas).
    s.monitor("sendkey alt-tab 3000")?;
    s.wait_for("ESCRITORIO_MENU alt-tab", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-alt-tab.png")?;
    thread::sleep(Duration::from_millis(2500));

    // Vista de tareas (Win+Tab) y menú de inicio (Win).
    s.monitor("sendkey meta_l-tab")?;
    s.wait_for("ESCRITORIO_MENU tareas", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-tareas.png")?;
    s.monitor("sendkey esc")?;
    s.monitor("sendkey meta_l")?;
    s.wait_for("ESCRITORIO_MENU inicio", STEP)?;
    thread::sleep(Duration::from_millis(600));
    shot(&mut s, "jarvis-os-inicio.png")?;
    s.monitor("sendkey esc")?;

    // Energía (Alt+F4 con el escritorio al frente) y la pantalla de inicio de sesión.
    s.monitor("sendkey meta_l-d")?;
    thread::sleep(Duration::from_millis(300));
    s.monitor("sendkey alt-f4")?;
    s.wait_for("ESCRITORIO_MENU apagado", STEP)?;
    s.monitor("sendkey right")?;
    s.monitor("sendkey right")?;
    s.monitor("sendkey right")?;
    thread::sleep(Duration::from_millis(400));
    shot(&mut s, "jarvis-os-energia.png")?;
    s.monitor("sendkey ret")?;
    s.wait_for("SESION_CERRADA", STEP)?;
    thread::sleep(Duration::from_millis(500));
    shot(&mut s, "jarvis-os-sesion.png")
}

/// `cargo xtask pantallas`: dos monitores (virtio-gpu con dos salidas). Extender: el Monitor pasa
/// a la segunda pantalla (Win+Shift+→) y se captura cada una; después Win+P → Duplicar.
fn screens(image: &Path, disk: &Path) -> Result<()> {
    MONITORS.store(2, std::sync::atomic::Ordering::Relaxed);
    let mut s = Session::start(image, disk)?;
    s.wait_for("PANTALLAS_LISTAS 2560x800", BOOT_TIMEOUT)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.monitor("sendkey ctrl-shift-esc")?;
    s.wait_for("VENTANA_ABIERTA Monitor", STEP)?;
    s.monitor("sendkey meta_l-shift-right")?;
    s.wait_for("VENTANA_OTRO_MONITOR", STEP)?;
    s.monitor("sendkey meta_l-e")?;
    s.wait_for("VENTANA_ABIERTA Archivos", STEP)?;
    thread::sleep(Duration::from_secs(3));
    let dir = target_dir();
    s.screenshot_head(&dir.join("jarvis-os-pantalla1.png"), Some(0))?;
    s.screenshot_head(&dir.join("jarvis-os-pantalla2.png"), Some(1))?;
    // Win+P: el panel, y "Duplicar" (la segunda opción).
    s.monitor("sendkey meta_l-p")?;
    s.wait_for("ESCRITORIO_MENU proyectar", STEP)?;
    thread::sleep(Duration::from_millis(500));
    s.screenshot_head(&dir.join("jarvis-os-win-p.png"), Some(0))?;
    // Las opciones: solo la 1, duplicar, extender (la actual), solo la 2.
    s.monitor("sendkey up")?;
    s.monitor("sendkey ret")?;
    // (El kernel arma la imagen nueva antes de anotar el modo en el log.)
    s.wait_for("PANTALLAS_LISTAS 1280x800", STEP)?;
    s.wait_for("PANTALLAS_MODO duplicar", STEP)?;
    thread::sleep(Duration::from_secs(1));
    s.screenshot_head(&dir.join("jarvis-os-duplicar2.png"), Some(1))?;
    s.quit();
    println!("ok: dos monitores (extender, mover una ventana, Win+P y duplicar)");
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

// --- disco virtual ---------------------------------------------------------------------------

/// Tamaño de los discos nuevos. (Uno que ya existe no cambia: `cargo xtask disk --reset` crea
/// uno nuevo, pero borra lo que tenía.)
const DISK_MIB: u64 = 256;
/// Clusters de 2 KiB: con 256 MiB quedan ~130 000 (FAT32 necesita al menos 65 525).
const CLUSTER: u32 = 2048;
/// Carpetas que se crean aunque estén vacías (git no guarda carpetas vacías).
const EMPTY_DIRS: [&str; 5] = [
    "Papelera",
    "Facultad/Algoritmos",
    "Descargas",
    "Programas/bin",
    "Sistema/paquetes",
];

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

/// Un disco recién creado (tests y capturas: siempre parten del mismo estado). El teclado queda
/// en EE. UU.: QEMU manda las teclas por su nombre en esa distribución (`sendkey slash`).
fn fresh_disk(name: &str) -> Result<PathBuf> {
    let path = target_dir().join(name);
    create_disk(&path)?;
    let mut file = open_fatfs(&path)?;
    let fs =
        fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    let mut cfg = fs
        .root_dir()
        .open_dir("Sistema")
        .and_then(|d| d.create_file("config.ini"))
        .map_err(|e| e.to_string())?;
    cfg.write_all(
        b"# Pruebas automaticas: QEMU tipea con nombres de teclas de EE. UU.\nteclado=us\n",
    )
    .map_err(|e| e.to_string())?;
    drop(cfg);
    fs.unmount().map_err(|e| e.to_string())?;
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
        .bytes_per_cluster(CLUSTER)
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
