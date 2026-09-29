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
            let resp = if let Some(program) = r.url.strip_prefix("http://paquetes.jarvis/usuario/") {
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
            } else if let Some(body) = fake_internet(&r.url) {
                Ok(HttpResponse {
                    status: 200,
                    content_type: "text/plain".into(),
                    url: r.url.clone(),
                    body,
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
    let body: &str = if url.starts_with("https://api.snapcraft.io/v2/snaps/find") {
        r#"{"results":[{"name":"vlc","revision":{"version":"3.0.20"},"snap":{"publisher":{"username":"videolan","validation":"verified"},"summary":"The ultimate media player","title":"VLC"}}]}"#
    } else if url.starts_with("https://api.snapcraft.io/v2/snaps/info/vlc") {
        r#"{"name":"vlc","snap":{"summary":"The ultimate media player","publisher":{"display-name":"VideoLAN"}},"channel-map":[]}"#
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

#[test]
fn apt_instala_programas_del_repositorio_y_los_desinstala() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    let out = run(&mut t, "apt install esenciales");
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
    assert!(run(&mut t, "apt install neofetch").contains("ya está en su versión más reciente"));
    assert!(run(&mut t, "apt install no-existe").contains("No se ha podido localizar el paquete"));
    // Desinstalar: el programa va a la Papelera y ya no se encuentra.
    assert!(run(&mut t, "apt remove calc").contains("Papelera"));
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

#[test]
fn wget_descarga_un_exe_que_se_puede_inspeccionar_pero_no_ejecutar() {
    let mut t = Driver::new();
    open_terminal(&mut t);
    run(&mut t, "cd /Documentos");
    let out = run(&mut t, "wget http://ejemplo.com/programa.exe");
    assert!(out.contains("guardado"), "{out}");
    assert!(out.contains("PE32+ de Windows"), "{out}");
    assert!(run(&mut t, "file programa.exe").contains("programa de consola"));
    let out = run(&mut t, "./programa.exe");
    assert!(out.contains("todavía no puede ejecutarlo"), "{out}");
    assert!(fatfs_exists(t.d, "/Documentos/programa.exe"));
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
        "sudo ufw deny out to example.com && ufw deny out port 8080 app apt && ufw status numbered",
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
    let out = run(&mut t, "grep firewall_regla /Sistema/config.ini");
    assert!(out.contains("denegar salida a example.com"), "{out}");
    // apt usa otro sitio: sigue andando.
    let out = run(&mut t, "apt install hola");
    assert!(out.contains("hola"), "{out}");
    // Borrar la regla vuelve a dejar pasar.
    run(&mut t, "ufw delete 1");
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
    let out = run(&mut t, "snap refresh saludo --beta");
    assert!(out.contains("actualizado (revisión 5)"), "{out}");
    assert!(run(&mut t, "saludo").contains("(beta)"));
    let out = run(&mut t, "snap revert saludo");
    assert!(out.contains("revisión 3"), "{out}");
    assert!(!run(&mut t, "saludo").contains("(beta)"));
    // Uno de Linux (de Snapcraft) no se puede instalar: se explica por qué.
    let out = run(&mut t, "snap install vlc");
    assert!(
        out.contains("snap de Linux") && out.contains("snap download vlc"),
        "{out}"
    );
    let out = run(&mut t, "snap remove saludo && ls /snap/bin");
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
    let out = run(&mut t, "winget install 7ZIP.7zip");
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
    let out = run(&mut t, "apt install programas-linux");
    assert!(out.contains("Listo"), "{out}");
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
        data: b"Hola desde Linux
".to_vec(),
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
        [(pid, ProcReply::Line(b"Roman
".to_vec()))]
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

    // Uno dinámico no se lanza: se explica por qué.
    t.d.fs_mut()
        .unwrap()
        .write_file(
            "/Documentos/dinamico",
            &tiny_elf(Some("/lib64/ld-linux-x86-64.so.2")),
            jarvis_fs::Timestamp::EPOCH,
        )
        .unwrap();
    let out = run(&mut t, "/Documentos/dinamico");
    assert!(out.contains("bibliotecas dinámicas"), "{out}");
    assert!(t.d.take_requests().spawn.is_empty());
    assert!(fatfs_exists(t.d, "/Papelera/k11.txt"));
}
