//! Tests cruzados contra `fatfs`, una implementación de FAT32 independiente y madura.
//!
//! - `fatfs` formatea y escribe → nuestro FAT32 lee.
//! - Nuestro FAT32 escribe → `fatfs` lee.
//! - El espacio libre se verifica contando la FAT directamente sobre los bytes de la imagen,
//!   sin confiar en ninguna de las dos implementaciones.

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};

use fatfs::{FatType, FormatVolumeOptions, FsOptions};
use jarvis_fs::{BlockCache, FileSystem, FsError, MemDisk, Timestamp};

const SIZE: usize = 40 * 1024 * 1024;
const NOW: Timestamp = Timestamp {
    year: 2026,
    month: 9,
    day: 23,
    hour: 22,
    minute: 30,
    second: 10,
};

type Fatfs<'a> = fatfs::FileSystem<&'a mut Cursor<Vec<u8>>>;

fn blank() -> Vec<u8> {
    let mut disk = Cursor::new(vec![0u8; SIZE]);
    let opts = FormatVolumeOptions::new()
        .fat_type(FatType::Fat32)
        .bytes_per_cluster(512)
        .total_sectors((SIZE / 512) as u32)
        .volume_label(*b"JARVIS     ");
    fatfs::format_volume(&mut disk, opts).unwrap();
    disk.into_inner()
}

/// Abre la imagen con `fatfs`, corre `f` y devuelve la imagen (con lo que haya escrito).
fn with_fatfs<R>(img: Vec<u8>, f: impl FnOnce(&Fatfs<'_>) -> R) -> (R, Vec<u8>) {
    let mut disk = Cursor::new(img);
    let r = {
        let fs = fatfs::FileSystem::new(&mut disk, FsOptions::new()).unwrap();
        let r = f(&fs);
        fs.unmount().unwrap();
        r
    };
    (r, disk.into_inner())
}

/// Abre la imagen con nuestro FAT32, corre `f` y devuelve la imagen.
fn with_ours<R>(img: Vec<u8>, f: impl FnOnce(&mut FileSystem<MemDisk>) -> R) -> (R, Vec<u8>) {
    let mut fs = FileSystem::mount(MemDisk::new(img)).unwrap();
    let r = f(&mut fs);
    (r, fs.into_device().into_inner())
}

fn fatfs_read(fs: &Fatfs<'_>, path: &str) -> Vec<u8> {
    let mut data = Vec::new();
    fs.root_dir()
        .open_file(path.trim_start_matches('/'))
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    data
}

fn fatfs_names(fs: &Fatfs<'_>, path: &str) -> BTreeSet<String> {
    let path = path.trim_start_matches('/');
    let dir = if path.is_empty() {
        fs.root_dir()
    } else {
        fs.root_dir().open_dir(path).unwrap()
    };
    dir.iter()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != "." && n != "..")
        .collect()
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + seed as usize) % 251) as u8)
        .collect()
}

struct Geometry {
    reserved: usize,
    fat_sectors: usize,
    first_data: usize,
    clusters: usize,
    spc: usize,
}

fn geometry(img: &[u8]) -> Geometry {
    let u16_at = |i: usize| u16::from_le_bytes([img[i], img[i + 1]]) as usize;
    let u32_at =
        |i: usize| u32::from_le_bytes([img[i], img[i + 1], img[i + 2], img[i + 3]]) as usize;
    let (spc, reserved, fats, fat_sectors, total) = (
        img[13] as usize,
        u16_at(14),
        img[16] as usize,
        u32_at(36),
        u32_at(32),
    );
    let first_data = reserved + fats * fat_sectors;
    Geometry {
        reserved,
        fat_sectors,
        first_data,
        clusters: (total - first_data) / spc,
        spc,
    }
}

/// Clusters libres contados directamente en la FAT 1 de la imagen.
fn raw_free_clusters(img: &[u8]) -> u64 {
    let g = geometry(img);
    let fat = &img[g.reserved * 512..(g.reserved + g.fat_sectors) * 512];
    (2..g.clusters + 2)
        .filter(|&c| {
            u32::from_le_bytes(fat[c * 4..c * 4 + 4].try_into().unwrap()) & 0x0FFF_FFFF == 0
        })
        .count() as u64
}

/// Las dos copias de la FAT tienen que ser idénticas.
fn fats_match(img: &[u8]) -> bool {
    let g = geometry(img);
    let a = &img[g.reserved * 512..(g.reserved + g.fat_sectors) * 512];
    let b = &img[(g.reserved + g.fat_sectors) * 512..(g.reserved + 2 * g.fat_sectors) * 512];
    a == b
}

/// Consistencia general después de cada test.
fn check_consistency(img: &[u8], ours_free: u64) {
    assert!(fats_match(img), "las dos copias de la FAT difieren");
    let raw = raw_free_clusters(img) * 512;
    assert_eq!(
        ours_free, raw,
        "nuestro espacio libre no coincide con la FAT real"
    );
}

#[test]
fn monta_un_volumen_formateado_por_fatfs() {
    let img = blank();
    let raw_free = raw_free_clusters(&img) * 512;
    let ((label, free, total), _) = with_ours(img, |fs| {
        (fs.label().to_string(), fs.free_bytes(), fs.total_bytes())
    });
    assert_eq!(label, "JARVIS");
    assert_eq!(free, raw_free);
    assert!(total > 39 * 1024 * 1024 && total <= SIZE as u64);
}

#[test]
fn rechaza_discos_que_no_son_fat32() {
    assert_eq!(
        FileSystem::mount(MemDisk::new(vec![0u8; 1024 * 1024])).err(),
        Some(FsError::NotFat32)
    );
}

#[test]
fn lee_lo_que_escribe_fatfs() {
    let big = pattern(300_000, 7);
    let (_, img) = with_fatfs(blank(), |fs| {
        let root = fs.root_dir();
        let docs = root.create_dir("Documentos").unwrap();
        docs.create_dir("Facultad").unwrap();
        docs.create_file("Álgebra y Geometría.txt")
            .unwrap()
            .write_all("x² + y² = r²".as_bytes())
            .unwrap();
        docs.create_file("grande.bin")
            .unwrap()
            .write_all(&big)
            .unwrap();
        docs.create_file("vacío.txt").unwrap();
        let muchos = root.create_dir("Muchos").unwrap();
        for i in 0..150 {
            muchos
                .create_file(&format!("archivo número {i}.md"))
                .unwrap()
                .write_all(format!("#{i}").as_bytes())
                .unwrap();
        }
    });
    let (_, img) = with_ours(img, |fs| {
        let names: BTreeSet<String> = fs
            .list("/Documentos")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        let esperado: BTreeSet<String> = [
            "Facultad",
            "Álgebra y Geometría.txt",
            "grande.bin",
            "vacío.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(names, esperado);
        assert_eq!(
            fs.read_file("/Documentos/Álgebra y Geometría.txt").unwrap(),
            "x² + y² = r²".as_bytes()
        );
        assert_eq!(
            fs.read_file("/documentos/GRANDE.BIN").unwrap(),
            big,
            "sin distinguir mayúsculas"
        );
        assert_eq!(fs.read_file("/Documentos/vacío.txt").unwrap(), b"");
        assert!(fs.stat("/Documentos/Facultad").unwrap().is_dir);
        assert_eq!(fs.list("/Muchos").unwrap().len(), 150);
        assert_eq!(
            fs.read_file("/Muchos/archivo número 149.md").unwrap(),
            b"#149"
        );
        assert_eq!(
            fs.read_prefix("/Documentos/grande.bin", 1000).unwrap(),
            big[..1000]
        );
    });
    let free = with_ours(img.clone(), |fs| fs.free_bytes()).0;
    check_consistency(&img, free);
}

#[test]
fn fatfs_lee_lo_que_escribimos() {
    let big = pattern(200_000, 3);
    let big2 = big.clone();
    let (free, img) = with_ours(blank(), |fs| {
        fs.mkdir("/Proyectos", NOW).unwrap();
        fs.mkdir("/Proyectos/jarvis-os", NOW).unwrap();
        fs.write_file("/Proyectos/jarvis-os/README.md", b"# JARVIS-OS", NOW)
            .unwrap();
        fs.write_file(
            "/Proyectos/Álgebra y Geometría — resumen.txt",
            "ñandú".as_bytes(),
            NOW,
        )
        .unwrap();
        fs.write_file("/Proyectos/grande.bin", &big2, NOW).unwrap();
        fs.write_file("/Proyectos/vacío", b"", NOW).unwrap();
        fs.write_file("/LEAME.TXT", b"nombre corto exacto", NOW)
            .unwrap();
        fs.mkdir("/Muchos", NOW).unwrap();
        for i in 0..150 {
            fs.write_file(
                &format!("/Muchos/Documento largo número {i}.txt"),
                format!("{i}").as_bytes(),
                NOW,
            )
            .unwrap();
        }
        fs.free_bytes()
    });
    check_consistency(&img, free);
    let (_, _) = with_fatfs(img, |fs| {
        assert_eq!(
            fatfs_names(fs, "/Proyectos"),
            [
                "jarvis-os",
                "Álgebra y Geometría — resumen.txt",
                "grande.bin",
                "vacío"
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        );
        assert_eq!(
            fatfs_read(fs, "Proyectos/jarvis-os/README.md"),
            b"# JARVIS-OS"
        );
        assert_eq!(
            fatfs_read(fs, "Proyectos/Álgebra y Geometría — resumen.txt"),
            "ñandú".as_bytes()
        );
        assert_eq!(fatfs_read(fs, "Proyectos/grande.bin"), big);
        assert_eq!(fatfs_read(fs, "Proyectos/vacío"), b"");
        assert_eq!(fatfs_read(fs, "LEAME.TXT"), b"nombre corto exacto");
        assert_eq!(fatfs_names(fs, "/Muchos").len(), 150);
        assert_eq!(
            fatfs_read(fs, "Muchos/Documento largo número 77.txt"),
            b"77"
        );
        // Los alias cortos tienen que ser únicos dentro de la carpeta.
        let shorts: Vec<String> = fs
            .root_dir()
            .open_dir("Muchos")
            .unwrap()
            .iter()
            .map(|e| e.unwrap().short_file_name())
            .collect();
        let unicos: BTreeSet<&String> = shorts.iter().collect();
        assert_eq!(
            unicos.len(),
            shorts.len(),
            "alias cortos repetidos: {shorts:?}"
        );
    });
}

#[test]
fn las_fechas_quedan_bien_guardadas() {
    let (_, img) = with_ours(blank(), |fs| {
        fs.write_file("/fecha.txt", b"x", NOW).unwrap()
    });
    let ((year, month, day, hour, min, sec), img) = with_fatfs(img, |fs| {
        let e = fs
            .root_dir()
            .iter()
            .map(|e| e.unwrap())
            .find(|e| e.file_name() == "fecha.txt")
            .unwrap();
        let m = e.modified();
        (
            m.date.year,
            m.date.month,
            m.date.day,
            m.time.hour,
            m.time.min,
            m.time.sec,
        )
    });
    assert_eq!(
        (year, month, day, hour, min, sec),
        (2026, 9, 23, 22, 30, 10)
    );
    let (stat, _) = with_ours(img, |fs| fs.stat("/fecha.txt").unwrap());
    assert_eq!(stat.modified, NOW);
    assert_eq!(stat.created, NOW);
}

#[test]
fn reemplazar_contenido() {
    let (free, img) = with_ours(blank(), |fs| {
        fs.write_file("/a.txt", &pattern(10_000, 1), NOW).unwrap();
        fs.write_file("/a.txt", b"corto", NOW).unwrap();
        fs.write_file("/b.txt", b"corto", NOW).unwrap();
        fs.write_file("/b.txt", &pattern(50_000, 2), NOW).unwrap();
        fs.free_bytes()
    });
    check_consistency(&img, free);
    with_fatfs(img, |fs| {
        assert_eq!(fatfs_read(fs, "a.txt"), b"corto");
        assert_eq!(fatfs_read(fs, "b.txt"), pattern(50_000, 2));
    });
}

/// Cluster al que apunta la entrada ".." de la carpeta que empieza en `dir_cluster`.
fn raw_dotdot(img: &[u8], dir_cluster: u32) -> u32 {
    let g = geometry(img);
    let off = (g.first_data + (dir_cluster as usize - 2) * g.spc) * 512 + 32;
    let e = &img[off..off + 32];
    assert_eq!(&e[0..11], b"..         ");
    ((u16::from_le_bytes([e[20], e[21]]) as u32) << 16) | u16::from_le_bytes([e[26], e[27]]) as u32
}

#[test]
fn renombrar_y_mover() {
    let ((a_cluster, b_cluster, free), img) = with_ours(blank(), |fs| {
        fs.mkdir("/A", NOW).unwrap();
        fs.mkdir("/B", NOW).unwrap();
        fs.write_file("/A/nota.txt", b"contenido", NOW).unwrap();
        fs.rename("/A/nota.txt", "Nota Final.txt").unwrap();
        fs.rename("/A/Nota Final.txt", "NOTA FINAL.TXT").unwrap(); // solo cambia mayúsculas
        assert_eq!(
            fs.rename("/A/NOTA FINAL.TXT", "otra/cosa"),
            Err(FsError::InvalidName)
        );
        fs.move_to("/A", "/B").unwrap();
        assert_eq!(fs.move_to("/B", "/B/A"), Err(FsError::MoveIntoItself));
        let a = fs.stat("/B/A").unwrap().first_cluster;
        let b = fs.stat("/B").unwrap().first_cluster;
        // Y de vuelta a la raíz: ".." tiene que volver a 0.
        fs.write_file("/B/A/extra.md", b"extra", NOW).unwrap();
        fs.move_to("/B/A/extra.md", "/").unwrap();
        (a, b, fs.free_bytes())
    });
    check_consistency(&img, free);
    assert_eq!(
        raw_dotdot(&img, a_cluster),
        b_cluster,
        "\"..\" tiene que apuntar a la carpeta nueva"
    );
    let (_, img) = with_fatfs(img, |fs| {
        assert_eq!(
            fatfs_names(fs, "/"),
            ["B", "extra.md"].iter().map(|s| s.to_string()).collect()
        );
        assert_eq!(fatfs_read(fs, "B/A/NOTA FINAL.TXT"), b"contenido");
        assert_eq!(fatfs_read(fs, "extra.md"), b"extra");
    });
    let (_, img) = with_ours(img, |fs| {
        fs.move_to("/B/A", "/").unwrap();
    });
    assert_eq!(raw_dotdot(&img, a_cluster), 0, "en la raíz, \"..\" es 0");
}

#[test]
fn borrar_recursivo_no_pierde_espacio() {
    let img = blank();
    let before = raw_free_clusters(&img) * 512;
    let (free, img) = with_ours(img, |fs| {
        fs.mkdir("/Arbol", NOW).unwrap();
        for i in 0..5 {
            fs.mkdir(&format!("/Arbol/rama {i}"), NOW).unwrap();
            for j in 0..20 {
                fs.write_file(
                    &format!("/Arbol/rama {i}/hoja {j}.txt"),
                    &pattern(3000, j),
                    NOW,
                )
                .unwrap();
            }
        }
        assert!(fs.free_bytes() < before);
        fs.remove("/Arbol").unwrap();
        assert_eq!(fs.remove("/"), Err(FsError::RootNotAllowed));
        assert_eq!(fs.remove("/Arbol"), Err(FsError::NotFound));
        fs.free_bytes()
    });
    assert_eq!(
        free, before,
        "después de borrar tiene que quedar el mismo espacio libre"
    );
    check_consistency(&img, free);
    with_fatfs(img, |fs| assert!(fatfs_names(fs, "/").is_empty()));
}

#[test]
fn disco_lleno_no_deja_basura() {
    let (free, img) = with_ours(blank(), |fs| {
        let libre = fs.free_bytes() as usize;
        assert_eq!(
            fs.write_file("/enorme.bin", &vec![1u8; libre + 512], NOW),
            Err(FsError::NoSpace)
        );
        assert_eq!(
            fs.free_bytes() as usize,
            libre,
            "un intento fallido no puede consumir espacio"
        );
        assert!(!fs.exists("/enorme.bin"));
        fs.write_file("/justo.bin", &vec![2u8; libre - 64 * 1024], NOW)
            .unwrap();
        fs.free_bytes()
    });
    check_consistency(&img, free);
}

#[test]
fn reutiliza_entradas_borradas() {
    let (free, img) = with_ours(blank(), |fs| {
        fs.mkdir("/Temp", NOW).unwrap();
        let mut despues_de_la_primera_vuelta = 0;
        for vuelta in 0..5 {
            for i in 0..60 {
                fs.write_file(
                    &format!("/Temp/temporal con nombre largo {i}.txt"),
                    b"x",
                    NOW,
                )
                .unwrap();
            }
            for i in 0..60 {
                fs.remove(&format!("/Temp/temporal con nombre largo {i}.txt"))
                    .unwrap();
            }
            if vuelta == 0 {
                despues_de_la_primera_vuelta = fs.free_bytes();
            }
        }
        assert_eq!(
            fs.free_bytes(),
            despues_de_la_primera_vuelta,
            "la carpeta no puede crecer sin límite"
        );
        fs.free_bytes()
    });
    check_consistency(&img, free);
}

#[test]
fn errores_esperables() {
    with_ours(blank(), |fs| {
        fs.mkdir("/Documentos", NOW).unwrap();
        fs.write_file("/Documentos/a.txt", b"a", NOW).unwrap();
        assert_eq!(fs.mkdir("/documentos", NOW), Err(FsError::AlreadyExists));
        assert_eq!(
            fs.create_file("/Documentos/A.TXT", b"", NOW),
            Err(FsError::AlreadyExists)
        );
        assert_eq!(fs.mkdir("/mal:nombre", NOW), Err(FsError::InvalidName));
        assert_eq!(
            fs.write_file("/Documentos", b"", NOW),
            Err(FsError::IsADirectory)
        );
        assert_eq!(
            fs.list("/Documentos/a.txt").err(),
            Some(FsError::NotADirectory)
        );
        assert_eq!(
            fs.read_file("/Documentos").err(),
            Some(FsError::IsADirectory)
        );
        assert_eq!(fs.list("/no/existe").err(), Some(FsError::NotFound));
        assert_eq!(
            fs.list("/Documentos/../..").err(),
            Some(FsError::InvalidName)
        );
        assert_eq!(fs.rename("/Documentos/a.txt", "b.txt"), Ok(()));
        fs.write_file("/Documentos/c.txt", b"c", NOW).unwrap();
        assert_eq!(
            fs.rename("/Documentos/b.txt", "C.txt"),
            Err(FsError::AlreadyExists)
        );
        assert_eq!(
            fs.read_file("/Documentos/b.txt").unwrap(),
            b"a",
            "un rename fallido no toca nada"
        );
    });
}

#[test]
fn copiar_archivos_y_carpetas() {
    let (free, img) = with_ours(blank(), |fs| {
        fs.mkdir("/Proyecto", NOW).unwrap();
        fs.mkdir("/Proyecto/src", NOW).unwrap();
        fs.write_file("/Proyecto/src/main.rs", &pattern(5000, 3), NOW)
            .unwrap();
        fs.write_file("/Proyecto/LÉAME.md", b"hola", NOW).unwrap();
        fs.write_file("/Proyecto/vacío.txt", b"", NOW).unwrap();
        fs.copy("/Proyecto", "/Copia de Proyecto", NOW).unwrap();
        fs.copy("/Proyecto/LÉAME.md", "/léame (2).md", NOW).unwrap();
        assert_eq!(
            fs.copy("/Proyecto", "/Proyecto/src/adentro", NOW),
            Err(FsError::MoveIntoItself)
        );
        assert_eq!(
            fs.copy("/Proyecto/LÉAME.md", "/Copia de Proyecto/léame.md", NOW),
            Err(FsError::AlreadyExists),
            "FAT no distingue mayúsculas"
        );
        assert_eq!(fs.copy("/no-existe", "/x", NOW), Err(FsError::NotFound));
        fs.free_bytes()
    });
    check_consistency(&img, free);
    with_fatfs(img, |fs| {
        assert_eq!(
            fatfs_read(fs, "/Copia de Proyecto/src/main.rs"),
            pattern(5000, 3)
        );
        assert_eq!(fatfs_read(fs, "/léame (2).md"), b"hola");
        assert!(fatfs_read(fs, "/Copia de Proyecto/vacío.txt").is_empty());
        assert!(fatfs_read(fs, "/Proyecto/src/main.rs") == pattern(5000, 3));
    });
}

#[test]
fn copiar_sin_espacio_no_deja_copias_a_medias() {
    let (free, img) = with_ours(blank(), |fs| {
        let big = fs.free_bytes() as usize * 6 / 10;
        fs.mkdir("/Grande", NOW).unwrap();
        fs.write_file("/Grande/a.bin", &vec![7u8; big / 2], NOW)
            .unwrap();
        fs.write_file("/Grande/b.bin", &vec![8u8; big / 2], NOW)
            .unwrap();
        let before = fs.free_bytes();
        assert_eq!(fs.copy("/Grande", "/Grande 2", NOW), Err(FsError::NoSpace));
        assert!(!fs.exists("/Grande 2"));
        assert_eq!(
            fs.free_bytes(),
            before,
            "se liberó lo que se llegó a copiar"
        );
        fs.free_bytes()
    });
    check_consistency(&img, free);
}

#[test]
fn con_cache_de_sectores_el_disco_queda_igual() {
    let img = blank();
    let mut fs = FileSystem::mount(BlockCache::new(MemDisk::new(img), 64)).unwrap();
    for i in 0..30 {
        fs.write_file(&format!("/nota {i}.txt"), &pattern(1500, i), NOW)
            .unwrap();
    }
    fs.mkdir("/Carpeta", NOW).unwrap();
    fs.move_to("/nota 3.txt", "/Carpeta").unwrap();
    fs.remove("/nota 4.txt").unwrap();
    for _ in 0..3 {
        assert_eq!(fs.list("/").unwrap().len(), 29);
    }
    let free = fs.free_bytes();
    let cache = fs.into_device();
    assert!(cache.hits > 0, "la caché se usó");
    let img = cache.into_inner().into_inner();
    check_consistency(&img, free);
    with_fatfs(img, |fs| {
        assert_eq!(fatfs_read(fs, "/Carpeta/nota 3.txt"), pattern(1500, 3));
        assert!(!fatfs_names(fs, "/").contains("nota 4.txt"));
    });
}
