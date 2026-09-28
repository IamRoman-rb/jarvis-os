//! Idiomas de la interfaz: castellano (el de siempre), inglés y portugués.
//!
//! Los textos del código están en castellano; [`tr`] devuelve la traducción del idioma elegido
//! en Configuración (o el mismo texto si no hay traducción, o si el idioma es castellano). Los
//! textos con datos adentro usan [`trf`], con `{}` donde van los datos.
//!
//! La terminal y los mensajes para los tests (los que salen por el puerto serie) quedan en
//! castellano: son la "API" del sistema. El idioma elegido se guarda en `/Sistema/config.ini`
//! (`idioma=en`) y también cambia la fecha del HUD.
//!
//! Solo caracteres de Latin-1: la fuente de la interfaz no tiene otros.

use alloc::string::String;
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Es,
    En,
    Pt,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Es, Lang::En, Lang::Pt];

    /// El nombre en su propio idioma.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Es => "Español",
            Lang::En => "English",
            Lang::Pt => "Português",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
            Lang::Pt => "pt",
        }
    }

    pub fn from_code(c: &str) -> Lang {
        match c.trim() {
            "en" => Lang::En,
            "pt" => Lang::Pt,
            _ => Lang::Es,
        }
    }

    /// Lo que va en `$LANG` en la terminal.
    pub fn locale(self) -> &'static str {
        match self {
            Lang::Es => "es_AR.UTF-8",
            Lang::En => "en_US.UTF-8",
            Lang::Pt => "pt_BR.UTF-8",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
    jarvis_gfx::clock::set_language(lang as u8);
}

pub fn get() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Pt,
        _ => Lang::Es,
    }
}

fn find(table: &'static [(&'static str, &'static str)], s: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(k, _)| k.cmp(&s))
        .ok()
        .map(|i| table[i].1)
}

/// La traducción de `s` al idioma actual (o `s` si no hay).
pub fn tr(s: &'static str) -> &'static str {
    let table = match get() {
        Lang::Es => return s,
        Lang::En => EN,
        Lang::Pt => PT,
    };
    find(table, s).unwrap_or(s)
}

/// Como [`tr`], con datos: cada `{}` del texto se reemplaza por el siguiente de `args`.
pub fn trf(template: &'static str, args: &[&str]) -> String {
    let t = match get() {
        Lang::Es => template,
        Lang::En => find(EN_F, template).unwrap_or(template),
        Lang::Pt => find(PT_F, template).unwrap_or(template),
    };
    let mut out = String::with_capacity(t.len() + 16);
    let mut args = args.iter();
    let mut rest = t;
    while let Some(p) = rest.find("{}") {
        out.push_str(&rest[..p]);
        out.push_str(args.next().copied().unwrap_or(""));
        rest = &rest[p + 2..];
    }
    out.push_str(rest);
    out
}

/// (castellano, inglés), ordenado para buscar rápido.
static EN: &[(&str, &str)] = &[
    ("(RePág/AvPág para moverse)", "(PgUp/PgDn to scroll)"),
    ("(sin guardar)", "(unsaved)"),
    ("(vacío) · Enter para editar", "(empty) · Enter to edit"),
    ("+ ARCHIVO", "+ FILE"),
    ("+ CARPETA", "+ FOLDER"),
    (
        "1 = lento · 3 = normal · 5 = rápido",
        "1 = slow · 3 = normal · 5 = fast",
    ),
    ("A la derecha", "On the right"),
    ("A la derecha del principal", "Right of the main one"),
    ("A la izquierda", "On the left"),
    ("ACCESOS", "QUICK ACCESS"),
    ("ALMACENAMIENTO", "STORAGE"),
    ("APAGAR", "SHUT DOWN"),
    ("APAGAR JARVIS-OS", "SHUT DOWN JARVIS-OS"),
    ("APLICACIONES", "APPS"),
    ("Abajo del principal", "Below the main one"),
    (
        "Abre la terminal con la lista de paquetes (apt list)",
        "Opens the terminal with the package list (apt list)",
    ),
    (
        "Abre la terminal con: apt update && apt upgrade",
        "Opens the terminal with: apt update && apt upgrade",
    ),
    (
        "Abre la terminal con: ufw show blocked",
        "Opens the terminal with: ufw show blocked",
    ),
    ("Abriendo Brave...", "Opening Brave..."),
    ("Abril", "April"),
    ("Acoplar a la derecha", "Snap right"),
    ("Acoplar a la izquierda", "Snap left"),
    ("Acoplar a los bordes", "Snap to edges"),
    ("Activado", "On"),
    ("Agosto", "August"),
    ("Almacenamiento", "Storage"),
    ("Alto contraste", "High contrast"),
    ("Amarillo", "Yellow"),
    ("Animaciones", "Animations"),
    ("Animación al abrir y cerrar", "Open and close animation"),
    ("Anotar lo bloqueado", "Log what's blocked"),
    (
        "Apagada: generá un código acá o escribí el de la otra máquina",
        "Off: generate a code here or type the other machine's",
    ),
    ("Apagar", "Shut down"),
    ("Apagar o reiniciar", "Shut down or restart"),
    (
        "Aparece en la terminal: usuario@equipo",
        "Shown in the terminal: user@computer",
    ),
    ("Apariencia", "Appearance"),
    ("Aplicaciones", "Apps"),
    ("Aplicaciones instaladas", "Installed apps"),
    (
        "Archivo binario: sin vista previa",
        "Binary file: no preview",
    ),
    (
        "Archivo nuevo: Ctrl+S lo crea.",
        "New file: Ctrl+S creates it.",
    ),
    ("Archivos", "Files"),
    ("Archivos (Win+E)", "Files (Win+E)"),
    (
        "Arrastrar al costado ocupa media pantalla; arriba, maximiza",
        "Dragging to a side fills half the screen; to the top, maximizes",
    ),
    ("Arrastrar solo el contorno", "Drag only the outline"),
    ("Azul", "Blue"),
    ("BLOQUEAR", "LOCK"),
    ("BORRAR", "DELETE"),
    ("BORRAR DEFINITIVAMENTE", "DELETE PERMANENTLY"),
    ("BORRAR TODO", "CLEAR ALL"),
    ("BRAVE NO ESTÁ DISPONIBLE", "BRAVE IS NOT AVAILABLE"),
    ("BUSCAR", "CHECK"),
    ("Barra de arriba al maximizar", "Top bar when maximized"),
    ("Barra y cursor", "Bar and cursor"),
    ("Bloqueadas", "Blocked"),
    ("Bloquear", "Lock"),
    ("Bloquear ahora", "Lock now"),
    ("Bloquear sin actividad", "Lock when idle"),
    ("Bloquear un sitio", "Block a site"),
    (
        "Bordes, íconos activos, el cursor de texto y la esfera",
        "Borders, active icons, the text cursor and the sphere",
    ),
    (
        "Borra para siempre lo que hay en /Papelera",
        "Deletes forever what's in /Papelera",
    ),
    ("Borrar = mover a la Papelera", "Delete = move to Trash"),
    ("Botones a la derecha", "Buttons on the right"),
    ("Botones a la izquierda", "Buttons on the left"),
    ("Botones de la barra de título", "Title bar buttons"),
    ("Brave cerró la conexión", "Brave closed the connection"),
    ("Buscador", "Search engine"),
    ("Buscar", "Search"),
    ("Buscar actualizaciones", "Check for updates"),
    (
        "Buscá con Brave o escribí una dirección",
        "Search with Brave or type an address",
    ),
    ("Buscá o escribí una dirección", "Search or type an address"),
    ("CAMBIAR DE VENTANA", "SWITCH WINDOW"),
    ("CANCELAR", "CANCEL"),
    ("CANCIONES · PARLANTE DE LA PC", "SONGS · PC SPEAKER"),
    ("CEREBRO", "BRAIN"),
    ("CERRAR SESIÓN", "SIGN OUT"),
    ("CLIC: MONITOR", "CLICK: MONITOR"),
    ("CONFIGURACIÓN", "SETTINGS"),
    ("CONFIGURACIÓN RÁPIDA", "QUICK SETTINGS"),
    ("CONFIRMAR", "CONFIRM"),
    ("CONTROL DE MISIÓN", "MISSION CONTROL"),
    ("CREADO", "CREATED"),
    ("CREAR", "CREATE"),
    (
        "CTRL+C/X/V · F2 RENOMBRAR · SUPR PAPELERA",
        "CTRL+C/X/V · F2 RENAME · DEL TRASH",
    ),
    ("Canal 2 del PIT (8254)", "PIT channel 2 (8254)"),
    ("Captura (Impr Pant)", "Screenshot (PrtSc)"),
    ("Carga cancelada.", "Loading canceled."),
    ("Carpeta", "Folder"),
    ("Carpeta vacía", "Empty folder"),
    ("Cerrar", "Close"),
    ("Cerrar sesión", "Sign out"),
    ("Cian", "Cyan"),
    ("Claro", "Light"),
    (
        "Clic o Enter: ir a la ventana · Supr: cerrarla · Esc: volver",
        "Click or Enter: go to window · Del: close it · Esc: back",
    ),
    ("Clic para reintentar", "Click to retry"),
    ("Color", "Color"),
    ("Color de acento", "Accent color"),
    (
        "Colores de todo el sistema: ventanas, menús y el escritorio",
        "Colors of the whole system: windows, menus and the desktop",
    ),
    ("Con varios monitores", "With several monitors"),
    (
        "Conectada al relé, esperando a la otra máquina",
        "Connected to the relay, waiting for the other machine",
    ),
    ("Conectada con", "Connected with"),
    ("Conectando al relé...", "Connecting to the relay..."),
    ("Conexiones entrantes", "Incoming connections"),
    ("Conexiones salientes", "Outgoing connections"),
    ("Configuración", "Settings"),
    ("Configuración (Win+I)", "Settings (Win+I)"),
    ("Configuración (prueba de red)", "Settings (network test)"),
    ("Consola JARVIS", "JARVIS console"),
    ("Consola JARVIS (Win+R)", "JARVIS console (Win+R)"),
    (
        "Consola de JARVIS. Escribí \"ayuda\" para ver qué sé hacer.",
        "JARVIS console. Type \"ayuda\" to see what I can do.",
    ),
    (
        "Cuando me conecten con Claude, voy a poder responderte de verdad.",
        "Once I'm connected to Claude, I'll be able to really answer you.",
    ),
    ("Cursor grande", "Large cursor"),
    (
        "Cuánto se mueve la página con cada paso",
        "How far the page moves with each step",
    ),
    ("Código de emparejado", "Pairing code"),
    ("Código nuevo", "New code"),
    ("DESINSTALAR", "UNINSTALL"),
    ("DETENER", "STOP"),
    ("DISCO", "DISK"),
    ("DISTRIBUCIONES (WIN+Z)", "LAYOUTS (WIN+Z)"),
    ("Del tema", "Theme's own"),
    ("Denegar", "Deny"),
    ("Desactivado", "Off"),
    ("Descargas", "Downloads"),
    ("Deslizar", "Slide"),
    (
        "Desplazamiento \"natural\", como en un touchpad",
        "\"Natural\" scrolling, like a touchpad",
    ),
    (
        "Después de un rato sin usar el teclado ni el mouse",
        "After a while without using the keyboard or mouse",
    ),
    ("Desvanecer", "Fade"),
    ("Diciembre", "December"),
    ("Dirección física (MAC)", "Physical address (MAC)"),
    (
        "Dirección y puerto (cargo xtask relay; en QEMU, 10.0.2.2:8120)",
        "Address and port (cargo xtask relay; in QEMU, 10.0.2.2:8120)",
    ),
    (
        "Dirección y puerto (en QEMU el anfitrión es 10.0.2.2)",
        "Address and port (in QEMU the host is 10.0.2.2)",
    ),
    ("Disco", "Disk"),
    ("Disco JARVIS (FAT32)", "JARVIS disk (FAT32)"),
    ("Dispositivo", "Device"),
    ("Distribución del teclado", "Keyboard layout"),
    ("Doble clic en el título", "Title double-click"),
    ("Documentos", "Documents"),
    ("Duplicar", "Duplicate"),
    ("ENLACES RÁPIDOS (WIN+X)", "QUICK LINKS (WIN+X)"),
    ("ESTADO", "STATUS"),
    ("Editor de texto", "Text editor"),
    ("Ejecutar (consola JARVIS)", "Run (JARVIS console)"),
    (
        "Ejemplo: tiktok.com (también bloquea sus subdominios)",
        "Example: tiktok.com (also blocks its subdomains)",
    ),
    (
        "El HUD, un color o una imagen BMP de /Imágenes",
        "The HUD, a color or a BMP picture from /Imágenes",
    ),
    (
        "El archivo es muy grande: se muestra el principio.",
        "The file is too big: showing the beginning.",
    ),
    ("El de la barra de arriba", "The one on the top bar"),
    (
        "El de la barra de íconos, JARVIS y la barra de arriba",
        "The one with the icon bar, JARVIS and the top bar",
    ),
    ("El doble de tamaño", "Twice the size"),
    ("El foco sigue al mouse", "Focus follows mouse"),
    (
        "El kernel todavía no tiene TLS: usa el puente del anfitrión",
        "The kernel has no TLS yet: it uses the host bridge",
    ),
    (
        "El mismo en las dos máquinas (vacío = no sincroniza)",
        "The same on both machines (empty = no sync)",
    ),
    (
        "El nombre de cada ventana en su barra de título",
        "Each window's name in its title bar",
    ),
    (
        "El puente mandó algo que no se entiende",
        "The bridge sent something unreadable",
    ),
    (
        "El que abre la barra: Brave (en el anfitrión) o el simple de JARVIS",
        "The one the bar opens: Brave (on the host) or JARVIS's simple one",
    ),
    (
        "El reloj de la máquina está en UTC",
        "The machine clock is in UTC",
    ),
    ("El resto del sistema", "The rest of the system"),
    ("El segundo monitor va", "The second monitor goes"),
    (
        "En /Sistema/firewall.log (ufw show blocked)",
        "In /Sistema/firewall.log (ufw show blocked)",
    ),
    ("En la barra de arriba", "In the top bar"),
    ("Enero", "January"),
    ("Enter guarda · Esc cancela", "Enter saves · Esc cancels"),
    ("Enter para abrir la carpeta", "Enter to open the folder"),
    (
        "Escribí para buscar apps o la web",
        "Type to search apps or the web",
    ),
    ("Escritorio nuevo", "New desktop"),
    (
        "Ese botón necesita JavaScript.",
        "That button needs JavaScript.",
    ),
    (
        "Ese enlace necesita JavaScript.",
        "That link needs JavaScript.",
    ),
    (
        "Ese valor no sirve (usuario y equipo: letras, números y guiones; PIN: solo números).",
        "That value won't work (user and computer: letters, digits and dashes; PIN: digits only).",
    ),
    ("Español (Latinoamérica)", "Spanish (Latin America)"),
    (
        "Esta página se arma con JavaScript, que JARVIS-OS todavía no ejecuta: puede verse incompleta.",
        "This page is built with JavaScript, which JARVIS-OS can't run yet: it may look incomplete.",
    ),
    ("Estado", "Status"),
    (
        "Este formulario manda datos con POST: el puente solo acepta GET (ver ADR 0004).",
        "This form sends data with POST: the bridge only accepts GET (see ADR 0004).",
    ),
    ("Extender", "Extend"),
    (
        "Extender, duplicar o usar uno solo (también Win+P)",
        "Extend, duplicate or use just one (also Win+P)",
    ),
    ("FINALIZAR", "END TASK"),
    ("Facultad", "University"),
    ("Febrero", "February"),
    ("Firewall", "Firewall"),
    (
        "Flechas o mouse: la zona para esta ventana · Enter la acomoda · las demás completan",
        "Arrows or mouse: the zone for this window · Enter places it · the others fill in",
    ),
    (
        "Fondo blanco como en otros navegadores (si no, oscuro)",
        "White background like other browsers (otherwise dark)",
    ),
    ("Fondo de escritorio", "Desktop background"),
    ("Formato de 24 horas", "24-hour format"),
    ("GENERAR", "GENERATE"),
    ("GUARDAR COMO", "SAVE AS"),
    (
        "Generalo en una máquina y escribilo en la otra",
        "Generate it on one machine and type it on the other",
    ),
    (
        "Gráficos de CPU, memoria, disco y red abajo a la izquierda",
        "CPU, memory, disk and network charts at the bottom left",
    ),
    ("HUD de JARVIS", "JARVIS HUD"),
    ("HUD oscuro", "Dark HUD"),
    (
        "Hay cambios sin guardar: Ctrl+S guarda; cerrá de nuevo para descartarlos.",
        "There are unsaved changes: Ctrl+S saves; close again to discard them.",
    ),
    (
        "Hay un solo monitor: no hay nada que extender o duplicar.",
        "There is only one monitor: nothing to extend or duplicate.",
    ),
    ("Hora e idioma", "Time & language"),
    ("IDENTIFICAR", "IDENTIFY"),
    ("INSPECTOR", "INSPECTOR"),
    ("Identificar", "Identify"),
    ("Idioma", "Language"),
    ("Imágenes", "Pictures"),
    ("Inglés (EE. UU.)", "English (US)"),
    ("Inicio", "Home"),
    ("Inicio (Win)", "Start (Win)"),
    ("Instalar más programas", "Install more programs"),
    ("Invertir la rueda", "Invert the wheel"),
    ("JARVIS · escritorio (Win+D)", "JARVIS · desktop (Win+D)"),
    (
        "JARVIS-OS BLOQUEADO · ESCRIBÍ TU PIN Y APRETÁ ENTER",
        "JARVIS-OS LOCKED · TYPE YOUR PIN AND PRESS ENTER",
    ),
    (
        "JARVIS-OS BLOQUEADO · TOCÁ UNA TECLA O HACÉ CLIC",
        "JARVIS-OS LOCKED · PRESS A KEY OR CLICK",
    ),
    (
        "JARVIS-OS no ofrece servicios: se rechaza lo que no pidió",
        "JARVIS-OS offers no services: anything it didn't ask for is rejected",
    ),
    ("Julio", "July"),
    ("Junio", "June"),
    (
        "Kernel propio en Rust (x86_64, UEFI)",
        "Own kernel written in Rust (x86_64, UEFI)",
    ),
    ("La Papelera está vacía.", "The Trash is empty."),
    (
        "La Papelera no se puede mover a la Papelera.",
        "The Trash can't be moved to the Trash.",
    ),
    (
        "La esfera de JARVIS gira (apagarlo ahorra CPU)",
        "JARVIS's sphere spins (turning it off saves CPU)",
    ),
    (
        "La pantalla del firmware (sin placa virtio-gpu: no se pueden sumar monitores)",
        "The firmware display (no virtio-gpu card: monitors can't be added)",
    ),
    (
        "La ruta tiene que empezar con /",
        "The path must start with /",
    ),
    (
        "La ventana bajo el mouse pasa adelante sin hacer clic",
        "The window under the mouse comes forward without clicking",
    ),
    (
        "La ventana salta a su lugar al soltar (más liviano)",
        "The window jumps into place on release (lighter)",
    ),
    (
        "Las máquinas virtuales no lo emulan;",
        "Virtual machines don't emulate it;",
    ),
    (
        "Latinoamérica: ñ, tildes (´ + vocal) y AltGr+Q = @",
        "Latin American: ñ, accents (´ + vowel) and AltGr+Q = @",
    ),
    ("Lenta", "Slow"),
    ("Letra de la terminal", "Terminal font"),
    ("Letra del editor", "Editor font"),
    ("Listo", "Done"),
    ("Listo · modo lectura (F9)", "Done · reader mode (F9)"),
    ("Lo mismo que Win+L", "Same as Win+L"),
    ("Lo primero que abre Brave", "The first thing Brave opens"),
    (
        "Lo que abre el navegador (about:inicio = la de JARVIS)",
        "What the browser opens (about:inicio = JARVIS's)",
    ),
    (
        "Lo que escribís en la barra que no es una dirección",
        "What you type in the bar that isn't an address",
    ),
    ("Lo que no dice ninguna regla", "What no rule mentions"),
    (
        "Lo que pongas acá aparece en las otras máquinas",
        "What you put here shows up on the other machines",
    ),
    ("Los da el servidor DHCP", "Given by the DHCP server"),
    ("MEMORIA", "MEMORY"),
    ("MEMORIA (HEAP DEL NÚCLEO)", "MEMORY (KERNEL HEAP)"),
    ("MODIFICADO", "MODIFIED"),
    ("MOVER", "MOVE"),
    ("MOVER A LA PAPELERA", "MOVE TO TRASH"),
    ("Marzo", "March"),
    ("Maximizar", "Maximize"),
    ("Mayo", "May"),
    ("Memoria", "Memory"),
    (
        "Menús, listas y botones en 20 px (si no, 16)",
        "Menus, lists and buttons at 20 px (otherwise 16)",
    ),
    (
        "Menús, ventanas y Configuración (la terminal sigue en castellano)",
        "Menus, windows and Settings (the terminal stays in Spanish)",
    ),
    ("Minimizar", "Minimize"),
    (
        "Minimizar, maximizar y cerrar",
        "Minimize, maximize and close",
    ),
    ("Modo lectura", "Reader mode"),
    ("Monitor (Ctrl+Shift+Esc)", "Monitor (Ctrl+Shift+Esc)"),
    ("Monitor del sistema", "System monitor"),
    ("Monitor principal", "Main monitor"),
    ("Monitores", "Monitors"),
    ("Mostrar el escritorio", "Show desktop"),
    ("Mostrar imágenes", "Show images"),
    ("Mouse y teclado", "Mouse & keyboard"),
    (
        "Muestra el número de cada monitor",
        "Shows each monitor's number",
    ),
    ("Música", "Music"),
    ("NO SE PUDO CARGAR", "COULDN'T LOAD"),
    ("NOMBRE", "NAME"),
    ("NOMBRE 8.3", "8.3 NAME"),
    ("NOTIFICACIONES", "NOTIFICATIONS"),
    ("NUEVA CARPETA", "NEW FOLDER"),
    ("NUEVO ARCHIVO DE TEXTO", "NEW TEXT FILE"),
    ("Nada", "Nothing"),
    ("Nada seleccionado", "Nothing selected"),
    ("Naranja", "Orange"),
    ("Navegador", "Browser"),
    ("Navegador principal", "Main browser"),
    ("Navegador simple", "Simple browser"),
    ("Navegador web", "Web browser"),
    (
        "Necesita \"Animaciones\" encendido (Personalización)",
        "Needs \"Animations\" on (Personalization)",
    ),
    (
        "Ni las apps ni la terminal borran para siempre sin preguntar",
        "Neither apps nor the terminal delete forever without asking",
    ),
    ("Ninguna", "None"),
    ("No hay apps abiertas.", "No open apps."),
    (
        "No hay disco para guardar la captura.",
        "There is no disk to save the screenshot.",
    ),
    (
        "No hay disco para guardar la descarga.",
        "There's no disk to save the download.",
    ),
    ("No hay disco.", "No disk."),
    (
        "No hay disco: arrancá QEMU con el disco virtual",
        "No disk: start QEMU with the virtual disk",
    ),
    ("No hay nada copiado.", "Nothing has been copied."),
    ("No hay notificaciones nuevas.", "No new notifications."),
    ("No hay ventanas abiertas.", "No open windows."),
    (
        "No se cerró la sesión: hay cambios sin guardar.",
        "Did not sign out: there are unsaved changes.",
    ),
    ("No se pudo leer la imagen.", "Couldn't read the image."),
    ("Nombre del equipo", "Computer name"),
    ("Nombre:", "Name:"),
    ("Normal", "Normal"),
    ("Noviembre", "November"),
    ("Nueva pestaña", "New tab"),
    ("Nunca", "Never"),
    ("Octubre", "October"),
    ("PAPELERA", "TRASH"),
    ("PEGAR", "PASTE"),
    ("PIN INCORRECTO. PROBÁ OTRA VEZ.", "WRONG PIN. TRY AGAIN."),
    ("PIN de desbloqueo", "Unlock PIN"),
    ("PROBAR", "TEST"),
    ("PROCESADOR", "PROCESSOR"),
    ("PROYECTAR (WIN+P)", "PROJECT (WIN+P)"),
    ("Panel de estado", "Status panel"),
    ("Pantalla", "Display"),
    ("Pantallas", "Displays"),
    ("Papelera", "Trash"),
    (
        "Paquetes de JARVIS-OS (apt, dpkg)",
        "JARVIS-OS packages (apt, dpkg)",
    ),
    (
        "Para pasar el mouse de uno al otro",
        "To move the mouse from one to the other",
    ),
    ("Parlante de la PC", "PC speaker"),
    ("Permitir", "Allow"),
    ("Personalización", "Personalization"),
    (
        "Pide http://info.cern.ch/ por la red propia",
        "Fetches http://info.cern.ch/ over our own network stack",
    ),
    (
        "Pidiendo dirección (DHCP)...",
        "Getting an address (DHCP)...",
    ),
    ("Privacidad y seguridad", "Privacy & security"),
    ("Probando...", "Testing..."),
    ("Probar el parlante", "Test the speaker"),
    ("Probar la conexión", "Test the connection"),
    ("Procesador", "Processor"),
    ("Programas de Windows (winget)", "Windows programs (winget)"),
    ("Programas instalados", "Installed programs"),
    ("Proyectos", "Projects"),
    ("Puente de Brave", "Brave bridge"),
    ("Puerta de enlace y DNS", "Gateway and DNS"),
    ("Página completa", "Full page"),
    ("Página de inicio", "Home page"),
    (
        "Página de inicio (navegador simple)",
        "Home page (simple browser)",
    ),
    ("Página de inicio de Brave", "Brave home page"),
    ("Páginas claras", "Light pages"),
    ("Qué hace con la ventana", "What it does to the window"),
    ("RED", "NETWORK"),
    ("REINICIAR", "RESTART"),
    ("RENDIMIENTO", "PERFORMANCE"),
    ("RENOMBRAR", "RENAME"),
    ("REPRODUCIR", "PLAY"),
    ("RESTAURAR", "RESTORE"),
    ("Red", "Network"),
    ("Red e Internet", "Network & Internet"),
    ("Reiniciar", "Restart"),
    ("Reloj 24 h", "24 h clock"),
    ("Relé", "Relay"),
    (
        "Resolución que eligió el firmware (UEFI GOP)",
        "Resolution chosen by the firmware (UEFI GOP)",
    ),
    ("Restaurar", "Restore"),
    (
        "Revisa cada conexión que sale antes de que llegue a la red",
        "Checks every outgoing connection before it reaches the network",
    ),
    ("Rojo", "Red"),
    ("Rosa", "Pink"),
    ("Rueda del mouse", "Mouse wheel"),
    ("Rápida", "Fast"),
    (
        "SESIÓN CERRADA · ESCRIBÍ TU PIN Y APRETÁ ENTER",
        "SIGNED OUT · TYPE YOUR PIN AND PRESS ENTER",
    ),
    (
        "SESIÓN CERRADA · TOCÁ UNA TECLA O HACÉ CLIC PARA ENTRAR",
        "SIGNED OUT · PRESS A KEY OR CLICK TO SIGN IN",
    ),
    ("SIN DISCO", "NO DISK"),
    ("SIN RED", "NO NETWORK"),
    ("SINCRONIZADO", "SYNCED"),
    ("SONANDO", "PLAYING"),
    ("SUSPENDER", "SLEEP"),
    (
        "Se convierten a BMP en el puente del anfitrión",
        "They're converted to BMP by the host bridge",
    ),
    ("Segundos en el reloj", "Seconds on the clock"),
    ("Septiembre", "September"),
    (
        "Si no, 12 horas con a. m. / p. m.",
        "Otherwise, 12 hours with a.m. / p.m.",
    ),
    ("Siempre", "Always"),
    (
        "Sin conexión con el relé (reintenta sola)",
        "No connection to the relay (retries on its own)",
    ),
    ("Sin disco", "No disk"),
    ("Sin placa de red", "No network adapter"),
    ("Sin sensor térmico.", "No thermal sensor."),
    ("Sin título", "Untitled"),
    ("Sincronización", "Sync"),
    ("Sistema", "System"),
    (
        "Solo el contenido: sin menús, formularios ni estilos",
        "Just the content: no menus, forms or styles",
    ),
    ("Solo la pantalla 1", "Display 1 only"),
    ("Solo la pantalla 2", "Display 2 only"),
    (
        "Solo números (hasta 8). Vacío = sin PIN",
        "Digits only (up to 8). Empty = no PIN",
    ),
    (
        "Solo puedo mostrar imágenes BMP sin compresión (por ahora).",
        "I can only show uncompressed BMP images (for now).",
    ),
    (
        "Solo si el puente corre en otra máquina (--red)",
        "Only if the bridge runs on another machine (--red)",
    ),
    ("Sonido", "Sound"),
    ("Sonidos", "Sounds"),
    ("Sonidos del sistema", "System sounds"),
    ("Suspender", "Sleep"),
    ("TAMAÑO", "SIZE"),
    ("TEMPERATURA", "TEMPERATURE"),
    ("TIPO", "TYPE"),
    ("Tamaño en píxeles", "Size in pixels"),
    (
        "También al maximizar, acoplar y minimizar",
        "Also when maximizing, snapping and minimizing",
    ),
    ("Tema", "Theme"),
    ("Temperatura", "Temperature"),
    ("Terminal (Ctrl+Alt+T)", "Terminal (Ctrl+Alt+T)"),
    ("Terminal (wget, curl)", "Terminal (wget, curl)"),
    ("Texto grande", "Large text"),
    ("Tiempo encendido", "Uptime"),
    ("Tienda de snaps", "Snap store"),
    ("Tipografía", "Typography"),
    (
        "Todavía no instalaste ninguno. Probá: apt install neofetch",
        "You haven't installed any yet. Try: apt install neofetch",
    ),
    (
        "Todavía no tengo voz propia, pero ya sé cómo moverme cuando hable.",
        "I don't have my own voice yet, but I already know how to move when I speak.",
    ),
    (
        "Todo lo del disco ya está guardado",
        "Everything on disk is already saved",
    ),
    (
        "Todo lo que está en la Papelera se borra para siempre.",
        "Everything in the Trash will be deleted forever.",
    ),
    (
        "Todos los sistemas funcionan con normalidad.",
        "All systems are running normally.",
    ),
    ("Token del puente", "Bridge token"),
    ("Tráfico desde el arranque", "Traffic since boot"),
    (
        "Tu nombre de usuario en la terminal",
        "Your user name in the terminal",
    ),
    ("Títulos en negrita", "Bold titles"),
    (
        "Un La (440 Hz) por el parlante de la PC",
        "An A (440 Hz) through the PC speaker",
    ),
    (
        "Un pitido corto con los avisos de error",
        "A short beep with error messages",
    ),
    ("Usuario", "User"),
    ("VACIAR", "EMPTY"),
    ("VACIAR LA PAPELERA", "EMPTY TRASH"),
    ("VENTANA (ALT+ESPACIO)", "WINDOW (ALT+SPACE)"),
    ("VER", "VIEW"),
    ("VISTA PREVIA", "PREVIEW"),
    ("Vaciar la Papelera", "Empty the Trash"),
    ("Velocidad de las animaciones", "Animation speed"),
    ("Velocidad del puntero", "Pointer speed"),
    ("Ventanas", "Windows"),
    (
        "Ventanas abiertas, gráficos, IP y hora arriba de todo",
        "Open windows, graphs, IP and time at the very top",
    ),
    ("Ver lo bloqueado", "View what's blocked"),
    ("Verde", "Green"),
    ("Versión", "Version"),
    ("Violeta", "Violet"),
    ("Visor de imágenes", "Image viewer"),
    ("Vuelve a arrancar la máquina", "Reboots the machine"),
    ("Win+Abajo", "Win+Down"),
    ("Win+Arriba", "Win+Up"),
    ("Win+Der.", "Win+Right"),
    (
        "Win+E abre Archivos, Alt+Tab cambia de ventana y Win+D vuelve conmigo.",
        "Win+E opens Files, Alt+Tab switches windows and Win+D brings you back to me.",
    ),
    ("Win+Izq.", "Win+Left"),
    ("Zona horaria", "Time zone"),
    ("Zoom", "Zoom"),
    ("archivo", "file"),
    ("archivo comprimido", "archive"),
    ("carpeta", "folder"),
    ("código", "code"),
    ("do", "su"),
    ("en ejecución", "running"),
    (
        "en una PC Intel se lee de la CPU.",
        "on an Intel PC it is read from the CPU.",
    ),
    ("enviados", "sent"),
    ("imagen", "image"),
    ("ju", "th"),
    ("lu", "mo"),
    ("ma", "tu"),
    ("mi", "we"),
    ("minimizada", "minimized"),
    ("papelera", "trash"),
    ("recibidos", "received"),
    ("sin conectar", "not connected"),
    ("sin disco", "no disk"),
    ("sin placa", "no adapter"),
    ("sin sensor", "no sensor"),
    ("sá", "sa"),
    ("texto", "text"),
    ("vi", "fr"),
    (
        "¿Qué querés que haga la computadora?",
        "What do you want the computer to do?",
    ),
    (
        "¿Seguro? Se borra para siempre: hacé clic otra vez",
        "Sure? It's deleted forever: click again",
    ),
];

/// (castellano, portugués)
static PT: &[(&str, &str)] = &[
    ("(RePág/AvPág para moverse)", "(PgUp/PgDn para rolar)"),
    ("(sin guardar)", "(não salvo)"),
    ("(vacío) · Enter para editar", "(vazio) · Enter para editar"),
    ("+ ARCHIVO", "+ ARQUIVO"),
    ("+ CARPETA", "+ PASTA"),
    (
        "1 = lento · 3 = normal · 5 = rápido",
        "1 = lento · 3 = normal · 5 = rápido",
    ),
    ("A la derecha", "À direita"),
    ("A la derecha del principal", "À direita do principal"),
    ("A la izquierda", "À esquerda"),
    ("ACCESOS", "ACESSO RÁPIDO"),
    ("ALMACENAMIENTO", "ARMAZENAMENTO"),
    ("APAGAR", "DESLIGAR"),
    ("APAGAR JARVIS-OS", "DESLIGAR JARVIS-OS"),
    ("APLICACIONES", "APLICATIVOS"),
    ("Abajo del principal", "Abaixo do principal"),
    (
        "Abre la terminal con la lista de paquetes (apt list)",
        "Abre o terminal com a lista de pacotes (apt list)",
    ),
    (
        "Abre la terminal con: apt update && apt upgrade",
        "Abre o terminal com: apt update && apt upgrade",
    ),
    (
        "Abre la terminal con: ufw show blocked",
        "Abre o terminal com: ufw show blocked",
    ),
    ("Abriendo Brave...", "Abrindo o Brave..."),
    ("Abril", "Abril"),
    ("Acoplar a la derecha", "Encaixar à direita"),
    ("Acoplar a la izquierda", "Encaixar à esquerda"),
    ("Acoplar a los bordes", "Encaixar nas bordas"),
    ("Activado", "Ativado"),
    ("Agosto", "Agosto"),
    ("Almacenamiento", "Armazenamento"),
    ("Alto contraste", "Alto contraste"),
    ("Amarillo", "Amarelo"),
    ("Animaciones", "Animações"),
    ("Animación al abrir y cerrar", "Animação ao abrir e fechar"),
    ("Anotar lo bloqueado", "Registrar o bloqueado"),
    (
        "Apagada: generá un código acá o escribí el de la otra máquina",
        "Desligada: gere um código aqui ou digite o da outra máquina",
    ),
    ("Apagar", "Desligar"),
    ("Apagar o reiniciar", "Desligar ou reiniciar"),
    (
        "Aparece en la terminal: usuario@equipo",
        "Aparece no terminal: usuario@computador",
    ),
    ("Apariencia", "Aparência"),
    ("Aplicaciones", "Aplicativos"),
    ("Aplicaciones instaladas", "Aplicativos instalados"),
    (
        "Archivo binario: sin vista previa",
        "Arquivo binário: sem prévia",
    ),
    (
        "Archivo nuevo: Ctrl+S lo crea.",
        "Arquivo novo: Ctrl+S o cria.",
    ),
    ("Archivos", "Arquivos"),
    ("Archivos (Win+E)", "Arquivos (Win+E)"),
    (
        "Arrastrar al costado ocupa media pantalla; arriba, maximiza",
        "Arrastar para o lado ocupa meia tela; para cima, maximiza",
    ),
    ("Arrastrar solo el contorno", "Arrastar só o contorno"),
    ("Azul", "Azul"),
    ("BLOQUEAR", "BLOQUEAR"),
    ("BORRAR", "APAGAR"),
    ("BORRAR DEFINITIVAMENTE", "APAGAR DEFINITIVAMENTE"),
    ("BORRAR TODO", "LIMPAR TUDO"),
    ("BRAVE NO ESTÁ DISPONIBLE", "BRAVE NÃO ESTÁ DISPONÍVEL"),
    ("BUSCAR", "PROCURAR"),
    (
        "Barra de arriba al maximizar",
        "Barra superior ao maximizar",
    ),
    ("Barra y cursor", "Barra e cursor"),
    ("Bloqueadas", "Bloqueadas"),
    ("Bloquear", "Bloquear"),
    ("Bloquear ahora", "Bloquear agora"),
    ("Bloquear sin actividad", "Bloquear sem atividade"),
    ("Bloquear un sitio", "Bloquear um site"),
    (
        "Bordes, íconos activos, el cursor de texto y la esfera",
        "Bordas, ícones ativos, o cursor de texto e a esfera",
    ),
    (
        "Borra para siempre lo que hay en /Papelera",
        "Apaga para sempre o que há em /Papelera",
    ),
    (
        "Borrar = mover a la Papelera",
        "Apagar = mover para a Lixeira",
    ),
    ("Botones a la derecha", "Botões à direita"),
    ("Botones a la izquierda", "Botões à esquerda"),
    ("Botones de la barra de título", "Botões da barra de título"),
    ("Brave cerró la conexión", "O Brave fechou a conexão"),
    ("Buscador", "Buscador"),
    ("Buscar", "Buscar"),
    ("Buscar actualizaciones", "Procurar atualizações"),
    (
        "Buscá con Brave o escribí una dirección",
        "Pesquise com o Brave ou digite um endereço",
    ),
    (
        "Buscá o escribí una dirección",
        "Busque ou digite um endereço",
    ),
    ("CAMBIAR DE VENTANA", "TROCAR DE JANELA"),
    ("CANCELAR", "CANCELAR"),
    (
        "CANCIONES · PARLANTE DE LA PC",
        "MÚSICAS · ALTO-FALANTE DO PC",
    ),
    ("CEREBRO", "CÉREBRO"),
    ("CERRAR SESIÓN", "SAIR"),
    ("CLIC: MONITOR", "CLIQUE: MONITOR"),
    ("CONFIGURACIÓN", "CONFIGURAÇÕES"),
    ("CONFIGURACIÓN RÁPIDA", "CONFIGURAÇÕES RÁPIDAS"),
    ("CONFIRMAR", "CONFIRMAR"),
    ("CONTROL DE MISIÓN", "CONTROLE DE MISSÃO"),
    ("CREADO", "CRIADO"),
    ("CREAR", "CRIAR"),
    (
        "CTRL+C/X/V · F2 RENOMBRAR · SUPR PAPELERA",
        "CTRL+C/X/V · F2 RENOMEAR · DEL LIXEIRA",
    ),
    ("Canal 2 del PIT (8254)", "Canal 2 do PIT (8254)"),
    ("Captura (Impr Pant)", "Captura (PrtSc)"),
    ("Carga cancelada.", "Carregamento cancelado."),
    ("Carpeta", "Pasta"),
    ("Carpeta vacía", "Pasta vazia"),
    ("Cerrar", "Fechar"),
    ("Cerrar sesión", "Sair"),
    ("Cian", "Ciano"),
    ("Claro", "Claro"),
    (
        "Clic o Enter: ir a la ventana · Supr: cerrarla · Esc: volver",
        "Clique ou Enter: ir para a janela · Del: fechá-la · Esc: voltar",
    ),
    ("Clic para reintentar", "Clique para tentar de novo"),
    ("Color", "Cor"),
    ("Color de acento", "Cor de destaque"),
    (
        "Colores de todo el sistema: ventanas, menús y el escritorio",
        "Cores de todo o sistema: janelas, menus e a área de trabalho",
    ),
    ("Con varios monitores", "Com vários monitores"),
    (
        "Conectada al relé, esperando a la otra máquina",
        "Conectada ao relé, esperando a outra máquina",
    ),
    ("Conectada con", "Conectada com"),
    ("Conectando al relé...", "Conectando ao relé..."),
    ("Conexiones entrantes", "Conexões de entrada"),
    ("Conexiones salientes", "Conexões de saída"),
    ("Configuración", "Configurações"),
    ("Configuración (Win+I)", "Configurações (Win+I)"),
    (
        "Configuración (prueba de red)",
        "Configurações (teste de rede)",
    ),
    ("Consola JARVIS", "Console JARVIS"),
    ("Consola JARVIS (Win+R)", "Console JARVIS (Win+R)"),
    (
        "Consola de JARVIS. Escribí \"ayuda\" para ver qué sé hacer.",
        "Console do JARVIS. Digite \"ayuda\" para ver o que sei fazer.",
    ),
    (
        "Cuando me conecten con Claude, voy a poder responderte de verdad.",
        "Quando me conectarem ao Claude, vou poder te responder de verdade.",
    ),
    ("Cursor grande", "Cursor grande"),
    (
        "Cuánto se mueve la página con cada paso",
        "Quanto a página se move a cada passo",
    ),
    ("Código de emparejado", "Código de pareamento"),
    ("Código nuevo", "Código novo"),
    ("DESINSTALAR", "DESINSTALAR"),
    ("DETENER", "PARAR"),
    ("DISCO", "DISCO"),
    ("DISTRIBUCIONES (WIN+Z)", "LAYOUTS (WIN+Z)"),
    ("Del tema", "Do tema"),
    ("Denegar", "Negar"),
    ("Desactivado", "Desativado"),
    ("Descargas", "Downloads"),
    ("Deslizar", "Deslizar"),
    (
        "Desplazamiento \"natural\", como en un touchpad",
        "Rolagem \"natural\", como num touchpad",
    ),
    (
        "Después de un rato sin usar el teclado ni el mouse",
        "Depois de um tempo sem usar o teclado nem o mouse",
    ),
    ("Desvanecer", "Esmaecer"),
    ("Diciembre", "Dezembro"),
    ("Dirección física (MAC)", "Endereço físico (MAC)"),
    (
        "Dirección y puerto (cargo xtask relay; en QEMU, 10.0.2.2:8120)",
        "Endereço e porta (cargo xtask relay; no QEMU, 10.0.2.2:8120)",
    ),
    (
        "Dirección y puerto (en QEMU el anfitrión es 10.0.2.2)",
        "Endereço e porta (no QEMU o anfitrião é 10.0.2.2)",
    ),
    ("Disco", "Disco"),
    ("Disco JARVIS (FAT32)", "Disco JARVIS (FAT32)"),
    ("Dispositivo", "Dispositivo"),
    ("Distribución del teclado", "Layout do teclado"),
    ("Doble clic en el título", "Clique duplo no título"),
    ("Documentos", "Documentos"),
    ("Duplicar", "Duplicar"),
    ("ENLACES RÁPIDOS (WIN+X)", "LINKS RÁPIDOS (WIN+X)"),
    ("ESTADO", "ESTADO"),
    ("Editor de texto", "Editor de texto"),
    ("Ejecutar (consola JARVIS)", "Executar (console JARVIS)"),
    (
        "Ejemplo: tiktok.com (también bloquea sus subdominios)",
        "Exemplo: tiktok.com (também bloqueia seus subdomínios)",
    ),
    (
        "El HUD, un color o una imagen BMP de /Imágenes",
        "O HUD, uma cor ou uma imagem BMP de /Imágenes",
    ),
    (
        "El archivo es muy grande: se muestra el principio.",
        "O arquivo é muito grande: mostrando o começo.",
    ),
    ("El de la barra de arriba", "O da barra superior"),
    (
        "El de la barra de íconos, JARVIS y la barra de arriba",
        "O da barra de ícones, do JARVIS e da barra superior",
    ),
    ("El doble de tamaño", "O dobro do tamanho"),
    ("El foco sigue al mouse", "O foco segue o mouse"),
    (
        "El kernel todavía no tiene TLS: usa el puente del anfitrión",
        "O kernel ainda não tem TLS: usa a ponte do anfitrião",
    ),
    (
        "El mismo en las dos máquinas (vacío = no sincroniza)",
        "O mesmo nas duas máquinas (vazio = não sincroniza)",
    ),
    (
        "El nombre de cada ventana en su barra de título",
        "O nome de cada janela na barra de título",
    ),
    (
        "El puente mandó algo que no se entiende",
        "A ponte mandou algo que não se entende",
    ),
    (
        "El que abre la barra: Brave (en el anfitrión) o el simple de JARVIS",
        "O que a barra abre: Brave (no anfitrião) ou o simples do JARVIS",
    ),
    (
        "El reloj de la máquina está en UTC",
        "O relógio da máquina está em UTC",
    ),
    ("El resto del sistema", "O resto do sistema"),
    ("El segundo monitor va", "O segundo monitor fica"),
    (
        "En /Sistema/firewall.log (ufw show blocked)",
        "Em /Sistema/firewall.log (ufw show blocked)",
    ),
    ("En la barra de arriba", "Na barra superior"),
    ("Enero", "Janeiro"),
    ("Enter guarda · Esc cancela", "Enter salva · Esc cancela"),
    ("Enter para abrir la carpeta", "Enter para abrir a pasta"),
    (
        "Escribí para buscar apps o la web",
        "Digite para buscar apps ou na web",
    ),
    ("Escritorio nuevo", "Nova área de trabalho"),
    (
        "Ese botón necesita JavaScript.",
        "Esse botão precisa de JavaScript.",
    ),
    (
        "Ese enlace necesita JavaScript.",
        "Esse link precisa de JavaScript.",
    ),
    (
        "Ese valor no sirve (usuario y equipo: letras, números y guiones; PIN: solo números).",
        "Esse valor não serve (usuário e computador: letras, números e hifens; PIN: só números).",
    ),
    ("Español (Latinoamérica)", "Espanhol (América Latina)"),
    (
        "Esta página se arma con JavaScript, que JARVIS-OS todavía no ejecuta: puede verse incompleta.",
        "Esta página é montada com JavaScript, que o JARVIS-OS ainda não executa: pode parecer incompleta.",
    ),
    ("Estado", "Status"),
    (
        "Este formulario manda datos con POST: el puente solo acepta GET (ver ADR 0004).",
        "Este formulário envia dados com POST: a ponte só aceita GET (veja ADR 0004).",
    ),
    ("Extender", "Estender"),
    (
        "Extender, duplicar o usar uno solo (también Win+P)",
        "Estender, duplicar ou usar só um (também Win+P)",
    ),
    ("FINALIZAR", "FINALIZAR"),
    ("Facultad", "Faculdade"),
    ("Febrero", "Fevereiro"),
    ("Firewall", "Firewall"),
    (
        "Flechas o mouse: la zona para esta ventana · Enter la acomoda · las demás completan",
        "Setas ou mouse: a zona para esta janela · Enter a posiciona · as outras completam",
    ),
    (
        "Fondo blanco como en otros navegadores (si no, oscuro)",
        "Fundo branco como em outros navegadores (se não, escuro)",
    ),
    ("Fondo de escritorio", "Papel de parede"),
    ("Formato de 24 horas", "Formato de 24 horas"),
    ("GENERAR", "GERAR"),
    ("GUARDAR COMO", "SALVAR COMO"),
    (
        "Generalo en una máquina y escribilo en la otra",
        "Gere em uma máquina e digite na outra",
    ),
    (
        "Gráficos de CPU, memoria, disco y red abajo a la izquierda",
        "Gráficos de CPU, memória, disco e rede no canto inferior esquerdo",
    ),
    ("HUD de JARVIS", "HUD do JARVIS"),
    ("HUD oscuro", "HUD escuro"),
    (
        "Hay cambios sin guardar: Ctrl+S guarda; cerrá de nuevo para descartarlos.",
        "Há alterações não salvas: Ctrl+S salva; feche de novo para descartá-las.",
    ),
    (
        "Hay un solo monitor: no hay nada que extender o duplicar.",
        "Há um só monitor: não há o que estender ou duplicar.",
    ),
    ("Hora e idioma", "Hora e idioma"),
    ("IDENTIFICAR", "IDENTIFICAR"),
    ("INSPECTOR", "INSPETOR"),
    ("Identificar", "Identificar"),
    ("Idioma", "Idioma"),
    ("Imágenes", "Imagens"),
    ("Inglés (EE. UU.)", "Inglês (EUA)"),
    ("Inicio", "Início"),
    ("Inicio (Win)", "Iniciar (Win)"),
    ("Instalar más programas", "Instalar mais programas"),
    ("Invertir la rueda", "Inverter a roda"),
    (
        "JARVIS · escritorio (Win+D)",
        "JARVIS · área de trabalho (Win+D)",
    ),
    (
        "JARVIS-OS BLOQUEADO · ESCRIBÍ TU PIN Y APRETÁ ENTER",
        "JARVIS-OS BLOQUEADO · DIGITE SEU PIN E APERTE ENTER",
    ),
    (
        "JARVIS-OS BLOQUEADO · TOCÁ UNA TECLA O HACÉ CLIC",
        "JARVIS-OS BLOQUEADO · APERTE UMA TECLA OU CLIQUE",
    ),
    (
        "JARVIS-OS no ofrece servicios: se rechaza lo que no pidió",
        "JARVIS-OS não oferece serviços: rejeita o que não pediu",
    ),
    ("Julio", "Julho"),
    ("Junio", "Junho"),
    (
        "Kernel propio en Rust (x86_64, UEFI)",
        "Kernel próprio em Rust (x86_64, UEFI)",
    ),
    ("La Papelera está vacía.", "A Lixeira está vazia."),
    (
        "La Papelera no se puede mover a la Papelera.",
        "A Lixeira não pode ser movida para a Lixeira.",
    ),
    (
        "La esfera de JARVIS gira (apagarlo ahorra CPU)",
        "A esfera do JARVIS gira (desligar economiza CPU)",
    ),
    (
        "La pantalla del firmware (sin placa virtio-gpu: no se pueden sumar monitores)",
        "A tela do firmware (sem placa virtio-gpu: não dá para somar monitores)",
    ),
    (
        "La ruta tiene que empezar con /",
        "O caminho tem que começar com /",
    ),
    (
        "La ventana bajo el mouse pasa adelante sin hacer clic",
        "A janela sob o mouse vem para frente sem clicar",
    ),
    (
        "La ventana salta a su lugar al soltar (más liviano)",
        "A janela pula para o lugar ao soltar (mais leve)",
    ),
    (
        "Las máquinas virtuales no lo emulan;",
        "As máquinas virtuais não o emulam;",
    ),
    (
        "Latinoamérica: ñ, tildes (´ + vocal) y AltGr+Q = @",
        "América Latina: ñ, acentos (´ + vogal) e AltGr+Q = @",
    ),
    ("Lenta", "Lenta"),
    ("Letra de la terminal", "Fonte do terminal"),
    ("Letra del editor", "Fonte do editor"),
    ("Listo", "Pronto"),
    ("Listo · modo lectura (F9)", "Pronto · modo leitura (F9)"),
    ("Lo mismo que Win+L", "O mesmo que Win+L"),
    (
        "Lo primero que abre Brave",
        "A primeira coisa que o Brave abre",
    ),
    (
        "Lo que abre el navegador (about:inicio = la de JARVIS)",
        "O que o navegador abre (about:inicio = a do JARVIS)",
    ),
    (
        "Lo que escribís en la barra que no es una dirección",
        "O que você digita na barra que não é um endereço",
    ),
    (
        "Lo que no dice ninguna regla",
        "O que nenhuma regra menciona",
    ),
    (
        "Lo que pongas acá aparece en las otras máquinas",
        "O que você colocar aqui aparece nas outras máquinas",
    ),
    ("Los da el servidor DHCP", "Fornecidos pelo servidor DHCP"),
    ("MEMORIA", "MEMÓRIA"),
    ("MEMORIA (HEAP DEL NÚCLEO)", "MEMÓRIA (HEAP DO NÚCLEO)"),
    ("MODIFICADO", "MODIFICADO"),
    ("MOVER", "MOVER"),
    ("MOVER A LA PAPELERA", "MOVER PARA A LIXEIRA"),
    ("Marzo", "Março"),
    ("Maximizar", "Maximizar"),
    ("Mayo", "Maio"),
    ("Memoria", "Memória"),
    (
        "Menús, listas y botones en 20 px (si no, 16)",
        "Menus, listas e botões em 20 px (senão, 16)",
    ),
    (
        "Menús, ventanas y Configuración (la terminal sigue en castellano)",
        "Menus, janelas e Configurações (o terminal continua em espanhol)",
    ),
    ("Minimizar", "Minimizar"),
    (
        "Minimizar, maximizar y cerrar",
        "Minimizar, maximizar e fechar",
    ),
    ("Modo lectura", "Modo leitura"),
    ("Monitor (Ctrl+Shift+Esc)", "Monitor (Ctrl+Shift+Esc)"),
    ("Monitor del sistema", "Monitor do sistema"),
    ("Monitor principal", "Monitor principal"),
    ("Monitores", "Monitores"),
    ("Mostrar el escritorio", "Mostrar a área de trabalho"),
    ("Mostrar imágenes", "Mostrar imagens"),
    ("Mouse y teclado", "Mouse e teclado"),
    (
        "Muestra el número de cada monitor",
        "Mostra o número de cada monitor",
    ),
    ("Música", "Música"),
    ("NO SE PUDO CARGAR", "NÃO FOI POSSÍVEL CARREGAR"),
    ("NOMBRE", "NOME"),
    ("NOMBRE 8.3", "NOME 8.3"),
    ("NOTIFICACIONES", "NOTIFICAÇÕES"),
    ("NUEVA CARPETA", "NOVA PASTA"),
    ("NUEVO ARCHIVO DE TEXTO", "NOVO ARQUIVO DE TEXTO"),
    ("Nada", "Nada"),
    ("Nada seleccionado", "Nada selecionado"),
    ("Naranja", "Laranja"),
    ("Navegador", "Navegador"),
    ("Navegador principal", "Navegador principal"),
    ("Navegador simple", "Navegador simples"),
    ("Navegador web", "Navegador web"),
    (
        "Necesita \"Animaciones\" encendido (Personalización)",
        "Precisa de \"Animações\" ligado (Personalização)",
    ),
    (
        "Ni las apps ni la terminal borran para siempre sin preguntar",
        "Nem os apps nem o terminal apagam para sempre sem perguntar",
    ),
    ("Ninguna", "Nenhuma"),
    ("No hay apps abiertas.", "Nenhum app aberto."),
    (
        "No hay disco para guardar la captura.",
        "Não há disco para salvar a captura.",
    ),
    (
        "No hay disco para guardar la descarga.",
        "Não há disco para salvar o download.",
    ),
    ("No hay disco.", "Sem disco."),
    (
        "No hay disco: arrancá QEMU con el disco virtual",
        "Sem disco: inicie o QEMU com o disco virtual",
    ),
    ("No hay nada copiado.", "Nada foi copiado."),
    ("No hay notificaciones nuevas.", "Nenhuma notificação nova."),
    ("No hay ventanas abiertas.", "Nenhuma janela aberta."),
    (
        "No se cerró la sesión: hay cambios sin guardar.",
        "A sessão não foi encerrada: há alterações não salvas.",
    ),
    (
        "No se pudo leer la imagen.",
        "Não foi possível ler a imagem.",
    ),
    ("Nombre del equipo", "Nome do computador"),
    ("Nombre:", "Nome:"),
    ("Normal", "Normal"),
    ("Noviembre", "Novembro"),
    ("Nueva pestaña", "Nova aba"),
    ("Nunca", "Nunca"),
    ("Octubre", "Outubro"),
    ("PAPELERA", "LIXEIRA"),
    ("PEGAR", "COLAR"),
    (
        "PIN INCORRECTO. PROBÁ OTRA VEZ.",
        "PIN INCORRETO. TENTE DE NOVO.",
    ),
    ("PIN de desbloqueo", "PIN de desbloqueio"),
    ("PROBAR", "TESTAR"),
    ("PROCESADOR", "PROCESSADOR"),
    ("PROYECTAR (WIN+P)", "PROJETAR (WIN+P)"),
    ("Panel de estado", "Painel de status"),
    ("Pantalla", "Tela"),
    ("Pantallas", "Telas"),
    ("Papelera", "Lixeira"),
    (
        "Paquetes de JARVIS-OS (apt, dpkg)",
        "Pacotes do JARVIS-OS (apt, dpkg)",
    ),
    (
        "Para pasar el mouse de uno al otro",
        "Para passar o mouse de um para o outro",
    ),
    ("Parlante de la PC", "Alto-falante do PC"),
    ("Permitir", "Permitir"),
    ("Personalización", "Personalização"),
    (
        "Pide http://info.cern.ch/ por la red propia",
        "Pede http://info.cern.ch/ pela rede própria",
    ),
    ("Pidiendo dirección (DHCP)...", "Pedindo endereço (DHCP)..."),
    ("Privacidad y seguridad", "Privacidade e segurança"),
    ("Probando...", "Testando..."),
    ("Probar el parlante", "Testar o alto-falante"),
    ("Probar la conexión", "Testar a conexão"),
    ("Procesador", "Processador"),
    (
        "Programas de Windows (winget)",
        "Programas do Windows (winget)",
    ),
    ("Programas instalados", "Programas instalados"),
    ("Proyectos", "Projetos"),
    ("Puente de Brave", "Ponte do Brave"),
    ("Puerta de enlace y DNS", "Gateway e DNS"),
    ("Página completa", "Página completa"),
    ("Página de inicio", "Página inicial"),
    (
        "Página de inicio (navegador simple)",
        "Página inicial (navegador simples)",
    ),
    ("Página de inicio de Brave", "Página inicial do Brave"),
    ("Páginas claras", "Páginas claras"),
    ("Qué hace con la ventana", "O que faz com a janela"),
    ("RED", "REDE"),
    ("REINICIAR", "REINICIAR"),
    ("RENDIMIENTO", "DESEMPENHO"),
    ("RENOMBRAR", "RENOMEAR"),
    ("REPRODUCIR", "TOCAR"),
    ("RESTAURAR", "RESTAURAR"),
    ("Red", "Rede"),
    ("Red e Internet", "Rede e Internet"),
    ("Reiniciar", "Reiniciar"),
    ("Reloj 24 h", "Relógio 24 h"),
    ("Relé", "Relé"),
    (
        "Resolución que eligió el firmware (UEFI GOP)",
        "Resolução escolhida pelo firmware (UEFI GOP)",
    ),
    ("Restaurar", "Restaurar"),
    (
        "Revisa cada conexión que sale antes de que llegue a la red",
        "Verifica cada conexão de saída antes de chegar à rede",
    ),
    ("Rojo", "Vermelho"),
    ("Rosa", "Rosa"),
    ("Rueda del mouse", "Roda do mouse"),
    ("Rápida", "Rápida"),
    (
        "SESIÓN CERRADA · ESCRIBÍ TU PIN Y APRETÁ ENTER",
        "SESSÃO ENCERRADA · DIGITE SEU PIN E APERTE ENTER",
    ),
    (
        "SESIÓN CERRADA · TOCÁ UNA TECLA O HACÉ CLIC PARA ENTRAR",
        "SESSÃO ENCERRADA · TOQUE UMA TECLA OU CLIQUE PARA ENTRAR",
    ),
    ("SIN DISCO", "SEM DISCO"),
    ("SIN RED", "SEM REDE"),
    ("SINCRONIZADO", "SINCRONIZADO"),
    ("SONANDO", "TOCANDO"),
    ("SUSPENDER", "SUSPENDER"),
    (
        "Se convierten a BMP en el puente del anfitrión",
        "São convertidas para BMP na ponte do anfitrião",
    ),
    ("Segundos en el reloj", "Segundos no relógio"),
    ("Septiembre", "Setembro"),
    (
        "Si no, 12 horas con a. m. / p. m.",
        "Se não, 12 horas com a.m. / p.m.",
    ),
    ("Siempre", "Sempre"),
    (
        "Sin conexión con el relé (reintenta sola)",
        "Sem conexão com o relé (tenta de novo sozinha)",
    ),
    ("Sin disco", "Sem disco"),
    ("Sin placa de red", "Sem placa de rede"),
    ("Sin sensor térmico.", "Sem sensor térmico."),
    ("Sin título", "Sem título"),
    ("Sincronización", "Sincronização"),
    ("Sistema", "Sistema"),
    (
        "Solo el contenido: sin menús, formularios ni estilos",
        "Só o conteúdo: sem menus, formulários nem estilos",
    ),
    ("Solo la pantalla 1", "Só a tela 1"),
    ("Solo la pantalla 2", "Só a tela 2"),
    (
        "Solo números (hasta 8). Vacío = sin PIN",
        "Só números (até 8). Vazio = sem PIN",
    ),
    (
        "Solo puedo mostrar imágenes BMP sin compresión (por ahora).",
        "Só consigo mostrar imagens BMP sem compressão (por enquanto).",
    ),
    (
        "Solo si el puente corre en otra máquina (--red)",
        "Só se a ponte roda em outra máquina (--red)",
    ),
    ("Sonido", "Som"),
    ("Sonidos", "Sons"),
    ("Sonidos del sistema", "Sons do sistema"),
    ("Suspender", "Suspender"),
    ("TAMAÑO", "TAMANHO"),
    ("TEMPERATURA", "TEMPERATURA"),
    ("TIPO", "TIPO"),
    ("Tamaño en píxeles", "Tamanho em pixels"),
    (
        "También al maximizar, acoplar y minimizar",
        "Também ao maximizar, encaixar e minimizar",
    ),
    ("Tema", "Tema"),
    ("Temperatura", "Temperatura"),
    ("Terminal (Ctrl+Alt+T)", "Terminal (Ctrl+Alt+T)"),
    ("Terminal (wget, curl)", "Terminal (wget, curl)"),
    ("Texto grande", "Texto grande"),
    ("Tiempo encendido", "Tempo ligado"),
    ("Tienda de snaps", "Loja de snaps"),
    ("Tipografía", "Tipografia"),
    (
        "Todavía no instalaste ninguno. Probá: apt install neofetch",
        "Você ainda não instalou nenhum. Tente: apt install neofetch",
    ),
    (
        "Todavía no tengo voz propia, pero ya sé cómo moverme cuando hable.",
        "Ainda não tenho voz própria, mas já sei como me mover quando falo.",
    ),
    (
        "Todo lo del disco ya está guardado",
        "Tudo no disco já está salvo",
    ),
    (
        "Todo lo que está en la Papelera se borra para siempre.",
        "Tudo o que está na Lixeira será apagado para sempre.",
    ),
    (
        "Todos los sistemas funcionan con normalidad.",
        "Todos os sistemas funcionam normalmente.",
    ),
    ("Token del puente", "Token da ponte"),
    ("Tráfico desde el arranque", "Tráfego desde a inicialização"),
    (
        "Tu nombre de usuario en la terminal",
        "Seu nome de usuário no terminal",
    ),
    ("Títulos en negrita", "Títulos em negrito"),
    (
        "Un La (440 Hz) por el parlante de la PC",
        "Um Lá (440 Hz) pelo alto-falante do PC",
    ),
    (
        "Un pitido corto con los avisos de error",
        "Um bipe curto com os avisos de erro",
    ),
    ("Usuario", "Usuário"),
    ("VACIAR", "ESVAZIAR"),
    ("VACIAR LA PAPELERA", "ESVAZIAR A LIXEIRA"),
    ("VENTANA (ALT+ESPACIO)", "JANELA (ALT+ESPAÇO)"),
    ("VER", "VER"),
    ("VISTA PREVIA", "PRÉ-VISUALIZAÇÃO"),
    ("Vaciar la Papelera", "Esvaziar a Lixeira"),
    ("Velocidad de las animaciones", "Velocidade das animações"),
    ("Velocidad del puntero", "Velocidade do ponteiro"),
    ("Ventanas", "Janelas"),
    (
        "Ventanas abiertas, gráficos, IP y hora arriba de todo",
        "Janelas abertas, gráficos, IP e hora no topo",
    ),
    ("Ver lo bloqueado", "Ver o bloqueado"),
    ("Verde", "Verde"),
    ("Versión", "Versão"),
    ("Violeta", "Violeta"),
    ("Visor de imágenes", "Visualizador de imagens"),
    ("Vuelve a arrancar la máquina", "Reinicia a máquina"),
    ("Win+Abajo", "Win+Baixo"),
    ("Win+Arriba", "Win+Cima"),
    ("Win+Der.", "Win+Dir."),
    (
        "Win+E abre Archivos, Alt+Tab cambia de ventana y Win+D vuelve conmigo.",
        "Win+E abre Arquivos, Alt+Tab troca de janela e Win+D volta para mim.",
    ),
    ("Win+Izq.", "Win+Esq."),
    ("Zona horaria", "Fuso horário"),
    ("Zoom", "Zoom"),
    ("archivo", "arquivo"),
    ("archivo comprimido", "arquivo compactado"),
    ("carpeta", "pasta"),
    ("código", "código"),
    ("do", "do"),
    ("en ejecución", "em execução"),
    (
        "en una PC Intel se lee de la CPU.",
        "num PC Intel é lido da CPU.",
    ),
    ("enviados", "enviados"),
    ("imagen", "imagem"),
    ("ju", "qi"),
    ("lu", "se"),
    ("ma", "te"),
    ("mi", "qa"),
    ("minimizada", "minimizada"),
    ("papelera", "lixeira"),
    ("recibidos", "recebidos"),
    ("sin conectar", "desconectado"),
    ("sin disco", "sem disco"),
    ("sin placa", "sem placa"),
    ("sin sensor", "sem sensor"),
    ("sá", "sá"),
    ("texto", "texto"),
    ("vi", "sx"),
    (
        "¿Qué querés que haga la computadora?",
        "O que você quer que o computador faça?",
    ),
    (
        "¿Seguro? Se borra para siempre: hacé clic otra vez",
        "Certeza? Será apagado para sempre: clique de novo",
    ),
];

static EN_F: &[(&str, &str)] = &[
    (
        "\"{}\" se borra para siempre.",
        "\"{}\" will be deleted forever.",
    ),
    ("Abrir {}", "Open {}"),
    ("Archivos · {}", "Files · {}"),
    (
        "Bajando {} imágenes y estilos...",
        "Downloading {} images and styles...",
    ),
    ("Buscar en la web: {}", "Search the web: {}"),
    ("Captura guardada en {}", "Screenshot saved to {}"),
    ("Cargando {}{}", "Loading {}{}"),
    ("Conectado · {}", "Connected · {}"),
    ("Conectando con Brave{}", "Connecting to Brave{}"),
    ("Conectando con {}...", "Connecting to {}..."),
    ("Configuración · {}", "Settings · {}"),
    (
        "Dirección del puente inválida: {}",
        "Invalid bridge address: {}",
    ),
    (
        "ENCENDIDO HACE {} · {} FPS · {} MS POR FRAME",
        "UP {} · {} FPS · {} MS PER FRAME",
    ),
    ("Escritorio {} de {}", "Desktop {} of {}"),
    ("Firewall: bloqueó {} ({})", "Firewall: blocked {} ({})"),
    (
        "Funciona: respuesta {} con {} bytes",
        "It works: response {} with {} bytes",
    ),
    ("Gráfico: {}", "Graph: {}"),
    ("Hace {} s. Esc cancela.", "{} s ago. Esc cancels."),
    (
        "LÍN {} · COL {} · CTRL+S GUARDAR",
        "LN {} · COL {} · CTRL+S SAVE",
    ),
    ("Música · {}", "Music · {}"),
    ("No anduvo: {}", "It failed: {}"),
    (
        "No se pudo abrir la página: {}",
        "Couldn't open the page: {}",
    ),
    (
        "No se pudo conectar con el puente de Brave en {} ({}). Se abre con cargo xtask run.",
        "Could not connect to the Brave bridge at {} ({}). It starts with cargo xtask run.",
    ),
    (
        "No se pudo guardar la captura: {}",
        "Couldn't save the screenshot: {}",
    ),
    (
        "No se pudo guardar la configuración: {}",
        "Couldn't save the settings: {}",
    ),
    (
        "Papelera vacía ({} elementos).",
        "Trash emptied ({} items).",
    ),
    ("Permitir: {}", "Allow: {}"),
    ("Regla {}", "Rule {}"),
    ("Regla: deny out app {}", "Rule: deny out app {}"),
    (
        "Se cortó la conexión con Brave ({})",
        "The connection to Brave dropped ({})",
    ),
    ("libre: {}", "free: {}"),
    ("recibido {} · enviado {}", "received {} · sent {}"),
    ("versión {}", "version {}"),
    ("{} MiB libres", "{} MiB free"),
    ("{} de {}", "{} {}"),
    ("{} de {} (heap)", "{} of {} (heap)"),
    (
        "{} de {} · flechas para ver más",
        "{} of {} · arrows to see more",
    ),
    (
        "{} elementos copiados: Ctrl+V los pega en otra carpeta.",
        "{} items copied: Ctrl+V pastes them in another folder.",
    ),
    (
        "{} elementos cortados: Ctrl+V los pega en otra carpeta.",
        "{} items cut: Ctrl+V pastes them in another folder.",
    ),
    (
        "{} elementos están en la Papelera.",
        "{} items are in the Trash.",
    ),
    ("{} elementos pegados.", "{} items pasted."),
    (
        "{} elementos se borran para siempre.",
        "{} items will be deleted forever.",
    ),
    ("{} elementos se borraron.", "{} items were deleted."),
    ("{} elementos · {}", "{} items · {}"),
    ("{} libres", "{} free"),
    ("{} min", "{} min"),
    ("{} renglones", "{} lines"),
    (
        "{} renglones más arriba · AvPág vuelve",
        "{} lines above · PgDn goes back",
    ),
    ("{} seleccionados de {} · {}", "{} selected of {} · {}"),
    ("{} · Navegador", "{} · Browser"),
    ("{}{} · Editor", "{}{} · Editor"),
    ("¿Mover \"{}\" a la Papelera?", "Move \"{}\" to the Trash?"),
    (
        "¿Mover {} elementos a la Papelera?",
        "Move {} items to the Trash?",
    ),
];

static PT_F: &[(&str, &str)] = &[
    (
        "\"{}\" se borra para siempre.",
        "\"{}\" será apagado para sempre.",
    ),
    ("Abrir {}", "Abrir {}"),
    ("Archivos · {}", "Arquivos · {}"),
    (
        "Bajando {} imágenes y estilos...",
        "Baixando {} imagens e estilos...",
    ),
    ("Buscar en la web: {}", "Buscar na web: {}"),
    ("Captura guardada en {}", "Captura salva em {}"),
    ("Cargando {}{}", "Carregando {}{}"),
    ("Conectado · {}", "Conectado · {}"),
    ("Conectando con Brave{}", "Conectando ao Brave{}"),
    ("Conectando con {}...", "Conectando com {}..."),
    ("Configuración · {}", "Configurações · {}"),
    (
        "Dirección del puente inválida: {}",
        "Endereço da ponte inválido: {}",
    ),
    (
        "ENCENDIDO HACE {} · {} FPS · {} MS POR FRAME",
        "LIGADO HÁ {} · {} FPS · {} MS POR QUADRO",
    ),
    ("Escritorio {} de {}", "Área de trabalho {} de {}"),
    ("Firewall: bloqueó {} ({})", "Firewall: bloqueou {} ({})"),
    (
        "Funciona: respuesta {} con {} bytes",
        "Funciona: resposta {} com {} bytes",
    ),
    ("Gráfico: {}", "Gráfico: {}"),
    ("Hace {} s. Esc cancela.", "Há {} s. Esc cancela."),
    (
        "LÍN {} · COL {} · CTRL+S GUARDAR",
        "LIN {} · COL {} · CTRL+S SALVAR",
    ),
    ("Música · {}", "Música · {}"),
    ("No anduvo: {}", "Não funcionou: {}"),
    (
        "No se pudo abrir la página: {}",
        "Não foi possível abrir a página: {}",
    ),
    (
        "No se pudo conectar con el puente de Brave en {} ({}). Se abre con cargo xtask run.",
        "Não foi possível conectar à ponte do Brave em {} ({}). Ela abre com cargo xtask run.",
    ),
    (
        "No se pudo guardar la captura: {}",
        "Não foi possível salvar a captura: {}",
    ),
    (
        "No se pudo guardar la configuración: {}",
        "Não foi possível salvar as configurações: {}",
    ),
    (
        "Papelera vacía ({} elementos).",
        "Lixeira esvaziada ({} itens).",
    ),
    ("Permitir: {}", "Permitir: {}"),
    ("Regla {}", "Regra {}"),
    ("Regla: deny out app {}", "Regra: deny out app {}"),
    (
        "Se cortó la conexión con Brave ({})",
        "A conexão com o Brave caiu ({})",
    ),
    ("libre: {}", "livre: {}"),
    ("recibido {} · enviado {}", "recebido {} · enviado {}"),
    ("versión {}", "versão {}"),
    ("{} MiB libres", "{} MiB livres"),
    ("{} de {}", "{} de {}"),
    ("{} de {} (heap)", "{} de {} (heap)"),
    (
        "{} de {} · flechas para ver más",
        "{} de {} · setas para ver mais",
    ),
    (
        "{} elementos copiados: Ctrl+V los pega en otra carpeta.",
        "{} itens copiados: Ctrl+V os cola em outra pasta.",
    ),
    (
        "{} elementos cortados: Ctrl+V los pega en otra carpeta.",
        "{} itens recortados: Ctrl+V os cola em outra pasta.",
    ),
    (
        "{} elementos están en la Papelera.",
        "{} itens estão na Lixeira.",
    ),
    ("{} elementos pegados.", "{} itens colados."),
    (
        "{} elementos se borran para siempre.",
        "{} itens serão apagados para sempre.",
    ),
    ("{} elementos se borraron.", "{} itens foram apagados."),
    ("{} elementos · {}", "{} itens · {}"),
    ("{} libres", "{} livres"),
    ("{} min", "{} min"),
    ("{} renglones", "{} linhas"),
    (
        "{} renglones más arriba · AvPág vuelve",
        "{} linhas acima · PgDn volta",
    ),
    ("{} seleccionados de {} · {}", "{} selecionados de {} · {}"),
    ("{} · Navegador", "{} · Navegador"),
    ("{}{} · Editor", "{}{} · Editor"),
    (
        "¿Mover \"{}\" a la Papelera?",
        "Mover \"{}\" para a Lixeira?",
    ),
    (
        "¿Mover {} elementos a la Papelera?",
        "Mover {} itens para a Lixeira?",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tablas_ordenadas_y_en_latin1() {
        for t in [EN, PT, EN_F, PT_F] {
            for w in t.windows(2) {
                assert!(w[0].0 < w[1].0, "desordenado: {} / {}", w[0].0, w[1].0);
            }
            for (k, v) in t.iter() {
                assert!(v.chars().all(|c| (c as u32) < 256), "fuera de Latin-1: {v}");
                assert_eq!(
                    k.matches("{}").count(),
                    v.matches("{}").count(),
                    "distinta cantidad de datos: {k}"
                );
            }
        }
    }

    #[test]
    fn traduce_y_completa() {
        set(Lang::En);
        assert_eq!(tr("Configuración"), "Settings");
        assert_eq!(tr("un texto sin traducción"), "un texto sin traducción");
        assert_eq!(trf("Escritorio {} de {}", &["2", "3"]), "Desktop 2 of 3");
        set(Lang::Pt);
        assert_eq!(tr("Papelera"), "Lixeira");
        set(Lang::Es);
        assert_eq!(tr("Papelera"), "Papelera");
        assert_eq!(trf("Escritorio {} de {}", &["2", "3"]), "Escritorio 2 de 3");
    }
}
