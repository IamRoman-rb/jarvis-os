//! El asistente de instalación del modo en vivo (setup.rs) y el primer inicio de sesión del
//! sistema instalado: contraseña y presentación de JARVIS.

mod common;

use std::io::{Cursor, Write};

use common::*;
use fatfs::FsOptions;
use jarvis_desktop::{Config, Desktop, DiskInfo, InstallState, Key, Power, SystemStats};
use jarvis_fs::{FileSystem, MemDisk};

fn disks() -> Vec<DiskInfo> {
    vec![
        DiskInfo {
            name: "CD 2: VBOX CD-ROM".into(),
            mib: 36,
            blank: false,
            boot_medium: true,
        },
        DiskInfo {
            name: "IDE 0: VBOX HARDDISK".into(),
            mib: 11_004,
            blank: true,
            boot_medium: false,
        },
    ]
}

fn live(disks: Vec<DiskInfo>) -> Driver {
    let mut t = Driver::new();
    t.d.set_stats(SystemStats {
        disks,
        ..SystemStats::default()
    });
    t.d.start_setup();
    assert_eq!(t.d.overlay_name(), "instalador");
    t.frame();
    t
}

fn erase(t: &mut Driver, n: usize) {
    for _ in 0..n {
        t.key(Key::Backspace);
    }
}

#[test]
fn pide_usuario_contrasena_y_disco_y_despues_reinicia() {
    let mut t = live(disks());
    t.key(Key::Enter);

    // Un nombre con caracteres raros no pasa.
    t.type_text("roman!");
    t.key(Key::Enter);
    t.frame();
    assert!(t.d.take_requests().install.is_none());
    erase(&mut t, 1);
    t.key(Key::Enter);

    // Contraseña: muy corta, después distinta, después bien.
    t.type_text("abc");
    t.key(Key::Enter);
    t.type_text("d");
    t.key(Key::Enter);
    t.type_text("abce");
    t.key(Key::Enter);
    t.frame();
    t.type_text("abcd");
    t.key(Key::Enter);

    // El disco vacío (el CD es el medio de arranque: no se ofrece) y la confirmación.
    t.frame();
    t.key(Key::Enter);
    t.frame();
    assert!(t.d.take_requests().install.is_none(), "falta confirmar");
    t.key(Key::Enter);
    let req = t.d.take_requests();
    assert_eq!(req.install, Some(1));
    let cfg = req
        .install_config
        .expect("la configuración para el disco nuevo");
    assert!(cfg.contains("usuario=roman"), "{cfg}");
    assert!(cfg.contains("presentacion=si"), "{cfg}");
    assert!(cfg.contains("pin_hash=pbkdf2-sha256$"), "{cfg}");
    assert!(!cfg.contains("abcd"), "la contraseña nunca va en texto");
    let parsed = Config::parse(&cfg);
    assert!(parsed.pin.expect("con contraseña").verify("abcd"));
    assert!(t.logs().iter().any(|l| l == "INSTALAR_PEDIDO 1"));

    // El kernel termina; Enter reinicia.
    t.frame();
    t.d.set_stats(SystemStats {
        disks: disks(),
        install: InstallState::Done(Ok("Saqué la ISO de la lectora.".into())),
        ..SystemStats::default()
    });
    assert!(t.logs().iter().any(|l| l == "INSTALADOR_LISTO"));
    t.frame();
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().power, Some(Power::Reboot));
}

#[test]
fn sin_disco_vacio_no_instala_y_se_puede_probar_en_vivo() {
    let mut only_cd = disks();
    only_cd.truncate(1);
    let mut t = live(only_cd);
    t.key(Key::Enter);
    t.type_text("ana");
    t.key(Key::Enter);
    t.type_text("1234");
    t.key(Key::Enter);
    t.type_text("1234");
    t.key(Key::Enter);
    t.frame();
    t.keys(&[Key::Enter, Key::Enter, Key::Enter]);
    assert!(t.d.take_requests().install.is_none());

    // Atrás hasta el principio y "probar sin instalar".
    t.keys(&[Key::Escape, Key::Escape, Key::Escape, Key::Escape]);
    assert_eq!(t.d.overlay_name(), "");
    assert!(
        t.logs()
            .iter()
            .any(|l| l.starts_with("JARVIS_HABLA: Modo en vivo"))
    );
}

#[test]
fn un_error_al_instalar_vuelve_a_elegir_disco() {
    let mut t = live(disks());
    t.key(Key::Enter);
    t.type_text("ana");
    t.key(Key::Enter);
    for _ in 0..2 {
        t.type_text("clave");
        t.key(Key::Enter);
    }
    t.keys(&[Key::Enter, Key::Enter]);
    assert_eq!(t.d.take_requests().install, Some(1));
    t.d.set_stats(SystemStats {
        disks: disks(),
        install: InstallState::Done(Err("no se pudo escribir".into())),
        ..SystemStats::default()
    });
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "INSTALADOR_ERROR no se pudo escribir")
    );
    t.frame();
    t.key(Key::Enter); // vuelve al disco
    t.key(Key::Enter); // confirmar
    t.key(Key::Enter); // instalar de nuevo
    assert_eq!(t.d.take_requests().install, Some(1));
}

/// Un disco como el que deja el instalador: con la configuración del asistente.
fn installed(user: &str, password: &str) -> Driver {
    let cfg = Config {
        user: user.into(),
        pin: jarvis_desktop::pin::hash_pin(password, user.as_bytes()),
        tour: true,
        ..Config::default()
    };
    let mut img = Cursor::new(seeded_image());
    {
        let fs = fatfs::FileSystem::new(&mut img, FsOptions::new()).unwrap();
        let dir = fs.root_dir().create_dir("Sistema").unwrap();
        dir.create_file("config.ini")
            .unwrap()
            .write_all(cfg.serialize().as_bytes())
            .unwrap();
    }
    let fs = FileSystem::mount(MemDisk::new(img.into_inner())).unwrap();
    Driver {
        d: Desktop::new(W, H, 300, Some(fs)),
        now: 1000,
    }
}

#[test]
fn el_primer_inicio_pide_la_contrasena_y_jarvis_presenta_el_sistema() {
    let mut t = installed("ana", "luna-azul");
    assert!(t.d.is_locked() && t.d.session_closed());
    t.frame();
    assert!(
        !t.logs().iter().any(|l| l.starts_with("SALUDO")),
        "la primera vez saluda la presentación"
    );

    t.type_text("luna-roja");
    t.key(Key::Enter);
    assert!(t.d.is_locked());
    assert!(t.logs().iter().any(|l| l == "ESCRITORIO_PIN_INCORRECTO"));

    t.type_text("luna-azul");
    t.key(Key::Enter);
    assert!(!t.d.is_locked());
    let mut logs = t.logs();
    assert!(logs.iter().any(|l| l == "SESION_INICIADA ana"));
    assert!(logs.iter().any(|l| l == "PRESENTACION_INICIO"));

    // El kernel llama a set_stats una vez por segundo; JARVIS habla frase por frase.
    for _ in 0..60 {
        t.now += 1000;
        t.frame();
        t.d.set_stats(SystemStats::default());
        logs.extend(t.logs());
    }
    let said: Vec<&String> = logs
        .iter()
        .filter(|l| l.starts_with("JARVIS_HABLA"))
        .collect();
    assert_eq!(
        said.first().map(|s| s.as_str()),
        Some("JARVIS_HABLA: Bienvenido a JARVIS-OS, Ana. Te muestro dónde está cada cosa.")
    );
    assert_eq!(said.len(), jarvis_desktop::setup::TOUR.len());
    assert!(logs.iter().any(|l| l == "PRESENTACION_FIN"));

    // Quedó guardado: la próxima vez no se repite.
    let text = String::from_utf8(fatfs_read(t.d, "/Sistema/config.ini").unwrap()).unwrap();
    assert!(text.contains("presentacion=no"), "{text}");
}

#[test]
fn la_tecla_windows_no_saltea_la_contrasena() {
    use jarvis_desktop::Mods;
    let win = Mods {
        win: true,
        ..Mods::NONE
    };
    let alt = Mods {
        alt: true,
        ..Mods::NONE
    };
    let mut t = installed("ana", "luna-azul");
    t.frame();
    // La tecla Windows sola (apretar y soltar), Alt+Tab, Win+D, Win+E, Esc.
    t.mods(win);
    t.mods(Mods::NONE);
    t.mods(alt);
    t.key(Key::Tab);
    t.mods(Mods::NONE);
    t.combo(win, Key::Char('d'));
    t.combo(win, Key::Char('e'));
    t.key(Key::Escape);
    t.frame();
    assert!(t.d.is_locked(), "sigue en la pantalla de inicio de sesión");
    assert_eq!(t.d.overlay_name(), "sesion");
    assert!(t.d.focused_app().is_none(), "no se abrió nada");
    t.type_text("luna-azul");
    t.key(Key::Enter);
    assert!(!t.d.is_locked());
}

#[test]
fn la_tecla_windows_no_saltea_el_asistente() {
    use jarvis_desktop::Mods;
    let win = Mods {
        win: true,
        ..Mods::NONE
    };
    let mut t = live(disks());
    t.mods(win);
    t.mods(Mods::NONE);
    t.combo(win, Key::Char('d'));
    t.frame();
    assert_eq!(t.d.overlay_name(), "instalador");
}
