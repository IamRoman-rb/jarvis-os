//! El instalador (K13, ADR 0011): copia JARVIS-OS del medio de arranque a un disco vacío.
//!
//! El medio de arranque (el pendrive, o la imagen de QEMU) es un disco GPT con una partición de
//! sistema EFI que armó el crate `bootloader`: un FAT16 con el cargador UEFI y el kernel. El
//! instalador **no lee archivos** de ahí (jarvis-fs solo entiende FAT32): copia la partición
//! entera, sector por sector, a la ESP del disco nuevo. Después crea la partición de datos de
//! JARVIS y la formatea en FAT32 con las carpetas de siempre.
//!
//! Solo escribe en discos vacíos (sin MBR ni GPT): se vuelve a verificar justo antes de escribir,
//! aunque Configuración ya lo haya mostrado como elegible.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use jarvis_desktop::DiskInfo;
use jarvis_drivers::gpt::{self, EFI_SYSTEM, Guid, JARVIS_DATA, NewPartition, PartitionDevice};
use jarvis_fs::{FileSystem, SECTOR_SIZE};

use crate::{entropy, serial_println, storage};

/// Dónde está el arranque de JARVIS: (disco, primer sector y sectores de su ESP).
fn find_boot(disks: &mut [storage::Found]) -> Option<(usize, u64, u64)> {
    for (i, d) in disks.iter_mut().enumerate() {
        let Some(parts) = gpt::read(&mut *d.disk) else {
            continue;
        };
        for p in parts.iter().filter(|p| p.kind == EFI_SYSTEM) {
            let mut sector = vec![0u8; SECTOR_SIZE];
            if d.disk.read(p.first, &mut sector).is_ok() && gpt::is_jarvis_boot(&sector) {
                return Some((i, p.first, p.sectors()));
            }
        }
    }
    None
}

/// Los discos (menos el del sistema) para Configuración → Hardware. El medio de arranque queda
/// primero (se reordena la lista guardada, así los números no cambian después).
pub fn disks() -> Vec<DiskInfo> {
    storage::with_spare(|disks| {
        if let Some((b, _, _)) = find_boot(disks) {
            disks[..=b].rotate_right(1);
        }
        let boot = find_boot(disks).map(|b| b.0);
        disks
            .iter_mut()
            .enumerate()
            .map(|(i, d)| DiskInfo {
                name: d.name.clone(),
                mib: d.disk.sector_count() / 2048,
                blank: gpt::is_blank(&mut *d.disk).unwrap_or(false),
                boot_medium: boot == Some(i),
            })
            .collect()
    })
}

/// Un GUID al azar (versión 4), para el disco y cada partición.
fn random_guid() -> Guid {
    let mut g = [0u8; 16];
    entropy::fill(&mut g);
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    Guid(g)
}

/// Sectores que se copian por vez (64 KiB).
const CHUNK: u64 = 128;
/// Lo mínimo para la partición de datos (64 MiB).
const MIN_DATA: u64 = 64 * 2048;

/// Instala en el disco número `target` de la lista de `disks()`.
pub fn install(target: usize) -> Result<String, String> {
    storage::with_spare(|disks| {
        let (boot, esp_first, esp_sectors) =
            find_boot(disks).ok_or("no se encontró el medio de arranque de JARVIS-OS")?;
        if target == boot || target >= disks.len() {
            return Err(String::from("ese disco no se puede usar"));
        }
        let name = disks[target].name.clone();
        // La verificación que importa: justo antes de escribir.
        if gpt::is_blank(&mut *disks[target].disk) != Ok(true) {
            return Err(format!("{name} no está vacío: no se toca"));
        }
        serial_println!("INSTALAR_INICIO {name}");
        let total = disks[target].disk.sector_count();
        // La ESP del disco nuevo, del tamaño de la del medio redondeado a 1 MiB.
        let esp = esp_sectors.div_ceil(2048) * 2048;
        if total < esp + MIN_DATA + 4096 {
            return Err(format!("{name} es muy chico"));
        }
        let uniques = [random_guid(), random_guid(), random_guid()];
        let layout = gpt::create(
            total,
            &[
                NewPartition {
                    kind: EFI_SYSTEM,
                    sectors: esp,
                    name: "JARVIS-OS arranque",
                },
                NewPartition {
                    kind: JARVIS_DATA,
                    sectors: 0,
                    name: "JARVIS-OS",
                },
            ],
            &uniques,
        )
        .ok_or("no entra la tabla de particiones")?;
        let (new_esp, data) = (layout.partitions[0].clone(), layout.partitions[1].clone());

        // 1. Copiar la ESP del medio, sector por sector.
        let mut buf = vec![0u8; CHUNK as usize * SECTOR_SIZE];
        let mut done = 0;
        while done < esp_sectors {
            let n = CHUNK.min(esp_sectors - done);
            let bytes = &mut buf[..n as usize * SECTOR_SIZE];
            disks[boot]
                .disk
                .read(esp_first + done, bytes)
                .map_err(|_| "no se pudo leer el medio de arranque")?;
            disks[target]
                .disk
                .write(new_esp.first + done, bytes)
                .map_err(|_| format!("no se pudo escribir en {name}"))?;
            done += n;
        }
        serial_println!("INSTALAR_ARRANQUE {} MiB copiados", esp_sectors / 2048);

        // 2. La tabla de particiones (después de la copia: si algo falla antes, el disco sigue
        //    "vacío" para el próximo intento).
        for (lba, bytes) in &layout.writes {
            disks[target]
                .disk
                .write(*lba, bytes)
                .map_err(|_| format!("no se pudo escribir la GPT en {name}"))?;
        }

        // 3. La partición de datos: FAT32 con las carpetas de siempre.
        let mut part = PartitionDevice::new(&mut *disks[target].disk, data.first, data.sectors());
        jarvis_fs::format_fat32(&mut part, "JARVIS")
            .map_err(|_| "no se pudo formatear la partición de datos")?;
        let mut fs = FileSystem::mount(part).map_err(|e| format!("montaje: {e}"))?;
        storage::populate(
            &mut fs,
            "JARVIS-OS instalado. Tus archivos quedan en este disco.\n",
        );
        serial_println!("INSTALAR_LISTO {name}");
        Ok(format!(
            "Listo. Apagá, sacá el pendrive y arrancá desde {name} (menú de arranque del firmware)."
        ))
    })
}
