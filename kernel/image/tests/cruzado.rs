//! Tests cruzados: las imágenes las codifica otra implementación (la crate `png`, o un
//! escritor mínimo de acá para el entrelazado) y el resultado se compara con lo que decodifica
//! la referencia. Así un error de lectura de la norma no se esconde detrás de uno igual al
//! escribir.

use jarvis_image::{Error, Image, png as jpng};
use png::{BitDepth, ColorType, Filter, Transformations};

/// Números al azar reproducibles (un LCG alcanza para tests).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 56) as u8
    }
}

/// Datos de píxel: la mitad al azar y la mitad con franjas (que los filtros comprimen).
fn samples(len: usize, seed: u64) -> Vec<u8> {
    let mut r = Lcg(seed);
    (0..len)
        .map(|i| {
            if (i / 97) % 2 == 0 {
                r.next()
            } else {
                (i % 29) as u8 * 8
            }
        })
        .collect()
}

fn channels(c: ColorType) -> usize {
    match c {
        ColorType::Grayscale | ColorType::Indexed => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
    }
}

/// Lo que decodifica la crate `png`, llevado a RGBA de 8 bits.
fn reference(file: &[u8]) -> Image {
    let mut dec = png::Decoder::new(std::io::Cursor::new(file));
    dec.set_transformations(Transformations::normalize_to_color8());
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &buf[y * info.line_size..][..info.line_size];
        for x in 0..w {
            let p: [u8; 4] = match info.color_type {
                ColorType::Grayscale => [row[x], row[x], row[x], 255],
                ColorType::GrayscaleAlpha => [row[2 * x], row[2 * x], row[2 * x], row[2 * x + 1]],
                ColorType::Rgb => [row[3 * x], row[3 * x + 1], row[3 * x + 2], 255],
                ColorType::Rgba => [row[4 * x], row[4 * x + 1], row[4 * x + 2], row[4 * x + 3]],
                ColorType::Indexed => unreachable!("EXPAND saca la paleta"),
            };
            rgba.extend_from_slice(&p);
        }
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

#[allow(clippy::too_many_arguments)]
fn encode(
    w: u32,
    h: u32,
    color: ColorType,
    depth: BitDepth,
    filter: Filter,
    palette: Option<&[u8]>,
    trns: Option<&[u8]>,
    data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.set_filter(filter);
        if let Some(p) = palette {
            enc.set_palette(p.to_vec());
        }
        if let Some(t) = trns {
            enc.set_trns(t.to_vec());
        }
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(data).unwrap();
    }
    out
}

#[test]
fn todas_las_combinaciones_de_color_y_bits() {
    let combos: &[(ColorType, &[BitDepth])] = &[
        (
            ColorType::Grayscale,
            &[
                BitDepth::One,
                BitDepth::Two,
                BitDepth::Four,
                BitDepth::Eight,
                BitDepth::Sixteen,
            ],
        ),
        (ColorType::Rgb, &[BitDepth::Eight, BitDepth::Sixteen]),
        (
            ColorType::Indexed,
            &[
                BitDepth::One,
                BitDepth::Two,
                BitDepth::Four,
                BitDepth::Eight,
            ],
        ),
        (
            ColorType::GrayscaleAlpha,
            &[BitDepth::Eight, BitDepth::Sixteen],
        ),
        (ColorType::Rgba, &[BitDepth::Eight, BitDepth::Sixteen]),
    ];
    let filters = [
        Filter::NoFilter,
        Filter::Sub,
        Filter::Up,
        Filter::Avg,
        Filter::Paeth,
        Filter::Adaptive,
    ];
    let mut seed = 1;
    for &(color, depths) in combos {
        for &depth in depths {
            for &filter in &filters {
                // Anchos raros: las filas de 1, 2 y 4 bits no terminan en un byte justo.
                for (w, h) in [(1u32, 1u32), (13, 7), (64, 33)] {
                    seed += 1;
                    let bits = channels(color) * depth as usize;
                    let row = (w as usize * bits).div_ceil(8);
                    let data = samples(row * h as usize, seed);
                    // La paleta tiene todas las entradas posibles: ningún índice se sale.
                    let entries = 1usize << (depth as usize).min(8);
                    let palette: Vec<u8> = (0..entries * 3).map(|i| (i * 37 % 256) as u8).collect();
                    let trns: Option<Vec<u8>> = match color {
                        ColorType::Indexed => {
                            Some((0..entries.min(5)).map(|i| (i * 60) as u8).collect())
                        }
                        ColorType::Grayscale => Some(vec![0, 0]),
                        ColorType::Rgb => Some(vec![0, 0, 0, 0, 0, 0]),
                        _ => None,
                    };
                    let file = encode(
                        w,
                        h,
                        color,
                        depth,
                        filter,
                        (color == ColorType::Indexed).then_some(&palette[..]),
                        trns.as_deref(),
                        &data,
                    );
                    let mine = jpng::decode(&file)
                        .unwrap_or_else(|e| panic!("{color:?} {depth:?} {filter:?} {w}x{h}: {e}"));
                    assert_eq!(
                        mine,
                        reference(&file),
                        "{color:?} {depth:?} {filter:?} {w}x{h}"
                    );
                }
            }
        }
    }
}

/// Un escritor mínimo de PNG entrelazado (filtro 0 en todas las filas): la crate `png` no
/// escribe Adam7.
fn write_interlaced_rgba(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    const ADAM7: [(usize, usize, usize, usize); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    let mut raw = Vec::new();
    for (x0, y0, dx, dy) in ADAM7 {
        let pw = (w + dx - 1 - x0) / dx;
        let ph = (h + dy - 1 - y0) / dy;
        if pw == 0 || ph == 0 {
            continue;
        }
        for py in 0..ph {
            raw.push(0);
            for px in 0..pw {
                let (x, y) = (x0 + px * dx, y0 + py * dy);
                raw.extend_from_slice(&rgba[(y * w + x) * 4..][..4]);
            }
        }
    }
    let mut out = jpng::SIGNATURE.to_vec();
    let mut chunk = |kind: &[u8], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend_from_slice(&c);
        out.extend_from_slice(&jpng::crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 1]);
    chunk(b"IHDR", &ihdr);
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    // Partido en dos IDAT, como hacen muchos codificadores.
    chunk(b"IDAT", &z[..z.len() / 2]);
    chunk(b"IDAT", &z[z.len() / 2..]);
    chunk(b"IEND", &[]);
    out
}

#[test]
fn entrelazado_adam7() {
    for (w, h) in [(1, 1), (3, 2), (9, 9), (37, 21)] {
        let rgba = samples(w * h * 4, (w * h) as u64);
        let file = write_interlaced_rgba(w, h, &rgba);
        let mine = jpng::decode(&file).unwrap();
        assert_eq!(mine, reference(&file), "{w}x{h}");
        assert_eq!(mine.rgba, rgba);
    }
}

#[test]
fn archivos_rotos_dan_error_sin_entrar_en_panico() {
    let data = samples(20 * 10 * 3, 7);
    let file = encode(
        20,
        10,
        ColorType::Rgb,
        BitDepth::Eight,
        Filter::Paeth,
        None,
        None,
        &data,
    );
    assert!(jpng::decode(&file).is_ok());
    // Cortado en cualquier lugar: error, nunca pánico.
    for cut in 0..file.len() {
        assert!(jpng::decode(&file[..cut]).is_err(), "cortado en {cut}");
    }
    // Un byte cambiado en cualquier lugar: error (CRC) o una imagen, pero nunca pánico.
    for i in 8..file.len() {
        let mut f = file.clone();
        f[i] ^= 0x55;
        let _ = jpng::decode(&f);
    }
    // Un IDAT alterado con el CRC arreglado: lo detecta zlib (Adler-32 o datos inválidos).
    let idat = file.windows(4).position(|w| w == b"IDAT").unwrap();
    let len = u32::from_be_bytes(file[idat - 4..idat].try_into().unwrap()) as usize;
    let mut f = file.clone();
    f[idat + 4 + len / 2] ^= 0x01;
    let crc = jpng::crc32(&f[idat..idat + 4 + len]);
    f[idat + 4 + len..idat + 8 + len].copy_from_slice(&crc.to_be_bytes());
    assert!(jpng::decode(&f).is_err());
}

#[test]
fn una_cabecera_gigante_no_reserva_memoria() {
    // IHDR que dice 100000 × 100000 en un archivo de 60 bytes.
    let mut out = jpng::SIGNATURE.to_vec();
    let mut body = b"IHDR".to_vec();
    body.extend_from_slice(&100_000u32.to_be_bytes());
    body.extend_from_slice(&100_000u32.to_be_bytes());
    body.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&jpng::crc32(&body).to_be_bytes());
    assert_eq!(jpng::decode(&out), Err(Error::TooLarge));
}

#[test]
fn jpeg_contra_la_crate_image() {
    // La crate `image` codifica (su codificador propio) y decodifica (con zune-jpeg también,
    // pero por otro camino: RGB y sin nuestros topes). Tienen que coincidir.
    let (w, h) = (45u32, 30u32);
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 5) as u8, (y * 8) as u8, ((x + y) * 3) as u8]
        })
        .collect();
    let mut file = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, 90)
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    let mine = jarvis_image::decode(&file).unwrap().unwrap();
    assert_eq!((mine.width, mine.height), (45, 30));
    let theirs = image::load_from_memory(&file).unwrap().to_rgba8();
    assert_eq!(mine.rgba, theirs.into_raw());
    assert!(!mine.has_alpha());
    // Parecido al original (JPEG pierde un poco).
    let diff: u64 = mine
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgb.as_chunks::<3>().0)
        .map(|(a, b)| {
            (0..3)
                .map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs() as u64)
                .sum::<u64>()
        })
        .sum();
    assert!(
        diff / (w * h * 3) as u64 <= 4,
        "error medio {}",
        diff / (w * h * 3) as u64
    );
    // Cortado o basura: error, sin pánico.
    for cut in [2, 10, 100, file.len() / 2] {
        let _ = jarvis_image::decode(&file[..cut]);
    }
}

#[test]
fn jpeg_progresivo_de_verdad() {
    // Hecho con Pillow (libjpeg): progresivo y con el color submuestreado a 4:2:0, como tantos
    // de la web. `cargo xtask test` abre el mismo archivo en el kernel.
    let file = include_bytes!("../../paquetes/pruebas/aurora-progresivo.jpg");
    let mine = jarvis_image::decode(file).unwrap().unwrap();
    assert_eq!((mine.width, mine.height), (640, 400));
    let theirs = image::load_from_memory(file).unwrap().to_rgba8();
    assert_eq!(mine.rgba, theirs.into_raw());
}
