//! Programas de Windows en JARVIS-OS (ADR 0018): `vino` y los `.exe` de prueba.
//!
//! - `vino` (kernel/vino/) es un programa de Linux: se compila para musl como los de
//!   `kernel/usuario/` y va a `target/usuario/vino` (lo instala `apt install vino`).
//! - Los `.exe` de prueba se compilan con Visual Studio (`cargo xtask windows`, solo en Windows)
//!   y quedan **en el repositorio** (`kernel/usuario-win/bin/`), como los `.deb` de los tests de
//!   apt: la CI corre en Linux y no tiene el compilador de Microsoft. `cargo xtask usuario` los
//!   copia a `target/usuario/` (`apt install programas-windows`).
//! - `cargo xtask vino`: arranca JARVIS-OS, instala todo y corre cada `.exe` (también es parte de
//!   `cargo xtask test`).

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use super::{BOOT_MARKER, BOOT_TIMEOUT, Result, STEP, Session, target_dir, workspace_root};

/// Los `.exe` de prueba: (nombre, de dónde sale).
pub const PROGRAMS: [(&str, &str); 9] = [
    (
        "hola-rust-win.exe",
        "Rust, con el CRT en DLL (vcruntime140 y ucrtbase)",
    ),
    (
        "hola-rust-win-mt.exe",
        "Rust, con el CRT de Microsoft adentro (+crt-static)",
    ),
    ("hola-win.exe", "C, /MD (el CRT en DLL)"),
    (
        "hola-win-mt.exe",
        "C, /MT (el CRT adentro: solo usa kernel32)",
    ),
    ("falta-win.exe", "C, /MD, usa una función que vino no tiene"),
    (
        "saludo.dll",
        "DLL de C propia: DllMain, exports y TLS adentro de la DLL",
    ),
    (
        "usa-dll.exe",
        "C, usa saludo.dll por import y con LoadLibrary",
    ),
    ("hilos-win.exe", "Rust, hilos: Mutex, Condvar, canales, TLS"),
    (
        "hilos-c-win.exe",
        "C, hilos: CreateThread, CRITICAL_SECTION, eventos, semáforos",
    ),
];

fn prebuilt_dir() -> PathBuf {
    workspace_root().join("usuario-win").join("bin")
}

/// Compila `vino` y lo deja con los `.exe` de prueba en `out` (`target/usuario/`).
pub fn build_into(out: &Path) -> Result<()> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build_dir = target_dir().join("vino-build");
    let status = Command::new(cargo)
        .current_dir(workspace_root().join("vino"))
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .args([
            "build",
            "--release",
            "--target",
            "x86_64-unknown-linux-musl",
            "--target-dir",
        ])
        .arg(&build_dir)
        .status()
        .map_err(|e| format!("no pude ejecutar cargo: {e}"))?;
    if !status.success() {
        return Err("falló la compilación de vino (kernel/vino)".into());
    }
    let from = build_dir.join("x86_64-unknown-linux-musl/release/vino");
    fs::copy(&from, out.join("vino")).map_err(|e| format!("{}: {e}", from.display()))?;
    for (name, _) in PROGRAMS {
        let from = prebuilt_dir().join(name);
        fs::copy(&from, out.join(name)).map_err(|e| {
            format!(
                "{}: {e} (se generan con `cargo xtask windows`, en Windows con Visual Studio)",
                from.display()
            )
        })?;
    }
    Ok(())
}

/// `vcvars64.bat` de Visual Studio (el que deja listo `cl.exe`), con `vswhere`.
fn vcvars() -> Result<PathBuf> {
    let vswhere =
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    let out = Command::new(&vswhere)
        .args(["-latest", "-products", "*", "-property", "installationPath"])
        .output()
        .map_err(|e| format!("no encontré Visual Studio ({}): {e}", vswhere.display()))?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let bat = Path::new(&dir).join(r"VC\Auxiliary\Build\vcvars64.bat");
    if !bat.exists() {
        return Err(format!(
            "no encontré {}: instalá \"Desarrollo para el escritorio con C++\" en Visual Studio",
            bat.display()
        ));
    }
    Ok(bat)
}

/// `cargo xtask windows`: compila los `.exe` de prueba a `kernel/usuario-win/bin/`.
pub fn build_programs() -> Result<()> {
    if !cfg!(windows) {
        return Err("los .exe de prueba se compilan en Windows (con Visual Studio)".into());
    }
    let src = workspace_root().join("usuario-win");
    let bin = prebuilt_dir();
    fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    for (flags, dir, name) in [
        ("", "usuario-win-md", "hola-rust-win.exe"),
        (
            "-C target-feature=+crt-static",
            "usuario-win-mt",
            "hola-rust-win-mt.exe",
        ),
    ] {
        let build_dir = target_dir().join(dir);
        let status = Command::new(&cargo)
            .current_dir(&src)
            .env("RUSTFLAGS", flags)
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .args([
                "build",
                "--release",
                "--target",
                "x86_64-pc-windows-msvc",
                "--target-dir",
            ])
            .arg(&build_dir)
            .status()
            .map_err(|e| format!("no pude ejecutar cargo: {e}"))?;
        if !status.success() {
            return Err("falló la compilación de kernel/usuario-win".into());
        }
        let from = build_dir.join("x86_64-pc-windows-msvc/release/hola-rust-win.exe");
        fs::copy(&from, bin.join(name)).map_err(|e| format!("{}: {e}", from.display()))?;
        if flags.is_empty() {
            let from = build_dir.join("x86_64-pc-windows-msvc/release/hilos-win.exe");
            fs::copy(&from, bin.join("hilos-win.exe"))
                .map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }
    // Los de C: un .bat que prepara el entorno de Visual Studio y llama a cl.
    let work = target_dir().join("usuario-win-c");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let c = |f: &str| src.join(f).display().to_string();
    // (opciones de cl, fuentes, salida)
    let jobs = [
        ("/MD", "hola-win.c", "hola-win.exe"),
        ("/MT", "hola-win.c", "hola-win-mt.exe"),
        ("/MD", "falta-win.c", "falta-win.exe"),
        ("/MD /LD", "saludo.c", "saludo.dll"),
        // saludo.lib la deja la línea anterior: con ella se enlazan los imports de la DLL.
        ("/MD", "usa-dll.c saludo.lib", "usa-dll.exe"),
        ("/MD", "hilos-win.c", "hilos-c-win.exe"),
    ];
    let mut script = format!(
        "@call \"{}\" >nul\r\ncd /d \"{}\"\r\n",
        vcvars()?.display(),
        work.display()
    );
    for (opts, srcs, out) in jobs {
        let files: Vec<String> = srcs
            .split(' ')
            .map(|f| {
                if f.ends_with(".c") {
                    format!("\"{}\"", c(f))
                } else {
                    f.to_string()
                }
            })
            .collect();
        script.push_str(&format!(
            "cl /nologo /O1 {opts} {} /Fe:{out} || exit /b 1\r\n",
            files.join(" ")
        ));
    }
    let bat = work.join("compilar.bat");
    fs::write(&bat, script).map_err(|e| e.to_string())?;
    let status = Command::new("cmd")
        .arg("/c")
        .arg(&bat)
        .status()
        .map_err(|e| format!("no pude ejecutar cmd: {e}"))?;
    if !status.success() {
        return Err("falló la compilación de los programas de C (cl.exe)".into());
    }
    for (_, _, name) in jobs {
        fs::copy(work.join(name), bin.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    for (name, what) in PROGRAMS {
        let size = fs::metadata(bin.join(name)).map(|m| m.len()).unwrap_or(0);
        println!("{name:22} {:>4} KB  {what}", size / 1024);
    }
    Ok(())
}

/// Lo que se prueba en QEMU, con la Terminal ya abierta (lo usan `vino` y `test`).
pub fn steps(s: &mut Session) -> Result<()> {
    s.type_text("apt install vino programas-windows")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO programas-windows", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("cd /Documentos")?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Rust con la biblioteca estándar entera: argumentos, entorno, colecciones, archivos,
    // carpetas, la hora y la entrada. Con el CRT en DLL y con el CRT adentro.
    for exe in ["hola-rust-win.exe", "hola-rust-win-mt.exe"] {
        s.type_text(&format!("wine /Programas/windows/{exe} uno dos"))?;
        s.monitor("sendkey ret")?;
        s.wait_for("TERMINAL_WINDOWS /Programas/windows/", STEP)?;
        s.wait_for("Hola desde Rust en Windows! 3 argumentos", STEP)?;
        s.wait_for("arg: dos", STEP)?;
        s.wait_for("archivo: 3 lineas", STEP)?;
        s.wait_for("carpeta: 1 archivo(s)", STEP)?;
        s.wait_for("hora: ok", STEP)?;
        s.wait_for("Escribi algo:", STEP)?;
        s.type_text("vino")?;
        s.monitor("sendkey ret")?;
        s.wait_for("Leido: vino", STEP)?;
        s.wait_for("TERMINAL_FIN 0", STEP)?;
    }
    // C con printf, archivos, matemática, atexit y la API de Windows directa; con /MD y /MT.
    // `./programa.exe` también lo corre vino.
    for exe in ["hola-win.exe", "hola-win-mt.exe"] {
        s.type_text(&format!(
            "cp /Programas/windows/{exe} . && ./{exe} a \"b c\""
        ))?;
        s.monitor("sendkey ret")?;
        s.wait_for("Hola desde Windows! argc=3", STEP)?;
        s.wait_for("argv[2] = b c", STEP)?;
        s.wait_for("memoria ok, calloc suma 0", STEP)?;
        s.wait_for(
            "enteros: -42     7|7    |00007 3000000000 ff FF 1234567890123",
            STEP,
        )?;
        s.wait_for("reales: 3.14   -2.500 1.234568e+04 0.0001", STEP)?;
        s.wait_for("raiz de 2 = 1.414214, seno(1) = 0.8415, 2^10 = 1024", STEP)?;
        s.wait_for("archivo: 2 lineas", STEP)?;
        s.wait_for("GetModuleFileNameA: ok", STEP)?;
        s.wait_for("WriteConsoleA ok", STEP)?;
        s.wait_for("Como te llamas?", STEP)?;
        s.type_text("Roman")?;
        s.monitor("sendkey ret")?;
        s.wait_for("Hola, Roman!", STEP)?;
        s.wait_for("atexit: chau (3)", STEP)?;
        s.wait_for("TERMINAL_FIN 0", STEP)?;
    }
    // Un programa con su propia DLL: se busca al lado del .exe, se inicia (DllMain) y también se
    // carga con LoadLibrary.
    s.type_text("wine /Programas/windows/usa-dll.exe")?;
    s.monitor("sendkey ret")?;
    s.wait_for("dll: DllMain ok, 2+3=5, tls=6", STEP)?;
    s.wait_for("LoadLibrary: ok, GetProcAddress: 42", STEP)?;
    s.wait_for("ruta de la DLL: ok", STEP)?;
    s.wait_for("no-existe.dll: NULL ok", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Hilos de Windows sobre hilos de Linux (clone + futex en el kernel).
    s.type_text("wine /Programas/windows/hilos-win.exe")?;
    s.monitor("sendkey ret")?;
    s.wait_for("win mutex: 4000, tls: [10, 11, 12, 13]", STEP)?;
    s.wait_for("win condvar ok", STEP)?;
    s.wait_for("win paralelo: ok", STEP)?;
    s.wait_for("HILOS_WIN_OK", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("wine /Programas/windows/hilos-c-win.exe")?;
    s.monitor("sendkey ret")?;
    s.wait_for(
        "c suma: 4000, atomico: 4000, codigos: 1000, propio main: 0",
        STEP,
    )?;
    s.wait_for("antes del evento: esperando", STEP)?;
    s.wait_for("evento y semaforo: ok, codigo 7", STEP)?;
    s.wait_for("HILOS_C_OK", STEP)?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    // Lo que falta se dice (y el programa termina), en vez de colgarse.
    s.type_text("wine /Programas/windows/falta-win.exe")?;
    s.monitor("sendkey ret")?;
    s.wait_for("antes", STEP)?;
    s.wait_for("TERMINAL_FIN 3", STEP)?;
    s.saw_or_wait("falta KERNEL32!GetSystemPowerStatus", STEP)?;
    Ok(())
}

/// `cargo xtask vino`: solo la parte de Windows (más rápido que `cargo xtask test`).
pub fn test(image: &Path, disk: &Path, python: bool) -> Result<()> {
    if python {
        copy_python(disk)?;
    }
    super::puente::start();
    let mut s = Session::start(image, disk)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.monitor("sendkey ctrl-alt-t")?;
    s.wait_for("VENTANA_ABIERTA Terminal", STEP)?;
    thread::sleep(Duration::from_millis(500));
    let mut r = steps(&mut s);
    if python && r.is_ok() {
        r = python_steps(&mut s);
    }
    thread::sleep(Duration::from_secs(1));
    s.screenshot(&target_dir().join("jarvis-os-vino.png"))?;
    s.quit();
    r?;
    println!("ok: vino corrió los programas de Windows (Rust y C, con el CRT en DLL y adentro)");
    Ok(())
}

/// Python de Windows (el del anfitrión) corre un script en JARVIS-OS y tiene que dar lo mismo que
/// en Windows: se compara con lo que da el mismo Python acá.
fn python_steps(s: &mut Session) -> Result<()> {
    let script = workspace_root().join("usuario-win/python/prueba-python.py");
    let py = host_python().ok_or("no encontré Python en el anfitrión")?;
    let work = target_dir().join("python-anfitrion");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let native = Command::new(py.join("python.exe"))
        .arg(&script)
        .current_dir(&work)
        .output()
        .map_err(|e| e.to_string())?;
    let want = String::from_utf8_lossy(&native.stdout).replace("\r\n", "\n");
    fs::copy(&script, target_dir().join("usuario/prueba-python.py")).map_err(|e| e.to_string())?;
    s.type_text(
        "wget -q http://paquetes.jarvis/usuario/prueba-python.py -O /Documentos/prueba-python.py",
    )?;
    s.monitor("sendkey ret")?;
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    s.type_text("wine /Programas/python/python.exe /Documentos/prueba-python.py")?;
    s.monitor("sendkey ret")?;
    // Las últimas líneas no dependen de la carpeta: hashes, decimales, azar con semilla.
    let lines: Vec<&str> = want.lines().collect();
    for line in &lines[lines.len().saturating_sub(5)..] {
        s.wait_for(line, Duration::from_secs(120))?;
    }
    s.wait_for("TERMINAL_FIN 0", STEP)?;
    Ok(())
}

/// Las carpetas de la biblioteca de Python que no hacen falta (y pesan): tests, interfaces
/// gráficas, instaladores y los `.pyc` del anfitrión.
const PYTHON_SKIP: [&str; 9] = [
    "__pycache__",
    "test",
    "tests",
    "idlelib",
    "tkinter",
    "turtledemo",
    "ensurepip",
    "site-packages",
    "lib2to3",
];

/// Copia `src` adentro de `dir` en el disco (sin lo de [`PYTHON_SKIP`] ni las DLL de prueba).
fn copy_filtered<T: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<'_, T>, src: &Path) -> Result<()> {
    let io = |e: std::io::Error| format!("{}: {e}", src.display());
    for entry in fs::read_dir(src).map_err(io)?.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if PYTHON_SKIP.contains(&name.as_str()) || name.starts_with("_test") {
            continue;
        }
        if path.is_dir() {
            copy_filtered(&dir.create_dir(&name).map_err(io)?, &path)?;
        } else {
            let data = fs::read(&path).map_err(io)?;
            let mut file = dir.create_file(&name).map_err(io)?;
            file.truncate().map_err(io)?;
            file.write_all(&data).map_err(io)?;
        }
    }
    Ok(())
}

/// El Python de Windows instalado en el anfitrión (para probar `vino` con un programa grande de
/// verdad). Solo en la PC de desarrollo: no va al repositorio.
fn host_python() -> Option<PathBuf> {
    let base = PathBuf::from(env::var("LOCALAPPDATA").ok()?).join("Programs/Python");
    let mut found: Vec<PathBuf> = fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("python.exe").is_file())
        .collect();
    found.sort();
    found.pop()
}

/// Pone el Python del anfitrión en `/Programas/python` del disco.
fn copy_python(disk: &Path) -> Result<()> {
    let py = host_python().ok_or(r"no encontré Python en %LOCALAPPDATA%\Programs\Python")?;
    let mut file = super::open_fatfs(disk)?;
    let fs =
        fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    let root = fs.root_dir();
    let programas = root
        .open_dir("Programas")
        .or_else(|_| root.create_dir("Programas"))
        .map_err(|e| e.to_string())?;
    let dest = programas.create_dir("python").map_err(|e| e.to_string())?;
    for entry in fs::read_dir(&py)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let path = entry.path();
        if path.is_file() && (lower.ends_with(".exe") || lower.ends_with(".dll")) {
            let data = fs::read(&path).map_err(|e| e.to_string())?;
            let mut f = dest.create_file(&name).map_err(|e| e.to_string())?;
            f.truncate().map_err(|e| e.to_string())?;
            f.write_all(&data).map_err(|e| e.to_string())?;
        } else if name == "Lib" || name == "DLLs" {
            copy_filtered(&dest.create_dir(&name).map_err(|e| e.to_string())?, &path)?;
        }
    }
    drop(dest);
    drop(programas);
    drop(root);
    fs.unmount().map_err(|e| e.to_string())?;
    println!("Python de {} copiado a /Programas/python", py.display());
    Ok(())
}

/// `cargo xtask consola [--python]`: JARVIS-OS con la Terminal abierta y `vino` instalado, que
/// tipea cada renglón que se agrega a `target/consola/entrada.txt`. Lo que llega por el puerto
/// serie sale por la salida estándar. Para desarrollar `vino` sin reiniciar QEMU: se recompila
/// en el anfitrión (`cargo xtask usuario`) y en JARVIS-OS se baja de nuevo con
/// `wget http://paquetes.jarvis/usuario/vino -O /Programas/bin/vino`.
///
/// Renglones especiales: `!salir`, `!captura` (en `target/consola.png`) y `!tecla X` (una tecla
/// con el nombre de QEMU: `ctrl-c`, `ret`…).
pub fn console(image: &Path, python: bool) -> Result<()> {
    let disk = super::fresh_disk("disco-consola.img")?;
    if python {
        copy_python(&disk)?;
    }
    super::puente::start();
    let dir = target_dir().join("consola");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let input = dir.join("entrada.txt");
    fs::write(&input, "").map_err(|e| e.to_string())?;
    let mut s = Session::start(image, &disk)?;
    s.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    s.monitor("sendkey ctrl-alt-t")?;
    s.wait_for("VENTANA_ABIERTA Terminal", STEP)?;
    thread::sleep(Duration::from_millis(500));
    s.type_text("apt install vino programas-windows")?;
    s.monitor("sendkey ret")?;
    s.wait_for("APT_INSTALADO programas-windows", STEP)?;
    println!("CONSOLA_LISTA: agregá renglones a {}", input.display());
    let mut done = 0;
    loop {
        s.pump(Duration::from_millis(300));
        let text = fs::read_to_string(&input).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        while done < lines.len() {
            let line = lines[done].trim_end();
            done += 1;
            println!("[consola] {line}");
            if line == "!salir" {
                s.quit();
                return Ok(());
            } else if line == "!captura" {
                s.screenshot(&target_dir().join("consola.png"))?;
            } else if let Some(k) = line.strip_prefix("!tecla ") {
                s.monitor(&format!("sendkey {k}"))?;
            } else {
                s.type_text(line)?;
                s.monitor("sendkey ret")?;
            }
        }
    }
}
