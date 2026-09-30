//! El instalador (K13, Configuración → Hardware): solo ofrece discos vacíos, pide confirmar dos
//! veces y recién entonces le pasa el pedido al kernel.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::{AppKind, DiskInfo, InstallState, Key, Launch, SystemStats};

fn disks() -> Vec<DiskInfo> {
    vec![
        DiskInfo {
            name: "USB: Kingston DataTraveler".into(),
            mib: 30_000,
            blank: false,
            boot_medium: true,
        },
        DiskInfo {
            name: "SATA 0: WD Green 2.5 480GB".into(),
            mib: 457_862,
            blank: false,
            boot_medium: false,
        },
        DiskInfo {
            name: "NVMe: disco nuevo".into(),
            mib: 244_198,
            blank: true,
            boot_medium: false,
        },
    ]
}

fn open(t: &mut Driver, stats: SystemStats) {
    t.d.set_stats(stats);
    t.d.open(
        Launch::Settings(jarvis_desktop::apps::settings::HARDWARE),
        t.now,
        CLOCK,
    );
    let Some(App::Settings(s)) = t.d.app(AppKind::Settings) else {
        panic!("Configuración no se abrió");
    };
    assert_eq!(s.section.name(), "Hardware");
}

#[test]
fn instala_solo_en_el_disco_vacio_y_con_dos_confirmaciones() {
    let mut t = Driver::new();
    open(
        &mut t,
        SystemStats {
            devices: vec![("Red".into(), "Realtek RTL8168".into())],
            disks: disks(),
            ..SystemStats::default()
        },
    );
    // La última fila es el disco vacío. Un Enter pide confirmar; todavía no pasa nada.
    t.key(Key::End);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().install, None);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().install, Some(2));
    assert!(t.logs().iter().any(|l| l == "INSTALAR_PEDIDO 2"));

    // El disco con Windows (particiones) no tiene botón: Enter no hace nada.
    t.key(Key::Up);
    t.key(Key::Enter);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().install, None);
}

#[test]
fn sin_medio_de_arranque_no_se_ofrece_nada() {
    let mut t = Driver::new();
    let mut d = disks();
    d[0].boot_medium = false;
    open(
        &mut t,
        SystemStats {
            disks: d,
            ..SystemStats::default()
        },
    );
    t.key(Key::End);
    t.key(Key::Enter);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().install, None);
}

#[test]
fn mientras_instala_no_se_puede_pedir_otra_vez() {
    let mut t = Driver::new();
    open(
        &mut t,
        SystemStats {
            disks: disks(),
            install: InstallState::Working("NVMe: disco nuevo".into()),
            ..SystemStats::default()
        },
    );
    t.key(Key::End);
    t.key(Key::Enter);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().install, None);
}
