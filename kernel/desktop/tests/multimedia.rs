//! K12: la app Música y el Visor de videos con los parlantes. El kernel está simulado: el test
//! pide el audio mezclado como lo haría la tarea "audio" y le dice al escritorio cuánto sonó.

mod common;

use common::*;
use jarvis_desktop::Launch;

/// Simula `ms` de reproducción: la placa consume de a 20 ms (a 48 kHz) y el escritorio dibuja
/// un cuadro cada 20 ms.
fn play(t: &mut Driver, played: &mut u64, ms: u64) -> Vec<String> {
    let mut logs = Vec::new();
    for _ in 0..ms / 20 {
        let pcm = t.d.audio_render(960);
        assert_eq!(pcm.len(), 1920);
        *played += 960;
        t.d.set_audio_played(*played);
        t.now += 20;
        t.frame();
        logs.extend(t.logs());
    }
    logs
}

fn write(t: &mut Driver, path: &str, data: &[u8]) {
    let fs = t.d.fs_mut().unwrap();
    let _ = fs.mkdir(
        path.rsplit_once('/').unwrap().0,
        jarvis_fs::Timestamp::EPOCH,
    );
    fs.write_file(path, data, jarvis_fs::Timestamp::EPOCH)
        .unwrap();
}

#[test]
fn un_video_sigue_a_su_audio() {
    let mut t = Driver::new();
    t.d.enable_sound(48_000);
    write(
        &mut t,
        "/Videos/demo.avi",
        include_bytes!("../../paquetes/videos/demo.avi"),
    );
    t.d.open(Launch::View("/Videos/demo.avi".into()), t.now, CLOCK);
    let mut played = 0;
    let logs = play(&mut t, &mut played, 200);
    assert!(
        logs.iter().any(
            |l| l.starts_with("VIDEO_ABIERTO /Videos/demo.avi 320x240, 120 cuadros")
                && l.ends_with("con audio")
        ),
        "{logs:?}"
    );
    assert!(logs.iter().any(|l| l == "VIDEO_PRIMER_CUADRO"));
    // Pausa: el tiempo del video no avanza aunque pase el reloj.
    t.key(jarvis_desktop::Key::Char(' '));
    let logs = play(&mut t, &mut played, 1000);
    assert!(!logs.iter().any(|l| l.starts_with("VIDEO_FIN")));
    t.key(jarvis_desktop::Key::Char(' '));
    // Termina cuando termina su audio (8 s), no antes.
    let logs = play(&mut t, &mut played, 7000);
    assert!(!logs.iter().any(|l| l.starts_with("VIDEO_FIN")), "{logs:?}");
    let logs = play(&mut t, &mut played, 1500);
    assert!(
        logs.iter().any(|l| l == "VIDEO_FIN /Videos/demo.avi"),
        "{logs:?}"
    );
}

#[test]
fn musica_por_los_parlantes() {
    let mut t = Driver::new();
    t.d.enable_sound(48_000);
    write(
        &mut t,
        "/Música/Escala.wav",
        include_bytes!("../../paquetes/musica/escala.wav"),
    );
    // Abrir un WAV (desde Archivos o `open`) lo reproduce en la app Música.
    t.d.open(Launch::Play("/Música/Escala.wav".into()), t.now, CLOCK);
    let mut played = 0;
    let logs = play(&mut t, &mut played, 1000);
    assert!(
        logs.iter()
            .any(|l| l == "MUSICA_ARCHIVO /Música/Escala.wav (3200 ms)"),
        "{logs:?}"
    );
    // Lo que se mezcló no es silencio.
    let pcm = t.d.audio_render(960);
    assert!(pcm.iter().any(|&s| s.unsigned_abs() > 1000));
    let logs = play(&mut t, &mut played, 3000);
    assert!(logs.iter().any(|l| l == "MUSICA_FIN"), "{logs:?}");
    // Sin placa de sonido, las partituras siguen sonando por el parlante de la PC.
    let mut t = Driver::new();
    t.d.open(Launch::App(jarvis_desktop::AppKind::Music), t.now, CLOCK);
    t.key(jarvis_desktop::Key::Enter);
    assert!(t.logs().iter().any(|l| l.ends_with("(parlante de la PC)")));
    // La nota se pide en el próximo cuadro.
    t.now += 10;
    t.frame();
    assert!(t.d.take_requests().tone.is_some_and(|hz| hz > 0));
}
