//! squashfs contra `backhand` (una implementación independiente): lo que arma backhand, esta
//! crate lo lee igual (carpetas, archivos chicos y grandes con fragmentos, enlaces, permisos y
//! dueños), con xz y con gzip, y con bloques de varios tamaños.

use std::io::Cursor;

use backhand::compression::Compressor;
use backhand::{FilesystemCompressor, FilesystemWriter, NodeHeader};
use jarvis_squashfs::{Compression, Error, Kind, SquashFs};

fn header(mode: u16, uid: u32, gid: u32) -> NodeHeader {
    NodeHeader {
        permissions: mode,
        uid,
        gid,
        mtime: 0,
    }
}

/// Un archivo de `n` bytes que no se comprime del todo (así hay varios bloques de verdad).
fn pseudo(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if i % 3 == 0 {
                b'a' + (x % 26) as u8
            } else {
                (x >> 8) as u8
            }
        })
        .collect()
}

fn build(compressor: Compressor, block_size: u32, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut w = FilesystemWriter::default();
    w.set_compressor(FilesystemCompressor::new(compressor, None).unwrap());
    w.set_block_size(block_size);
    w.push_dir("meta", header(0o755, 0, 0)).unwrap();
    w.push_dir("bin", header(0o755, 0, 0)).unwrap();
    w.push_dir("usr", header(0o755, 0, 0)).unwrap();
    w.push_dir("usr/lib", header(0o755, 0, 0)).unwrap();
    for (path, data) in files {
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        w.push_file(Cursor::new(data.clone()), path, header(mode, 1000, 27))
            .unwrap();
    }
    w.push_symlink("../bin/hola", "usr/lib/hola-enlace", header(0o777, 0, 0))
        .unwrap();
    let mut out = Cursor::new(Vec::new());
    w.write(&mut out).unwrap();
    out.into_inner()
}

fn check(compressor: Compressor, expected: Compression, block_size: u32) {
    let yaml = b"name: hola\nversion: '1.0'\napps:\n  hola:\n    command: bin/hola\n".to_vec();
    let files = vec![
        ("meta/snap.yaml", yaml.clone()),
        ("bin/hola", pseudo(3 * block_size as usize + 777, 1)),
        ("usr/lib/chico.txt", b"chiquito".to_vec()),
        ("usr/lib/vacio", Vec::new()),
        ("usr/lib/justo", pseudo(block_size as usize, 2)),
        ("usr/lib/ceros", vec![0u8; 2 * block_size as usize + 5]),
    ];
    let img = build(compressor, block_size, &files);
    let fs = SquashFs::open(&img).unwrap();
    assert_eq!(fs.compression, expected);
    assert_eq!(fs.block_size, block_size);
    let entries = fs.entries().unwrap();
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    for p in [
        "/",
        "/meta",
        "/bin",
        "/usr",
        "/usr/lib",
        "/meta/snap.yaml",
        "/bin/hola",
    ] {
        assert!(paths.contains(&p), "falta {p}: {paths:?}");
    }
    for (path, data) in &files {
        let e = fs.find(&entries, &format!("/{path}")).unwrap();
        assert_eq!(e.kind, Kind::File);
        assert_eq!(e.size, data.len() as u64, "{path}");
        assert_eq!(&fs.read(e).unwrap(), data, "{path}");
        assert_eq!((e.uid, e.gid), (1000, 27), "{path}");
    }
    let hola = fs.find(&entries, "/bin/hola").unwrap();
    assert_eq!(hola.mode, 0o755);
    let link = fs.find(&entries, "/usr/lib/hola-enlace").unwrap();
    assert_eq!(link.kind, Kind::Symlink("../bin/hola".into()));
    assert_eq!(
        jarvis_squashfs::resolve_link(&link.path, "../bin/hola").as_deref(),
        Some("/usr/bin/hola")
    );
    assert_eq!(fs.find(&entries, "/bin").unwrap().kind, Kind::Dir);
    // Leer una carpeta como archivo es un error, no un pánico.
    assert!(fs.read(fs.find(&entries, "/bin").unwrap()).is_err());
}

#[test]
fn xz_como_los_snaps() {
    check(Compressor::Xz, Compression::Xz, 131_072);
}

#[test]
fn gzip_y_bloques_chicos() {
    check(Compressor::Gzip, Compression::Gzip, 4096);
}

#[test]
fn muchos_archivos_en_una_carpeta() {
    // Más de 256 entradas: el directorio se parte en varias cabeceras y bloques de metadatos.
    let names: Vec<String> = (0..600)
        .map(|i| format!("usr/lib/archivo-{i:04}.txt"))
        .collect();
    let files: Vec<(&str, Vec<u8>)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), format!("contenido {i}").into_bytes()))
        .collect();
    let img = build(Compressor::Xz, 131_072, &files);
    let fs = SquashFs::open(&img).unwrap();
    let entries = fs.entries().unwrap();
    for (i, n) in names.iter().enumerate() {
        let e = fs.find(&entries, &format!("/{n}")).unwrap();
        assert_eq!(fs.read(e).unwrap(), format!("contenido {i}").into_bytes());
    }
}

#[test]
fn imagenes_rotas_no_entran_en_panico() {
    let img = build(
        Compressor::Xz,
        131_072,
        &[
            ("bin/hola", pseudo(200_000, 3)),
            ("meta/snap.yaml", b"name: x".to_vec()),
        ],
    );
    // Cortada.
    for cut in [100, img.len() / 3, img.len() / 2, img.len() - 10] {
        if let Ok(fs) = SquashFs::open(&img[..cut])
            && let Ok(entries) = fs.entries()
        {
            for e in &entries {
                let _ = fs.read(e);
            }
        }
    }
    // Con bytes cambiados al azar.
    let mut x = 12345u32;
    for _ in 0..200 {
        let mut bad = img.clone();
        for _ in 0..4 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let at = (x as usize) % bad.len();
            bad[at] ^= (x >> 8) as u8 | 1;
        }
        if let Ok(fs) = SquashFs::open(&bad)
            && let Ok(entries) = fs.entries()
        {
            for e in entries.iter().take(20) {
                let _ = fs.read(e);
            }
        }
    }
    assert!(matches!(
        SquashFs::open(b"hsqs corto"),
        Err(Error::NotSquashfs)
    ));
}
