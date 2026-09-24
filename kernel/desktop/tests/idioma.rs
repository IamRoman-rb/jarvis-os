//! El idioma de la interfaz. Va en su propio archivo (su propio proceso de test): el idioma es
//! global y cambiarlo al mismo tiempo que corren otros tests los confundiría.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::{AppKind, Key, Launch};

fn settings_section(t: &Driver) -> String {
    match t.d.app(AppKind::Settings) {
        Some(App::Settings(s)) => s.section.name().to_string(),
        _ => panic!("Configuración no está abierta"),
    }
}

#[test]
fn el_idioma_cambia_la_interfaz() {
    let mut t = Driver::new();
    t.d.open(Launch::Settings(2), t.now, CLOCK);
    assert_eq!(settings_section(&t), "Hora e idioma");
    // Primera fila: el idioma. → pasa a inglés.
    t.key(Key::Right);
    t.frame();
    assert_eq!(t.d.config().language, jarvis_desktop::i18n::Lang::En);
    assert_eq!(settings_section(&t), "Time & language");
    assert_eq!(jarvis_desktop::i18n::tr("Papelera"), "Trash");
    // Y a portugués.
    t.key(Key::Right);
    t.frame();
    assert_eq!(settings_section(&t), "Hora e idioma");
    assert_eq!(jarvis_desktop::i18n::tr("Configuración"), "Configurações");
    let cfg = fatfs_read(t.d, "/Sistema/config.ini").expect("se guardó");
    assert!(String::from_utf8(cfg).unwrap().contains("idioma=pt"));
    jarvis_desktop::i18n::set(jarvis_desktop::i18n::Lang::Es);
}
