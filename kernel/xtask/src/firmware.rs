//! El firmware de las placas (K14, ADR 0012): se baja, se verifica y va en el ramdisk.
//!
//! El firmware de la RTL8821CE es un programa de Realtek que no está en el repositorio (su
//! licencia permite redistribuirlo, no modificarlo). Se baja una vez de linux-firmware, de un
//! commit fijo, y se verifica su SHA-256: si el archivo cambiara, no se usa. Queda en caché en
//! `target/firmware/` y va al ramdisk de la imagen de arranque (`jarvis_drivers::ramdisk`), que
//! el bootloader carga con el kernel. Sin red la primera vez, la imagen sale sin firmware y
//! JARVIS-OS arranca igual, sin Wi-Fi.

use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::target_dir;

/// (nombre en el ramdisk, URL, SHA-256).
const FILES: [(&str, &str, &str); 1] = [(
    "rtw88/rtw8821c_fw.bin",
    "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/rtw88/rtw8821c_fw.bin?id=d947e4e8e314e9254a1242dc1a5d9cede2cce33d",
    "2ef409bc418549fcf294061dd0cae1fc22fd9da79b60524950b25de18732f3f0",
)];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Un archivo de firmware: de la caché o bajado (y verificado).
fn get(name: &str, url: &str, sha256: &str) -> Result<Vec<u8>, String> {
    let cache = target_dir().join("firmware").join(name);
    if let Ok(data) = fs::read(&cache)
        && hex(&Sha256::digest(&data)) == sha256
    {
        return Ok(data);
    }
    eprintln!("firmware: bajando {name} de linux-firmware...");
    let data = ureq::get(url)
        .header("User-Agent", "jarvis-os-xtask")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let got = hex(&Sha256::digest(&data));
    if got != sha256 {
        return Err(format!("{name}: el SHA-256 no coincide ({got})"));
    }
    if let Some(dir) = cache.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&cache, &data).map_err(|e| e.to_string())?;
    Ok(data)
}

/// Arma `target/ramdisk.bin` con los firmwares que se pudieron conseguir.
pub fn ramdisk() -> Option<PathBuf> {
    let mut files = Vec::new();
    for (name, url, sha) in FILES {
        // Además del hash: que el driver lo entienda (cabecera y tamaños de las secciones).
        let checked = get(name, url, sha).and_then(|data| {
            let h = jarvis_drivers::rtw88::fw::parse(&data).map_err(|e| e.to_string())?;
            eprintln!(
                "firmware: {name} v{}.{}.{} (DMEM {} B, IMEM {} B, EMEM {} B)",
                h.version, h.sub_version, h.sub_index, h.dmem_size, h.imem_size, h.emem_size
            );
            Ok(data)
        });
        match checked {
            Ok(data) => files.push((name, data)),
            Err(e) => eprintln!("firmware: sin {name} ({e}); la imagen sale sin Wi-Fi"),
        }
    }
    if files.is_empty() {
        return None;
    }
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (*n, d.as_slice())).collect();
    let path = target_dir().join("ramdisk.bin");
    fs::write(&path, jarvis_drivers::ramdisk::build(&refs)).ok()?;
    Some(path)
}
