//! Lo que el escritorio intercambia con el kernel, sin depender del hardware:
//!
//! - **Estado de la máquina** ([`SystemStats`]): el kernel lo mide (CPU, memoria, disco, red) y se
//!   lo pasa al escritorio una vez por segundo. El escritorio guarda la historia ([`History`])
//!   para los gráficos del panel de estado y del monitor.
//! - **Pedidos** ([`Outbox`]): lo que las apps le piden al kernel (una página web, un tono por el
//!   parlante, apagar) o al escritorio (abrir otra app, mostrar un aviso).

use alloc::string::String;
use alloc::vec::Vec;

/// Estado de la placa de red.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetInfo {
    /// Hay placa de red con driver.
    pub present: bool,
    pub mac: [u8; 6],
    /// Dirección que dio el servidor DHCP (None mientras la está pidiendo).
    pub ip: Option<[u8; 4]>,
    pub gateway: Option<[u8; 4]>,
    pub dns: Option<[u8; 4]>,
}

/// Una foto del estado de la máquina.
#[derive(Clone, Debug, Default)]
pub struct SystemStats {
    /// Porcentaje del tiempo que la CPU estuvo trabajando (el resto, dormida en `hlt`).
    pub cpu_pct: u8,
    pub cpu_name: String,
    pub heap_used: u64,
    pub heap_total: u64,
    /// RAM total que informó el firmware.
    pub ram_total: u64,
    pub fps: u32,
    pub frame_ms: u32,
    pub uptime_ms: u64,
    /// Bytes leídos y escritos en el disco desde el arranque.
    pub disk_read: u64,
    pub disk_written: u64,
    /// Bytes recibidos y enviados por la red desde el arranque.
    pub net_rx: u64,
    pub net_tx: u64,
    pub net: NetInfo,
    /// Temperatura de la CPU en °C, si hay un sensor que el kernel sepa leer (en una máquina
    /// virtual no hay: QEMU no emula el sensor térmico).
    pub temp_c: Option<u8>,
    /// Tamaño de cada monitor que tiene la placa de video (vacío = solo la pantalla del firmware).
    pub displays: Vec<(u32, u32)>,
}

/// Últimos `N` valores (una muestra por segundo), para los gráficos.
#[derive(Clone, Debug)]
pub struct Series<const N: usize> {
    values: [u32; N],
    len: usize,
    head: usize,
}

impl<const N: usize> Default for Series<N> {
    fn default() -> Self {
        Series {
            values: [0; N],
            len: 0,
            head: 0,
        }
    }
}

impl<const N: usize> Series<N> {
    pub fn push(&mut self, v: u32) {
        self.values[self.head] = v;
        self.head = (self.head + 1) % N;
        self.len = (self.len + 1).min(N);
    }

    /// De la más vieja a la más nueva.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        let start = (self.head + N - self.len) % N;
        (0..self.len).map(move |i| self.values[(start + i) % N])
    }

    pub fn last(&self) -> Option<u32> {
        (self.len > 0).then(|| self.values[(self.head + N - 1) % N])
    }

    pub fn max(&self) -> u32 {
        self.iter().max().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }
}

pub const HISTORY: usize = 60;

/// Historia del último minuto.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub cpu: Series<HISTORY>,
    /// Porcentaje del heap en uso.
    pub mem: Series<HISTORY>,
    /// Bytes por segundo.
    pub disk: Series<HISTORY>,
    pub rx: Series<HISTORY>,
    pub tx: Series<HISTORY>,
    /// °C (0 = sin dato).
    pub temp: Series<HISTORY>,
    last: Option<SystemStats>,
}

impl History {
    pub fn push(&mut self, s: &SystemStats) {
        self.cpu.push(s.cpu_pct as u32);
        let mem = (s.heap_used * 100).checked_div(s.heap_total).unwrap_or(0);
        self.mem.push(mem as u32);
        let (disk, rx, tx) = match &self.last {
            Some(prev) => {
                let secs = (s.uptime_ms.saturating_sub(prev.uptime_ms)).max(1);
                let rate =
                    |now: u64, before: u64| (now.saturating_sub(before) * 1000 / secs) as u32;
                (
                    rate(
                        s.disk_read + s.disk_written,
                        prev.disk_read + prev.disk_written,
                    ),
                    rate(s.net_rx, prev.net_rx),
                    rate(s.net_tx, prev.net_tx),
                )
            }
            None => (0, 0, 0),
        };
        self.disk.push(disk);
        self.rx.push(rx);
        self.tx.push(tx);
        self.temp.push(s.temp_c.unwrap_or(0) as u32);
        self.last = Some(s.clone());
    }
}

// --- pedidos ----------------------------------------------------------------------------------

/// Las apps que se pueden abrir.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    /// Consola de JARVIS: comandos escritos (el ícono del micrófono).
    Console,
    Monitor,
    Files,
    Music,
    Browser,
    Editor,
    Viewer,
    /// Terminal tipo Linux (shell `jsh`).
    Terminal,
    /// Configuración del sistema.
    Settings,
    /// Brave, en el anfitrión (ADR 0007).
    Brave,
}

/// Qué abrir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    App(AppKind),
    /// Archivos en una carpeta.
    Folder(String),
    /// Editor de texto con un archivo.
    Edit(String),
    /// Navegador en una dirección.
    Browse(String),
    /// Visor de imágenes.
    View(String),
    /// Terminal, opcionalmente ejecutando un comando.
    Terminal(Option<String>),
    /// Configuración en una sección (0 = Sistema).
    Settings(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Shutdown,
    Reboot,
}

/// Para qué es una descarga (cambia el límite de tamaño y cómo se pide).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FetchKind {
    /// Una página (hasta 8 MiB).
    #[default]
    Page,
    /// Una imagen: se pide por el puente, que la convierte a BMP (el único formato que el
    /// kernel sabe leer) y la achica si es enorme.
    Image,
    /// Un archivo para guardar (hasta 32 MiB).
    Download,
}

/// Un pedido HTTP GET para el kernel (que tiene la red).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRequest {
    pub id: u32,
    pub url: String,
    pub kind: FetchKind,
    /// Quién lo pide (para el firewall): `navegador`, `terminal`, `apt`…
    pub app: String,
}

/// La respuesta ya completa (el kernel sigue las redirecciones).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    /// La dirección final (después de redirecciones).
    pub url: String,
    pub body: Vec<u8>,
}

/// Una conexión TCP que queda abierta (Brave, la sincronización): a diferencia de un GET, los
/// datos van y vienen mientras dure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamRequest {
    pub id: u32,
    /// Nombre o dirección IP.
    pub host: String,
    pub port: u16,
    /// Quién la abre (para el firewall).
    pub app: String,
}

/// Lo que el escritorio le pide a la red sobre una conexión.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamOp {
    Connect(StreamRequest),
    Send(u32, Vec<u8>),
    Close(u32),
}

/// Lo que pasa con una conexión (llega con su número).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Connected,
    Data(Vec<u8>),
    /// Se cerró: `None` si fue normal, o el motivo del error.
    Closed(Option<String>),
}

#[derive(Clone, Debug, Default)]
pub struct Outbox {
    /// Conexiones TCP largas: abrir, mandar, cerrar.
    pub streams: Vec<StreamOp>,
    pub launch: Vec<Launch>,
    /// (texto, es_error)
    pub notify: Vec<(String, bool)>,
    pub net: Vec<NetRequest>,
    /// Frecuencia del parlante en Hz (0 = silencio). `None` = no cambia.
    pub tone: Option<u32>,
    pub power: Option<Power>,
    /// Guardar una captura de pantalla al terminar el próximo frame.
    pub screenshot: bool,
    /// Pedido de JARVIS para decir algo (lo muestra el asistente del escritorio).
    pub say: Option<String>,
    /// Ventanas a cerrar ("Finalizar tarea" en el monitor).
    pub close: Vec<u32>,
    /// Ventana a traer adelante.
    pub activate: Option<u32>,
    /// Abrir el menú de apagado.
    pub power_menu: bool,
    /// Configuración nueva (la app Configuración): aplicarla y guardarla.
    pub config: Option<crate::config::Config>,
    /// Bloquear la pantalla.
    pub lock: bool,
    /// Mostrar el número de cada monitor (Configuración → Pantallas → Identificar).
    pub identify: bool,
    /// Cerrar la ventana de la app que lo pide (`exit` en la terminal).
    pub close_self: bool,
    /// Número del último pedido de red (los números no se repiten).
    pub(crate) next_net: u32,
    /// La app que está trabajando ahora (el escritorio lo pone antes de llamarla): los
    /// pedidos de red salen a su nombre.
    pub app: &'static str,
}

impl Outbox {
    /// Encola un GET y devuelve su número (la respuesta llega con el mismo número).
    pub fn fetch(&mut self, url: &str) -> u32 {
        self.fetch_kind(url, FetchKind::Page)
    }

    pub fn fetch_kind(&mut self, url: &str, kind: FetchKind) -> u32 {
        let app = if self.app.is_empty() {
            "sistema"
        } else {
            self.app
        };
        self.fetch_as(url, kind, app)
    }

    /// Un pedido a nombre de otra "app" (apt, snap y winget corren en la terminal pero el
    /// firewall los distingue).
    pub fn fetch_as(&mut self, url: &str, kind: FetchKind, app: &str) -> u32 {
        self.next_net += 1;
        self.net.push(NetRequest {
            id: self.next_net,
            url: url.into(),
            kind,
            app: app.into(),
        });
        self.next_net
    }

    /// Abre una conexión TCP a nombre de la app actual. Devuelve su número: los eventos
    /// ([`StreamEvent`]) llegan con él. Comparte la numeración con [`fetch`](Self::fetch).
    pub fn connect(&mut self, host: &str, port: u16) -> u32 {
        let app = if self.app.is_empty() {
            "sistema"
        } else {
            self.app
        };
        self.connect_as(host, port, app)
    }

    pub fn connect_as(&mut self, host: &str, port: u16, app: &str) -> u32 {
        self.next_net += 1;
        self.streams.push(StreamOp::Connect(StreamRequest {
            id: self.next_net,
            host: host.into(),
            port,
            app: app.into(),
        }));
        self.next_net
    }

    pub fn send(&mut self, id: u32, data: Vec<u8>) {
        self.streams.push(StreamOp::Send(id, data));
    }

    pub fn close_stream(&mut self, id: u32) {
        self.streams.push(StreamOp::Close(id));
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notify.push((text.into(), error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_serie_guarda_los_ultimos_n() {
        let mut s = Series::<3>::default();
        assert!(s.is_empty());
        for v in 1..=5 {
            s.push(v);
        }
        assert_eq!(s.iter().collect::<Vec<_>>(), [3, 4, 5]);
        assert_eq!((s.last(), s.max(), s.len()), (Some(5), 5, 3));
    }

    #[test]
    fn la_historia_calcula_tasas_por_segundo() {
        let mut h = History::default();
        let mut s = SystemStats {
            heap_total: 200,
            heap_used: 50,
            ..Default::default()
        };
        h.push(&s);
        s.uptime_ms = 2000;
        s.net_rx = 4000;
        s.disk_written = 1000;
        h.push(&s);
        assert_eq!(h.rx.last(), Some(2000));
        assert_eq!(h.disk.last(), Some(500));
        assert_eq!(h.mem.last(), Some(25));
    }
}
