//! La consola con el cerebro (ADR 0008), con un `jarvis serve` simulado: el escritorio se
//! conecta con el token, manda los pedidos y muestra la respuesta a medida que llega.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::input::{Event, Mods};
use jarvis_desktop::system::SystemStats;
use jarvis_desktop::{AppKind, Key, Launch, StreamEvent, StreamOp};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Brain {
    t: Driver,
    stream: Option<u32>,
    /// Los renglones que mandó el escritorio.
    sent: String,
}

impl Brain {
    fn new() -> Brain {
        let mut t = Driver::new();
        t.d.set_brain(8121, TOKEN);
        Brain {
            t,
            stream: None,
            sent: String::new(),
        }
    }

    fn second(&mut self) {
        self.t.now += 1000;
        self.t.d.handle(Event::Mods(Mods::NONE), self.t.now, CLOCK);
        self.t.d.set_stats(SystemStats::default());
        self.collect();
    }

    fn collect(&mut self) {
        for op in self.t.d.take_requests().streams {
            match op {
                StreamOp::Connect(r) => {
                    assert_eq!((r.host.as_str(), r.port), ("10.0.2.2", 8121));
                    self.stream = Some(r.id);
                    self.t.d.stream_event(r.id, StreamEvent::Connected);
                    self.collect();
                    return;
                }
                StreamOp::Send(_, data) => self.sent.push_str(&String::from_utf8(data).unwrap()),
                StreamOp::Close(_) => self.stream = None,
            }
        }
    }

    fn reply(&mut self, line: &str) {
        let id = self.stream.expect("conectado");
        self.t
            .d
            .stream_event(id, StreamEvent::Data(format!("{line}\n").into_bytes()));
        self.collect();
    }

    fn console(&self) -> String {
        match self.t.d.app(AppKind::Console) {
            Some(App::Console(c)) => c.text(),
            _ => panic!("la consola no está abierta"),
        }
    }

    fn ask(&mut self, text: &str) {
        self.t.type_text(text);
        self.t.key(Key::Enter);
        self.collect();
    }
}

fn connected() -> Brain {
    let mut b = Brain::new();
    b.second();
    assert!(
        b.sent
            .starts_with(&format!("{{\"t\":\"hola\",\"token\":\"{TOKEN}\"")),
        "{}",
        b.sent
    );
    b.reply(r#"{"t":"listo"}"#);
    assert!(b.t.logs().iter().any(|l| l == "CEREBRO_CONECTADO"));
    b.second();
    b.t.d.open(Launch::App(AppKind::Console), b.t.now, CLOCK);
    b.sent.clear();
    b
}

#[test]
fn la_respuesta_llega_de_a_pedazos() {
    let mut b = connected();
    b.ask("¿qué es un \"kernel\"?");
    assert_eq!(
        b.sent,
        "{\"t\":\"pedido\",\"id\":1,\"texto\":\"¿qué es un \\\"kernel\\\"?\",\"origen\":\"consola\"}\n"
    );
    b.reply(r#"{"t":"texto","id":1,"delta":"Es el núcleo"}"#);
    assert!(b.console().contains("Es el núcleo\n"));
    b.reply(r#"{"t":"texto","id":1,"delta":" del sistema.\nManeja el hardware."}"#);
    b.reply(r#"{"t":"fin","id":1}"#);
    let text = b.console();
    assert!(
        text.contains("Es el núcleo del sistema.\nManeja el hardware.\n"),
        "{text}"
    );
    assert!(
        b.t.logs()
            .iter()
            .any(|l| l == "JARVIS_HABLA: Es el núcleo del sistema."),
        "la esfera dice la primera línea"
    );
}

#[test]
fn las_ordenes_locales_siguen_siendo_locales() {
    let mut b = connected();
    b.ask("abrir monitor");
    assert!(!b.sent.contains("pedido"), "no fue al cerebro: {}", b.sent);
    assert!(
        b.sent.contains(r#""t":"decir""#),
        "pero se dice en voz alta"
    );
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Monitor));
}

#[test]
fn esc_cancela_y_un_corte_avisa() {
    let mut b = connected();
    b.ask("contame algo largo");
    b.sent.clear();
    b.t.key(Key::Escape);
    b.collect();
    assert_eq!(b.sent, "{\"t\":\"cancelar\",\"id\":1}\n");
    b.reply(r#"{"t":"fin","id":1}"#);

    b.ask("otra cosa");
    let id = b.stream.unwrap();
    b.t.d.stream_event(id, StreamEvent::Closed(None));
    assert!(b.console().contains("Se cortó la conexión con el cerebro."));
    // Reconecta solo.
    b.stream = None;
    b.second();
    b.second();
    assert!(b.stream.is_some(), "reintenta la conexión");
}

#[test]
fn sin_cerebro_la_consola_lo_dice() {
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Console), t.now, CLOCK);
    t.type_text("qué es un kernel");
    t.key(Key::Enter);
    let text = match t.d.app(AppKind::Console) {
        Some(App::Console(c)) => c.text(),
        _ => unreachable!(),
    };
    assert!(text.contains("no está conectado"), "{text}");
    assert!(t.d.take_requests().streams.is_empty());
}

/// El último `resultado` o `confirmacion` que mandó el escritorio.
fn last_line(b: &Brain) -> String {
    b.sent.lines().last().unwrap_or("").to_string()
}

#[test]
fn una_accion_la_ejecuta_el_escritorio_y_contesta() {
    let mut b = connected();
    b.ask("abrí el navegador y buscá rust");
    b.reply(r#"{"t":"accion","llamada":1,"tool":"buscar_web","args":{"consulta":"rust"}}"#);
    assert!(
        matches!(b.t.d.focused_app(), Some(AppKind::Brave)),
        "se abrió el navegador"
    );
    assert_eq!(
        last_line(&b),
        r#"{"t":"resultado","llamada":1,"ok":true,"datos":"Busqué \"rust\" en el navegador."}"#
    );
    b.reply(r#"{"t":"accion","llamada":2,"tool":"escribir_archivo","args":{"ruta":"Notas/de jarvis.txt","contenido":"hola"}}"#);
    let data =
        b.t.d
            .fs_mut()
            .unwrap()
            .read_file("/Notas/de jarvis.txt")
            .unwrap();
    assert_eq!(data, b"hola", "crea la carpeta que falta");
    b.reply(r#"{"t":"accion","llamada":3,"tool":"leer_archivo","args":{"ruta":"/../Sistema"}}"#);
    assert!(last_line(&b).contains(r#""ok":false"#), "rutas con .. no");
}

#[test]
fn confirmar_nivel_2_con_el_teclado() {
    let mut b = connected();
    b.reply(
        r#"{"t":"confirmar","llamada":4,"tool":"mover","nivel":2,"descripcion":"Mover /a a /b."}"#,
    );
    assert_eq!(b.t.d.overlay_name(), "confirmar");
    // Arranca en Rechazar: Enter sin pensar no aprueba.
    b.t.key(Key::Enter);
    b.collect();
    assert_eq!(
        last_line(&b),
        r#"{"t":"confirmacion","llamada":4,"ok":false}"#
    );
    b.reply(
        r#"{"t":"confirmar","llamada":5,"tool":"mover","nivel":2,"descripcion":"Mover /a a /b."}"#,
    );
    b.t.key(Key::Left);
    b.t.key(Key::Enter);
    b.collect();
    assert_eq!(
        last_line(&b),
        r#"{"t":"confirmacion","llamada":5,"ok":true}"#
    );
    assert_eq!(b.t.d.overlay_name(), "");
}

#[test]
fn el_nivel_3_solo_se_aprueba_con_un_clic() {
    let mut b = connected();
    let desc = "Ejecutar en la terminal: apt install algo-muy-largo-que-no-entra-en-un-renglon-del-dialogo-de-confirmacion-de-jarvis";
    b.reply(&format!(
        r#"{{"t":"confirmar","llamada":6,"tool":"ejecutar_comando","nivel":3,"descripcion":"{desc}"}}"#
    ));
    b.t.key(Key::Left);
    b.t.key(Key::Enter);
    b.collect();
    assert_eq!(
        b.t.d.overlay_name(),
        "confirmar",
        "Enter no aprueba el nivel 3"
    );
    assert!(!b.sent.contains("\"ok\":true"));
    let [permit, _] = jarvis_desktop::shell::confirm_buttons(W, H, desc);
    b.t.click_at(permit.x + 10, permit.y + 10, 400);
    b.collect();
    assert_eq!(
        last_line(&b),
        r#"{"t":"confirmacion","llamada":6,"ok":true}"#
    );
}

#[test]
fn abrir_con_una_frase_va_al_cerebro() {
    let mut b = connected();
    b.ask("abrí el navegador y buscá rust");
    assert!(b.sent.contains("\"t\":\"pedido\""), "{}", b.sent);
}

#[test]
fn un_proyecto_abre_su_ventana_y_esc_lo_detiene() {
    let mut b = connected();
    b.reply(r#"{"t":"proyecto","nombre":"jarvis-os","ev":"inicio","texto":"jarvis-os: Seguí con lo que estábamos trabajando."}"#);
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Project));
    b.reply(r#"{"t":"proyecto","nombre":"jarvis-os","ev":"texto","texto":"Leo dónde habíamos quedado."}"#);
    b.reply(r#"{"t":"proyecto","nombre":"jarvis-os","ev":"herramienta","texto":"Read CLAUDE.md"}"#);
    let text = match b.t.d.app(AppKind::Project) {
        Some(App::Project(p)) => p.text(),
        _ => panic!("sin ventana Proyecto"),
    };
    assert!(
        text.contains("Leo dónde habíamos quedado.\n  · Read CLAUDE.md"),
        "{text}"
    );
    b.sent.clear();
    b.t.key(Key::Escape);
    b.collect();
    assert_eq!(b.sent, "{\"t\":\"proyecto_detener\"}\n");
    b.reply(r#"{"t":"proyecto","nombre":"jarvis-os","ev":"fin","texto":"Detenido."}"#);
    assert!(b.t.logs().iter().any(|l| l == "PROYECTO_FIN jarvis-os"));
}

#[test]
fn lo_que_se_oye_se_trata_como_escrito() {
    let mut b = connected();
    // Una orden local, por voz.
    b.reply(r#"{"t":"oido","texto":"abrir monitor"}"#);
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Monitor));
    assert!(
        !b.sent.contains("pedido"),
        "las órdenes locales no van al cerebro"
    );
    assert!(
        b.sent.starts_with(r#"{"t":"decir","texto":"Abriendo"#),
        "pero la respuesta se dice en voz alta: {}",
        b.sent
    );
    b.sent.clear();
    // Un pedido para Claude, por voz: va con origen voz (se contesta en voz alta).
    b.reply(r#"{"t":"oido","texto":"contame un chiste"}"#);
    assert!(
        b.sent
            .contains(r#""texto":"contame un chiste","origen":"voz""#),
        "{}",
        b.sent
    );
    assert!(b.console().contains("(voz) contame un chiste"));
    b.reply(r#"{"t":"voz","nivel":60}"#);
    // Win+J: escuchar sin la palabra de activación.
    b.sent.clear();
    b.t.combo(Mods::WIN, Key::Char('j'));
    b.collect();
    assert_eq!(b.sent, "{\"t\":\"escuchar\"}\n");
}

#[test]
fn abrir_un_archivo_y_leer_la_terminal() {
    let mut b = connected();
    b.reply(r#"{"t":"accion","llamada":1,"tool":"escribir_archivo","args":{"ruta":"/Descargas/file.txt","contenido":"Hola Mundo"}}"#);
    b.reply(
        r#"{"t":"accion","llamada":2,"tool":"abrir_archivo","args":{"ruta":"Descargas/file.txt"}}"#,
    );
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Editor));
    assert!(
        last_line(&b).contains("en Editor de texto"),
        "{}",
        last_line(&b)
    );
    b.reply(r#"{"t":"accion","llamada":3,"tool":"abrir_archivo","args":{"ruta":"/Descargas"}}"#);
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Files));
    b.reply(
        r#"{"t":"accion","llamada":4,"tool":"abrir_archivo","args":{"ruta":"/no/existe.txt"}}"#,
    );
    assert!(last_line(&b).contains(r#""ok":false"#));
    // La salida del comando vuelve a Claude.
    b.reply(r#"{"t":"accion","llamada":5,"tool":"ejecutar_comando","args":{"comando":"echo hola desde jsh"}}"#);
    assert!(
        last_line(&b).contains("hola desde jsh\n") || last_line(&b).ends_with("hola desde jsh\"}"),
        "{}",
        last_line(&b)
    );
    b.reply(r#"{"t":"accion","llamada":6,"tool":"leer_terminal","args":{}}"#);
    assert!(
        last_line(&b).contains("hola desde jsh"),
        "{}",
        last_line(&b)
    );
}

#[test]
fn iniciar_sesion_con_google_desde_configuracion() {
    let mut b = connected();
    b.reply(r#"{"t":"cuenta","sesion":false,"email":"","plan":"","estado":""}"#);
    assert!(b.t.logs().iter().any(|l| l == "CEREBRO_CUENTA no "));
    // Configuración → Asistente (IA): la última sección.
    b.t.combo(Mods::WIN, Key::Char('i'));
    b.t.key(Key::PageUp);
    let Some(App::Settings(st)) = b.t.d.app(AppKind::Settings) else {
        panic!("no se abrió Configuración");
    };
    assert_eq!(st.section.name(), "Asistente (IA)");
    // Filas: Cerebro, Cuenta (solo lectura), Iniciar sesión con Google.
    b.t.key(Key::Down);
    b.t.key(Key::Down);
    b.t.key(Key::Enter);
    b.collect();
    assert!(
        b.sent
            .contains(r#"{"t":"iniciar_sesion","metodo":"google"}"#),
        "{}",
        b.sent
    );
    b.reply(r#"{"t":"cuenta","sesion":true,"email":"roman@example.com","plan":"pro","estado":"Listo: entraste."}"#);
    assert!(
        b.t.logs()
            .iter()
            .any(|l| l == "CEREBRO_CUENTA si Listo: entraste.")
    );
}

#[test]
fn vincular_gemini_chatgpt_y_deepseek_desde_configuracion() {
    let mut b = Brain::new();
    b.second();
    // El principal y el consejo viajan en `hola` (por defecto, Claude y sin consejo).
    assert!(
        b.sent.contains(r#""principal":"claude","consejo":false}"#),
        "{}",
        b.sent
    );
    b.reply(r#"{"t":"listo"}"#);
    b.reply(r#"{"t":"agente","id":"gemini","nombre":"Gemini","vinculado":false,"metodo":"","detalle":"","estado":""}"#);
    b.second();
    b.sent.clear();
    b.t.combo(Mods::WIN, Key::Char('i'));
    b.t.key(Key::PageUp);
    // Cerebro, Cuenta, Google (Claude), Actualizar, Agente principal, Consejo, Gemini (estado),
    // Gemini con Google, Gemini clave de API...
    b.t.keys(&[Key::Down, Key::Down, Key::Down]);
    // Agente principal: → pasa de Claude a Gemini y se le avisa al cerebro.
    b.t.key(Key::Down);
    b.t.key(Key::Right);
    b.collect();
    assert!(
        b.sent
            .contains(r#"{"t":"agentes_modo","principal":"gemini","consejo":false}"#),
        "{}",
        b.sent
    );
    assert_eq!(b.t.d.config().ai_lead, 1);
    // El consejo.
    b.t.key(Key::Down);
    b.t.key(Key::Enter);
    b.collect();
    assert!(b.sent.contains(r#""consejo":true}"#), "{}", b.sent);
    // Gemini con Google (la fila de su estado es solo lectura).
    b.sent.clear();
    b.t.keys(&[Key::Down, Key::Down]);
    b.t.key(Key::Enter);
    b.collect();
    assert_eq!(
        b.sent,
        "{\"t\":\"vincular\",\"agente\":\"gemini\",\"metodo\":\"google\"}\n"
    );
    // ChatGPT con el formulario: la clave va al anfitrión y no a los logs.
    b.sent.clear();
    b.t.keys(&[Key::Down, Key::Down, Key::Down, Key::Down]);
    b.t.key(Key::Enter);
    b.t.type_text("sk-prueba-1234");
    b.t.key(Key::Enter);
    b.collect();
    assert_eq!(
        b.sent,
        "{\"t\":\"vincular\",\"agente\":\"chatgpt\",\"metodo\":\"clave\",\"clave\":\"sk-prueba-1234\"}\n"
    );
    assert!(!b.t.d.config().serialize().contains("sk-prueba"));
    b.reply(r#"{"t":"agente","id":"chatgpt","nombre":"ChatGPT","vinculado":true,"metodo":"clave","detalle":"clave ...1234","estado":"Listo: clave guardada en el anfitrión."}"#);
    let logs = b.t.logs();
    assert!(logs.iter().any(|l| l == "CEREBRO_VINCULAR chatgpt clave"));
    assert!(!logs.iter().any(|l| l.contains("sk-prueba")));
    assert!(
        logs.iter()
            .any(|l| l == "CEREBRO_AGENTE chatgpt si Listo: clave guardada en el anfitrión.")
    );
}

#[test]
fn al_arrancar_saluda_segun_la_hora_y_el_cerebro_lo_completa() {
    // CLOCK es a las 23:05: de noche.
    let mut b = Brain::new();
    b.second();
    let logs = b.t.logs();
    assert!(logs.iter().any(|l| l == "SALUDO noche"), "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l == "JARVIS_HABLA: Buenas noches, Roman.")
    );
    // En cuanto el cerebro está listo, le pide el saludo (con el id 0).
    b.reply(r#"{"t":"listo"}"#);
    assert!(
        b.sent
            .contains(r#"{"t":"saludo","id":0,"momento":"noche","nombre":"Roman"}"#),
        "{}",
        b.sent
    );
    b.reply(r#"{"t":"texto","id":0,"delta":"Buenas noches, Roman."}"#);
    b.reply(r#"{"t":"fin","id":0}"#);
    let logs = b.t.logs();
    assert!(logs.iter().any(|l| l == "CEREBRO_SALUDO noche"), "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l == "JARVIS_HABLA: Buenas noches, Roman."),
        "la respuesta del cerebro se muestra (y él la dice en voz alta)"
    );
    // Una sola vez: si se reconecta, no vuelve a saludar.
    let id = b.stream.unwrap();
    b.t.d.stream_event(id, StreamEvent::Closed(None));
    b.sent.clear();
    b.t.now += 60_000;
    b.second();
    b.reply(r#"{"t":"listo"}"#);
    assert!(!b.sent.contains("saludo"), "{}", b.sent);
    // Y los pedidos de Roman siguen numerándose desde 1.
    b.t.d.open(Launch::App(AppKind::Console), b.t.now, CLOCK);
    b.sent.clear();
    b.ask("hola");
    assert!(b.sent.contains(r#""id":1,"#), "{}", b.sent);
}

#[test]
fn a_la_manana_dice_buenos_dias() {
    let morning = Some(jarvis_gfx::clock::DateTime {
        hour: 8,
        ..CLOCK.unwrap()
    });
    // El primer momento en que se sabe la hora es el que vale.
    let mut d = common::desktop();
    d.handle(Event::Mods(Mods::NONE), 1000, morning);
    let logs = d.take_logs();
    assert!(logs.iter().any(|l| l == "SALUDO manana"), "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l == "JARVIS_HABLA: Buenos días, Roman.")
    );
}

#[test]
fn jarvis_aplica_sus_cambios_y_el_sistema_se_apaga_para_arrancar_la_version_nueva() {
    let mut b = connected();
    b.reply(r#"{"t":"reiniciar","motivo":"actualizar"}"#);
    assert!(b.t.logs().iter().any(|l| l == "CEREBRO_REINICIAR"));
    // Unos segundos, para que se lea la respuesta de JARVIS; después se apaga.
    b.second();
    let off = |b: &mut Brain| {
        b.t.logs()
            .iter()
            .any(|l| l == "ESCRITORIO_ENERGIA Shutdown")
    };
    assert!(!off(&mut b));
    b.t.now += 4000;
    b.second();
    assert!(
        off(&mut b),
        "apaga (no reinicia): xtask arranca la imagen nueva"
    );
}
