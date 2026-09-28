//! Sincronización entre máquinas (ADR 0007) desde el anfitrión:
//! - `cargo xtask relay [--publico] [puerto]`: el relé;
//! - `cargo xtask run2`: dos JARVIS con ventana, cada uno con su disco y su red de QEMU, unidos
//!   solo por el relé;
//! - `cargo xtask sincronizar`: la prueba punta a punta con dos QEMU sin ventana.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::{
    BOOT_MARKER, BOOT_TIMEOUT, Result, STEP, Session, arg_after, create_disk, open_fatfs, qemu,
    qemu_binary, target_dir, verify_file_exists, verify_file_on_disk,
};

const PORT: u16 = 8120;
/// El código de las pruebas y de run2 (en uso real, cada par genera el suyo en Configuración).
const TEST_CODE: &str = "JRVS2-SYNC3-PRUEB-AAAAA";

/// Levanta el relé en un hilo. `false` si el puerto está ocupado (probablemente ya hay uno).
fn start_relay(port: u16) -> Result<u16> {
    let l = jarvis_relay::bind(false, port).map_err(|e| format!("relé: puerto {port}: {e}"))?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    thread::spawn(move || jarvis_relay::serve(l));
    Ok(port)
}

pub fn relay_cmd() -> Result<()> {
    let public = std::env::args().any(|a| a == "--publico");
    let port = std::env::args()
        .skip(2)
        .find_map(|a| a.parse::<u16>().ok())
        .unwrap_or(PORT);
    let l = jarvis_relay::bind(public, port).map_err(|e| format!("relé: puerto {port}: {e}"))?;
    println!(
        "relé: escuchando en {} (Ctrl+C para cerrar)",
        l.local_addr().map_err(|e| e.to_string())?
    );
    if public {
        println!("relé: abierto a la red; los datos viajan cifrados de punta a punta");
    }
    jarvis_relay::serve(l);
    Ok(())
}

/// Un disco nuevo con la sincronización configurada y algunos archivos en `/Sincronizado`.
fn sync_disk(name: &str, machine: &str, port: u16, files: &[(&str, &str)]) -> Result<PathBuf> {
    let path = target_dir().join(name);
    create_disk(&path)?;
    let mut file = open_fatfs(&path)?;
    let fs =
        fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    let root = fs.root_dir();
    let mut cfg = root
        .open_dir("Sistema")
        .and_then(|d| d.create_file("config.ini"))
        .map_err(|e| e.to_string())?;
    write!(
        cfg,
        "teclado=us\nequipo={machine}\nsync_codigo={TEST_CODE}\nsync_rele=10.0.2.2:{port}\n"
    )
    .map_err(|e| e.to_string())?;
    drop(cfg);
    let dir = root.create_dir("Sincronizado").map_err(|e| e.to_string())?;
    for (n, content) in files {
        let mut f = dir.create_file(n).map_err(|e| e.to_string())?;
        f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    }
    drop(dir);
    drop(root);
    fs.unmount().map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn run2(image: &Path) -> Result<()> {
    let port = arg_after("--puerto")
        .and_then(|p| p.parse().ok())
        .unwrap_or(PORT);
    match start_relay(port) {
        Ok(p) => println!("relé: escuchando en 127.0.0.1:{p}"),
        Err(e) => println!("{e} (sigo: uso el relé que ya está abierto)"),
    }
    // Discos propios (no se toca target/disco.img); se crean la primera vez.
    let mut disks = Vec::new();
    for (name, machine) in [("disco-pc1.img", "PC1"), ("disco-pc2.img", "PC2")] {
        let path = target_dir().join(name);
        if !path.exists() {
            sync_disk(name, machine, port, &[])?;
        }
        disks.push(path);
    }
    let mut children = Vec::new();
    for d in &disks {
        let child = qemu(image, d, false)?
            .spawn()
            .map_err(|e| format!("no pude abrir QEMU ({}): {e}", qemu_binary().display()))?;
        children.push(child);
    }
    println!("dos JARVIS (PC1 y PC2): lo que pongas en /Sincronizado aparece en el otro");
    for mut c in children {
        let _ = c.wait();
    }
    Ok(())
}

/// Dos QEMU sin ventana: cada uno arranca con un archivo; se ven, se mandan los archivos, y un
/// renombre en uno llega al otro (el viejo, a la Papelera). Después, `fatfs` revisa los dos discos.
pub fn e2e(image: &Path) -> Result<()> {
    let port = start_relay(0)?;
    let disk_a = sync_disk(
        "disco-sync-a.img",
        "PC1",
        port,
        &[("de-a.txt", "hola desde PC1")],
    )?;
    let disk_b = sync_disk(
        "disco-sync-b.img",
        "PC2",
        port,
        &[("de-b.txt", "hola desde PC2")],
    )?;
    let mut a = Session::start(image, &disk_a)?;
    let mut b = Session::start(image, &disk_b)?;
    a.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    b.wait_for(BOOT_MARKER, BOOT_TIMEOUT)?;
    a.wait_for("SYNC_PAR PC2", STEP)?;
    a.wait_for("SYNC_RECIBIDO de-b.txt", STEP)?;
    b.wait_for("SYNC_RECIBIDO de-a.txt", STEP)?;
    println!("ok: se emparejaron y cada una recibió el archivo de la otra");

    // Renombrar en B con la terminal.
    b.monitor("sendkey ctrl-alt-t")?;
    b.wait_for("VENTANA_ABIERTA Terminal", STEP)?;
    b.type_text("mv /Sincronizado/de-a.txt /Sincronizado/renombrado.txt")?;
    b.monitor("sendkey ret")?;
    b.wait_for("TERMINAL_FIN 0", STEP)?;
    a.wait_for("SYNC_RECIBIDO renombrado.txt", STEP)?;
    a.wait_for("SYNC_PAPELERA de-a.txt", STEP)?;
    println!("ok: el renombre en PC2 llegó a PC1 (el nombre viejo, a la Papelera)");
    // Que termine de guardar el estado.
    thread::sleep(Duration::from_secs(1));
    a.quit();
    b.quit();
    drop((a, b));

    verify_file_on_disk(&disk_a, "Sincronizado/renombrado.txt", b"hola desde PC1")?;
    verify_file_on_disk(&disk_a, "Sincronizado/de-b.txt", b"hola desde PC2")?;
    verify_file_exists(&disk_a, "Papelera/de-a.txt")?;
    verify_file_on_disk(&disk_b, "Sincronizado/renombrado.txt", b"hola desde PC1")?;
    verify_file_on_disk(&disk_b, "Sincronizado/de-b.txt", b"hola desde PC2")?;
    verify_file_exists(&disk_b, "Sistema/sync.db")?;
    println!("ok: sincronización entre dos máquinas (los dos discos verificados con fatfs)");
    Ok(())
}
