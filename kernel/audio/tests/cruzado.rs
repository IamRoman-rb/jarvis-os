//! Tests cruzados: archivos armados por otra implementación (el generador en Python de
//! `kernel/paquetes/generar_multimedia.py`, que usa Pillow para los JPEG).

use jarvis_audio::avi::{Avi, AviAudio};
use jarvis_audio::resample::Decoder;
use jarvis_audio::wav::{Codec, Wav};

#[test]
fn el_adpcm_del_generador_de_python() {
    let file = include_bytes!("../../paquetes/musica/tema.wav").to_vec();
    let mut w = Wav::parse(file).unwrap();
    assert_eq!(w.format.codec, Codec::ImaAdpcm { block_align: 512 });
    assert_eq!((w.format.rate, w.format.channels), (22_050, 1));
    let mut out = Vec::new();
    w.next_chunk(&mut out);
    w.next_chunk(&mut out);
    // Los valores que reconstruye el codificador de Python (el mismo algoritmo, escrito aparte).
    assert_eq!(out.len(), 2034);
    assert_eq!(&out[..6], &[0, 10, 33, 85, 152, 234]);
    assert_eq!(&out[1000..1004], &[-7113, -5367, -3725, -1805]);
    assert_eq!(out.iter().map(|&v| v as i64).sum::<i64>(), 33103);
    // Dura 20 s.
    assert_eq!(w.frames() / 22_050, 20);
}

#[test]
fn el_video_del_generador_de_python() {
    let file = include_bytes!("../../paquetes/videos/demo.avi").to_vec();
    let avi = Avi::parse(&file).unwrap();
    assert_eq!((avi.width, avi.height), (320, 240));
    assert_eq!(avi.frames.len(), 120);
    assert_eq!(avi.duration_ms(), 7999);
    // Cada cuadro es un JPEG (lo hizo Pillow).
    let (at, len) = avi.frames[30];
    assert!(file[at..at + len].starts_with(&[0xFF, 0xD8]));
    let mut a = AviAudio::new(std::sync::Arc::new(file), &avi).unwrap();
    assert_eq!((a.rate(), a.channels()), (22_050, 1));
    let mut out = Vec::new();
    a.next_chunk(&mut out);
    // El bip de 880 Hz arranca con el video.
    assert_eq!(out.len(), 1470);
    assert!(out.iter().any(|&v| v > 8000));
}
