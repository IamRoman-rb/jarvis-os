//! Webcams UVC sin webcam: los descriptores de una cámara típica (MJPEG + YUY2, tres
//! alternates isócronos), probe/commit, cuadros armados a partir de paquetes y la decodificación
//! de MJPEG sin tablas de Huffman (cruzada contra la crate `image`).

use jarvis_drivers::uvc::{
    Format, FrameAssembler, PROBE_LEN, VS_COMMIT_CONTROL, VS_PROBE_CONTROL, class_request,
    mjpeg_fix, parse_probe, parse_streaming, probe_control, set_interface, yuy2_to_rgb,
};

fn interface(number: u8, alt: u8, endpoints: u8, subclass: u8) -> Vec<u8> {
    vec![9, 4, number, alt, endpoints, 0x0E, subclass, 0, 0]
}

fn iso_endpoint(address: u8, max_packet: u16) -> Vec<u8> {
    let [lo, hi] = max_packet.to_le_bytes();
    vec![7, 5, address, 0x05, lo, hi, 1]
}

fn frame(subtype: u8, index: u8, w: u16, h: u16, intervals: &[u32]) -> Vec<u8> {
    let mut d = vec![0, 0x24, subtype, index, 0];
    d.extend_from_slice(&w.to_le_bytes());
    d.extend_from_slice(&h.to_le_bytes());
    d.extend_from_slice(&[0; 12]); // tasas de bits y buffer
    d.extend_from_slice(&intervals[0].to_le_bytes()); // por defecto
    d.push(intervals.len() as u8);
    for i in intervals {
        d.extend_from_slice(&i.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}

/// Una webcam como la mayoría: control en la interfaz 0, streaming en la 1 con MJPEG (640×480,
/// 1280×720) y YUY2 (640×480), y tres alternates con más ancho de banda cada uno.
fn webcam_config() -> Vec<u8> {
    let mut d = vec![9, 2, 0, 0, 2, 1, 0, 0x80, 250];
    d.extend([8, 0x0B, 0, 2, 0x0E, 0x03, 0, 0]); // IAD
    d.extend(interface(0, 0, 1, 1));
    d.extend([
        13, 0x24, 0x01, 0x00, 0x01, 0x4D, 0, 0x00, 0x6C, 0xDC, 0x02, 1, 1,
    ]); // VC_HEADER
    d.extend([7, 5, 0x83, 0x03, 16, 0, 6]); // interrupción del control
    d.extend(interface(1, 0, 0, 2));
    d.extend([14, 0x24, 0x01, 2, 0, 0, 0x81, 0, 0, 0, 0, 0, 1, 0]); // VS_INPUT_HEADER
    d.extend([11, 0x24, 0x06, 1, 2, 1, 1, 0, 0, 0, 0]); // VS_FORMAT_MJPEG
    d.extend(frame(0x07, 1, 640, 480, &[333_333, 666_666]));
    d.extend(frame(0x07, 2, 1280, 720, &[333_333]));
    let mut yuy2 = vec![27, 0x24, 0x04, 2, 1];
    yuy2.extend(b"YUY2");
    yuy2.extend([
        0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    ]);
    yuy2.extend([16, 1, 0, 0, 0, 0]);
    d.extend(yuy2);
    d.extend(frame(0x05, 1, 640, 480, &[333_333]));
    d.extend(interface(1, 1, 1, 2));
    d.extend(iso_endpoint(0x81, 0x00C0));
    d.extend(interface(1, 2, 1, 2));
    d.extend(iso_endpoint(0x81, 0x0400));
    d.extend(interface(1, 3, 1, 2));
    d.extend(iso_endpoint(0x81, 0x1400)); // 1024 × 3 transacciones
    let total = d.len() as u16;
    d[2..4].copy_from_slice(&total.to_le_bytes());
    d
}

#[test]
fn lee_formatos_tamanos_y_alternates() {
    let s = parse_streaming(&webcam_config()).unwrap();
    assert_eq!((s.control_interface, s.interface), (0, 1));
    assert_eq!(s.frames.len(), 3);
    let f = &s.frames[0];
    assert_eq!(
        (f.format, f.format_index, f.frame_index),
        (Format::Mjpeg, 1, 1)
    );
    assert_eq!((f.width, f.height, f.interval), (640, 480, 333_333));
    assert_eq!(f.intervals, vec![333_333, 666_666]);
    assert_eq!(s.frames[2].format, Format::Yuy2);
    assert_eq!(s.frames[2].format_index, 2);
    let bytes: Vec<u32> = s.alternates.iter().map(|a| a.bytes).collect();
    assert_eq!(bytes, vec![192, 1024, 3072]);
    assert!(s.alternates.iter().all(|a| a.endpoint == 0x81));
    // Elegir: MJPEG lo más grande que entre; si nada entra, lo más chico.
    let pick = s.choose(800, 600).unwrap();
    assert_eq!((pick.format, pick.width), (Format::Mjpeg, 640));
    assert_eq!(s.choose(1920, 1080).unwrap().width, 1280);
    assert_eq!(s.choose(320, 240).unwrap().width, 640);
    // El alternate que alcanza para lo que pide la cámara.
    assert_eq!(s.alternate_for(800).unwrap().alternate, 2);
    assert_eq!(s.alternate_for(5000).unwrap().alternate, 3);
}

#[test]
fn una_configuracion_sin_video_no_es_una_webcam() {
    // Un teclado: interfaz HID.
    let keyboard = [
        9, 2, 25, 0, 1, 1, 0, 0xA0, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10,
    ];
    assert!(parse_streaming(&keyboard).is_none());
    assert!(parse_streaming(&[]).is_none());
}

#[test]
fn probe_y_commit() {
    let s = parse_streaming(&webcam_config()).unwrap();
    let p = probe_control(&s.frames[0]);
    assert_eq!(&p[0..8], &[1, 0, 1, 1, 0x15, 0x16, 0x05, 0x00]);
    // La cámara contesta con cuánto manda.
    let mut answer = p;
    answer[18..22].copy_from_slice(&614_400u32.to_le_bytes());
    answer[22..26].copy_from_slice(&944u32.to_le_bytes());
    let got = parse_probe(&answer).unwrap();
    assert_eq!(
        (got.max_frame, got.max_payload, got.interval),
        (614_400, 944, 333_333)
    );
    assert!(parse_probe(&answer[..10]).is_none());
    // Los pedidos de clase y SET_INTERFACE.
    assert_eq!(
        class_request(false, VS_PROBE_CONTROL, 1),
        [0x21, 0x01, 0, 1, 1, 0, PROBE_LEN as u8, 0]
    );
    assert_eq!(
        class_request(true, VS_PROBE_CONTROL, 1),
        [0xA1, 0x81, 0, 1, 1, 0, PROBE_LEN as u8, 0]
    );
    assert_eq!(class_request(false, VS_COMMIT_CONTROL, 1)[3], 2);
    assert_eq!(set_interface(1, 2), [0x01, 11, 2, 0, 1, 0, 0, 0]);
}

fn packet(fid: bool, eof: bool, err: bool, payload: &[u8]) -> Vec<u8> {
    let flags = 0x80 | fid as u8 | (eof as u8) << 1 | (err as u8) << 6;
    let mut p = vec![2, flags];
    p.extend_from_slice(payload);
    p
}

#[test]
fn arma_cuadros_con_fid_y_eof() {
    let mut a = FrameAssembler::new(1000);
    assert_eq!(a.push(&packet(false, false, false, b"ab")), None);
    assert_eq!(a.push(&[]), None); // paquete vacío
    assert_eq!(a.push(&[2, 0x80]), None); // solo cabecera
    assert_eq!(
        a.push(&packet(false, true, false, b"cd")),
        Some(b"abcd".to_vec())
    );
    // Una cámara que no marca EOF: el cambio de FID cierra el cuadro.
    assert_eq!(a.push(&packet(true, false, false, b"ef")), None);
    assert_eq!(
        a.push(&packet(false, false, false, b"gh")),
        Some(b"ef".to_vec())
    );
    // Un error en el medio: ese cuadro se tira.
    assert_eq!(a.push(&packet(false, false, true, b"xx")), None);
    assert_eq!(a.push(&packet(false, true, false, b"yy")), None);
    assert_eq!(
        a.push(&packet(true, true, false, b"ok")),
        Some(b"ok".to_vec())
    );
    // Más grande que lo que dijo el probe: se tira.
    let mut small = FrameAssembler::new(3);
    assert_eq!(small.push(&packet(false, true, false, b"long")), None);
    assert_eq!(
        small.push(&packet(true, true, false, b"ok")),
        Some(b"ok".to_vec())
    );
}

#[test]
fn yuy2_a_rgb() {
    // Negro, blanco, y un rojo y un azul de BT.601.
    let px = yuy2_to_rgb(4, 1, &[16, 128, 235, 128, 81, 90, 41, 240]);
    assert_eq!(px[0], 0x000000);
    assert_eq!(px[1], 0xFFFFFF);
    let (r, g, b) = (px[2] >> 16 & 0xFF, px[2] >> 8 & 0xFF, px[2] & 0xFF);
    assert!(r > 200 && g < 40 && b < 60, "{:06x}", px[2]);
    let (r, g, b) = (px[3] >> 16 & 0xFF, px[3] >> 8 & 0xFF, px[3] & 0xFF);
    assert!(r > 150 && g < 30 && b < 100, "{:06x}", px[3]);
    // Si faltan bytes, no se inventan píxeles.
    assert_eq!(yuy2_to_rgb(4, 2, &[16, 128, 16, 128]).len(), 2);
}

/// Saca los segmentos DHT de un JPEG (como lo manda una webcam MJPEG).
fn strip_dht(jpeg: &[u8]) -> Vec<u8> {
    let mut out = jpeg[..2].to_vec();
    let mut at = 2;
    loop {
        let marker = jpeg[at + 1];
        if marker == 0xDA {
            out.extend_from_slice(&jpeg[at..]);
            return out;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        if marker != 0xC4 {
            out.extend_from_slice(&jpeg[at..at + 2 + len]);
        }
        at += 2 + len;
    }
}

#[test]
fn mjpeg_sin_tablas_se_decodifica_igual() {
    let img = image::RgbImage::from_fn(48, 32, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
    });
    let mut jpeg = std::io::Cursor::new(Vec::new());
    img.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
    let jpeg = jpeg.into_inner();
    let bare = strip_dht(&jpeg);
    assert!(bare.len() < jpeg.len());
    assert!(image::load_from_memory(&bare).is_err());
    let fixed = mjpeg_fix(&bare);
    let a = image::load_from_memory(&jpeg).unwrap().to_rgb8();
    let b = image::load_from_memory(&fixed).unwrap().to_rgb8();
    assert_eq!(a, b, "con las tablas estándar da lo mismo que el original");
    // Si ya tiene tablas, no se toca.
    assert_eq!(mjpeg_fix(&jpeg), jpeg);
    assert_eq!(mjpeg_fix(b"no es jpeg"), b"no es jpeg".to_vec());
}
