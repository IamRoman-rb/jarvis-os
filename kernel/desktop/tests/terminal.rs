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
            let resp = if let Some(path) = r.url.strip_prefix("http://paquetes.jarvis/") {
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
            } else if r.url.ends_with("programa.exe") {
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
