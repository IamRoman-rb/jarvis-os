//! El lector de ZIP propio (`term/zip.rs`, para `winget install`) contra la crate `zip`: lo que
//! una arma, el otro lo lee igual; y los ZIP dañados o con nombres peligrosos se rechazan.

use std::io::{Cursor, Write};

use jarvis_desktop::term::zip as ours;
use zip::write::SimpleFileOptions;

fn build(files: &[(&str, &[u8], zip::CompressionMethod)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, method) in files {
        if name.ends_with('/') {
            w.add_directory(*name, SimpleFileOptions::default())
                .unwrap();
            continue;
        }
        w.start_file(
            *name,
            SimpleFileOptions::default()
                .compression_method(*method)
                .unix_permissions(0o755),
        )
        .unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn lo_que_arma_la_crate_zip_se_lee_igual() {
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let data = build(&[
        ("programa-1.0/", b"", zip::CompressionMethod::Stored),
        (
            "programa-1.0/rg.exe",
            b"MZ\x90\x00 un exe de mentira",
            zip::CompressionMethod::Deflated,
        ),
        (
            "programa-1.0/LEAME.txt",
            b"hola",
            zip::CompressionMethod::Stored,
        ),
        (
            "programa-1.0/datos/grande.bin",
            &big,
            zip::CompressionMethod::Deflated,
        ),
        (
            "programa-1.0/canción.txt",
            "ñandú".as_bytes(),
            zip::CompressionMethod::Deflated,
        ),
    ]);
    let entries = ours::entries(&data).unwrap();
    assert_eq!(entries.len(), 5);
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].name, "programa-1.0");
    let files = ours::extract_all(&data).unwrap();
    let get = |n: &str| {
        files
            .iter()
            .find(|(e, _)| e.name == n)
            .map(|(_, d)| d.clone())
    };
    assert_eq!(
        get("programa-1.0/rg.exe").unwrap(),
        b"MZ\x90\x00 un exe de mentira"
    );
    assert_eq!(get("programa-1.0/LEAME.txt").unwrap(), b"hola");
    assert_eq!(get("programa-1.0/datos/grande.bin").unwrap(), big);
    assert_eq!(get("programa-1.0/canción.txt").unwrap(), "ñandú".as_bytes());
    // Los permisos de Unix que guardó.
    let exe = entries.iter().find(|e| e.name.ends_with("rg.exe")).unwrap();
    assert_eq!(exe.unix_mode, Some(0o755));
}

#[test]
fn un_byte_cambiado_se_detecta() {
    let mut data = build(&[(
        "a.txt",
        b"contenido que se va a romper",
        zip::CompressionMethod::Stored,
    )]);
    let at = data.windows(9).position(|w| w == b"contenido").unwrap();
    data[at] ^= 0xFF;
    assert_eq!(ours::extract_all(&data).unwrap_err(), ours::Error::Corrupt);
}

#[test]
fn nombres_peligrosos_y_basura() {
    let data = build(&[(
        "../../Sistema/config.ini",
        b"x",
        zip::CompressionMethod::Stored,
    )]);
    assert!(matches!(ours::entries(&data), Err(ours::Error::BadName(_))));
    assert_eq!(
        ours::entries(b"no soy un zip").unwrap_err(),
        ours::Error::NotZip
    );
    let cut = build(&[("a.txt", b"hola mundo", zip::CompressionMethod::Deflated)]);
    assert!(ours::extract_all(&cut[..cut.len() / 2]).is_err());
}

#[test]
fn el_paquete_de_prueba_de_winget() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../paquetes/winget/hola-win.zip");
    let data = std::fs::read(path).unwrap();
    let files = ours::extract_all(&data).unwrap();
    let exe = files
        .iter()
        .find(|(e, _)| e.name == "hola-win-1.0/hola-win.exe")
        .unwrap();
    assert!(exe.1.starts_with(b"MZ"));
}
