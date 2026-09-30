//! Kernel de JARVIS-OS — hito K5: motor web, firewall, snap/winget e idiomas.
//!
//! No hay sistema operativo debajo: este código corre directamente sobre el hardware (o QEMU).
//! El crate `bootloader` se encarga de lo previo: pasar la CPU a modo 64 bits, armar las tablas
//! de páginas iniciales, mapear la memoria física y pedirle al firmware UEFI un framebuffer.
//! Después salta a `kernel_main`.
//!
//! El kernel es "fino": maneja el hardware (interrupciones, reloj, teclado, mouse, disco, placa
//! de red, parlante, pantalla) y le pasa todo al escritorio (`jarvis-desktop`), que tiene la
//! lógica de la interfaz. La pila TCP/IP y las descargas están en `jarvis-net`.
//!
//! Orden de arranque: serie → GDT → IDT/PIC → PIT → TSC → heap → disco → red → mouse →
//! pantalla → interrupciones → bucle.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod cpu;
mod display;
mod fw_cfg;
mod gdt;
mod interrupts;
mod keyboard;
mod mouse;
mod paging;
mod pci;
mod pit;
mod power;
mod queue;
mod rtc;
mod serial;
mod speaker;
mod time;
mod virtio_blk;
mod virtio_gpu;
mod virtio_modern;
mod virtio_net;
mod virtio_sound;

use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::info::{MemoryRegionKind, PixelFormat as BootPixelFormat};
use bootloader_api::{BootInfo, entry_point};
use jarvis_desktop::{Desktop, Event, MouseDecoder, NetInfo, Power, SystemStats};
use jarvis_fs::{BlockCache, BlockDevice, FileSystem, IoError, MemDisk};
use jarvis_gfx::clock::{DateTime, StrBuf};
use jarvis_gfx::{Canvas, Color, PixelFormat, text};
use jarvis_net::Net;
use spin::Mutex;
use virtio_blk::VirtioBlk;
use virtio_net::VirtioNet;

/// Partículas de la esfera.
const PARTICLES: usize = 22_000;
/// Tope de ~60 frames por segundo.
const FRAME_MS: u64 = 16;
/// Cada cuánto se miden la CPU, la memoria, el disco y la red.
const STATS_MS: u64 = 1000;
/// Cada cuánto se loguea el rendimiento por el puerto serie.
const REPORT_MS: u64 = 10_000;
/// Sectores del disco que se guardan en RAM (512 KiB).
const DISK_CACHE_SECTORS: usize = 1024;

/// Disco en RAM del modo en vivo (arranque desde la ISO, sin disco virtio): 48 MiB.
const LIVE_DISK_BYTES: usize = 48 * 1024 * 1024;

/// El disco del sistema: el virtio (con caché) o, en modo en vivo, uno en RAM.
enum Disk {
    Virtio(BlockCache<VirtioBlk>),
    Ram(MemDisk),
}

impl BlockDevice for Disk {
    fn sector_count(&self) -> u64 {
        match self {
            Disk::Virtio(d) => d.sector_count(),
            Disk::Ram(d) => d.sector_count(),
        }
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        match self {
            Disk::Virtio(d) => d.read(lba, buf),
            Disk::Ram(d) => d.read(lba, buf),
        }
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        match self {
            Disk::Virtio(d) => d.write(lba, buf),
            Disk::Ram(d) => d.write(lba, buf),
        }
    }
}

/// Modo en vivo: un FAT32 nuevo en RAM con las carpetas de siempre. Lo que se guarde se pierde
/// al apagar (hasta que haya drivers de disco reales para instalar, K13).
fn live_disk() -> Option<FileSystem<Disk>> {
    let mut ram = MemDisk::new(alloc::vec![0u8; LIVE_DISK_BYTES]);
    jarvis_fs::format_fat32(&mut ram, "JARVIS VIVO").ok()?;
    let mut fs = FileSystem::mount(Disk::Ram(ram)).ok()?;
    let now = jarvis_fs::Timestamp::EPOCH;
    for dir in [
        "/Documentos",
        "/Descargas",
        "/Imágenes",
        "/Música",
        "/Papelera",
        "/Sincronizado",
        "/Sistema",
    ] {
        let _ = fs.mkdir(dir, now);
    }
    let _ = fs.write_file(
        "/Documentos/Bienvenida.txt",
        "JARVIS-OS en modo en vivo: lo que guardes se pierde al apagar.
"
        .as_bytes(),
        now,
    );
    Some(fs)
}

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 4 * 1024 * 1024;
    // Mapear toda la memoria física: el heap la usa (ver allocator.rs) y el disco hace DMA.
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // Sin pedir resolución mínima: el bootloader deja el modo de video que eligió el firmware
    // (1280×800 en QEMU, la nativa del monitor en una PC real). Si se pide un mínimo, salta al
    // modo MÁS GRANDE que lo cumpla (en QEMU, 2560×1600).
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// La pantalla, compartida con el manejador de panic para poder mostrar el error.
static SCREEN: Mutex<Option<Canvas<'static>>> = Mutex::new(None);

/// La hora local: el reloj del hardware guarda UTC y la zona sale de la Configuración.
fn local_time(utc_offset: i8) -> Option<DateTime> {
    rtc::read_utc().map(|t| t.offset_hours(utc_offset))
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial_println!("JARVIS-OS {} arrancando", env!("CARGO_PKG_VERSION"));
    gdt::init();
    interrupts::init();
    pit::init();
    let tsc_mhz = time::init();
    serial_println!("TSC calibrado: {} MHz", tsc_mhz);

    let phys_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("el bootloader no mapeó la memoria física");
    let heap = allocator::init(&boot_info.memory_regions, phys_offset)
        .expect("no hay RAM usable para el heap");
    let ram: u64 = boot_info
        .memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .map(|r| r.end - r.start)
        .sum();
    serial_println!(
        "memoria usable: {} MiB, heap: {} MiB",
        ram / (1024 * 1024),
        heap / (1024 * 1024)
    );
    paging::init(phys_offset);
    let cpu_name = cpu::brand();
    serial_println!("CPU: {}", cpu_name);
    let thermal = cpu::Thermal::detect();
    match thermal.as_ref().and_then(cpu::Thermal::read) {
        Some(t) => serial_println!("TEMP {} C", t),
        None => serial_println!("TEMP sin sensor"),
    }

    // Disco: virtio-blk + caché de sectores + FAT32. Si no hay disco, el sistema arranca igual.
    let disk = VirtioBlk::init(phys_offset).and_then(|blk| {
        match FileSystem::mount(Disk::Virtio(BlockCache::new(blk, DISK_CACHE_SECTORS))) {
            Ok(fs) => Some(fs),
            Err(e) => {
                serial_println!("disco: no se pudo montar: {}", e);
                None
            }
        }
    });
    let disk = disk.or_else(|| {
        serial_println!("disco: no hay; modo en vivo (FAT32 en RAM)");
        let fs = live_disk();
        if fs.is_some() {
            serial_println!("MODO_EN_VIVO");
        }
        fs
    });
    if let Some(fs) = &disk {
        serial_println!(
            "disco: FAT32 \"{}\", {} MiB libres",
            fs.label(),
            fs.free_bytes() / (1024 * 1024)
        );
    }

    // Red: virtio-net + smoltcp. La dirección se pide por DHCP en el bucle.
    let mut net = VirtioNet::init(phys_offset).map(|dev| {
        let mac = dev.mac();
        Net::new(dev, mac, time::rdtsc(), time::millis())
    });
    if net.is_none() {
        serial_println!("red: no hay placa de red virtio");
    }

    // Placa de video con varias salidas (virtio-gpu). Sin ella, la pantalla del firmware.
    // Micrófono (virtio-sound). Sin placa, Configuración → Micrófono lo dice.
    let mic = virtio_sound::Mic::init(phys_offset);
    if mic.is_none() {
        serial_println!("microfono: no hay placa virtio-sound");
    }

    let mut gpu = virtio_gpu::VirtioGpu::init(phys_offset);
    let mut outputs: Vec<(u32, u32)> = Vec::new();
    if let Some(g) = gpu.as_mut() {
        let found = g.outputs();
        serial_println!("PANTALLAS {}", found.len());
        for (i, o) in found.iter().enumerate() {
            serial_println!(
                "PANTALLA {} {}x{}{}",
                i + 1,
                o.width,
                o.height,
                if o.enabled { "" } else { " (apagada)" }
            );
        }
        outputs = found.iter().map(|o| (o.width, o.height)).collect();
        // La placa informa el tamaño de la ventana de QEMU al arrancar (640×480 con GTK): si el
        // anfitrión pasó la resolución de su monitor, manda esa.
        if let Some((w, h)) = fw_cfg::resolution() {
            serial_println!("PANTALLA resolución del anfitrión {w}x{h}");
            outputs.fill((w, h));
        }
    }

    let mouse = mouse::init();
    serial_println!(
        "mouse: {}",
        match mouse {
            Some(true) => "PS/2 con rueda",
            Some(false) => "PS/2",
            None => "no responde",
        }
    );

    let Some(fb) = boot_info.framebuffer.as_mut() else {
        serial_println!("sin framebuffer: el firmware no dio pantalla gráfica");
        halt();
    };
    let info = fb.info();
    let format = match info.pixel_format {
        BootPixelFormat::Rgb => PixelFormat::Rgb,
        BootPixelFormat::U8 => PixelFormat::Gray,
        _ => PixelFormat::Bgr, // lo más común en UEFI
    };
    serial_println!(
        "pantalla: {}x{} ({:?}, {} bytes/píxel)",
        info.width,
        info.height,
        info.pixel_format,
        info.bytes_per_pixel
    );
    let screen = Canvas::new(
        fb.buffer_mut(),
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        format,
    )
    .expect("geometría de framebuffer inválida");
    *SCREEN.lock() = Some(screen);
    // Doble buffer en el heap: `bg` con la capa estática y `frame` donde se compone cada frame.
    // Con placa de video, reservados para el escritorio más grande posible (dos monitores).
    let firmware = display::Firmware {
        width: info.width,
        height: info.height,
        stride: info.stride,
        bpp: info.bytes_per_pixel,
        format,
    };
    let capacity = jarvis_desktop::display::max_pixels(&outputs);
    let mut surfaces = display::Surfaces::new(&firmware, gpu, capacity);

    let mut desktop = Desktop::new(info.width, info.height, PARTICLES, disk);
    // El cerebro de JARVIS en el anfitrión (si `cargo xtask run` lo levantó).
    if let Some((port, token)) = fw_cfg::brain() {
        serial_println!("cerebro: en el anfitrión, puerto {port}");
        desktop.set_brain(port, &token);
    }
    if surfaces.has_gpu() {
        desktop.set_outputs(outputs);
        if let Some(l) = desktop.take_display()
            && !surfaces.apply(&l)
        {
            serial_println!("pantallas: sigo con la pantalla del firmware");
        }
    }
    if let Some(bg) = surfaces.bg.as_mut() {
        desktop.draw_background(bg);
    }
    serial_println!(
        "configuración: zona UTC{:+}, teclado {}",
        desktop.utc_offset(),
        if desktop.latam_keyboard() {
            "latinoamericano"
        } else {
            "EE. UU."
        }
    );

    let base = SystemStats {
        cpu_name,
        ram_total: ram,
        net: NetInfo::default(),
        ..Default::default()
    };
    let decoder = if mouse == Some(true) {
        MouseDecoder::with_wheel()
    } else {
        MouseDecoder::new()
    };
    x86_64::instructions::interrupts::enable();
    run(
        &mut desktop,
        &mut surfaces,
        &mut net,
        base,
        decoder,
        thermal,
        mic,
    )
}

/// Un frame que tarda más que esto se anota en el log.
const SLOW_FRAME_MS: u64 = 300;

/// Bucle principal: dormir hasta el próximo tick, atender la red, pasarle la entrada al
/// escritorio, dibujar y hacer lo que el escritorio pidió.
fn run(
    desktop: &mut Desktop<Disk>,
    surfaces: &mut display::Surfaces,
    net: &mut Option<Net<VirtioNet>>,
    mut stats: SystemStats,
    mut mouse_decoder: MouseDecoder,
    thermal: Option<cpu::Thermal>,
    mut mic: Option<virtio_sound::Mic>,
) -> ! {
    let mut keyboard = keyboard::Keyboard::new();
    let mut last_mic = 0u64;
    let mut clock = local_time(desktop.utc_offset());
    let mut last_rtc = 0;
    let mut next_frame = 0;
    let mut first = true;
    let (mut frames, mut render_ms) = (0u64, 0u64);
    let (mut last_stats, mut last_report) = (0u64, 0u64);
    let (mut idle_ticks, mut stats_tsc) = (0u64, time::rdtsc());
    let mut last_ip = None;

    desktop.start(time::millis());
    loop {
        let sleep = time::rdtsc();
        x86_64::instructions::hlt(); // duerme hasta la próxima interrupción (≤ 4 ms)
        idle_ticks += time::rdtsc() - sleep;
        let now = time::millis();

        // El micrófono también (sus buffers son de 20 ms); el nivel va al escritorio 10 veces
        // por segundo (el medidor de Configuración → Micrófono).
        if let Some(m) = mic.as_mut() {
            m.poll();
            if now - last_mic >= 100 {
                last_mic = now;
                desktop.set_mic(Some(m.info.clone()));
            }
        }

        // La red se atiende en cada vuelta (cada ≤ 4 ms), no solo en cada frame.
        if let Some(n) = net.as_mut() {
            for (id, result) in n.poll(now) {
                desktop.net_response(id, result);
            }
            for (id, event) in n.take_stream_events() {
                desktop.stream_event(id, event);
            }
            if n.info().ip != last_ip {
                last_ip = n.info().ip;
                match last_ip {
                    Some(ip) => serial_println!("RED_IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
                    None => serial_println!("RED_SIN_IP"),
                }
            }
        }
        if now < next_frame {
            continue;
        }
        next_frame = now + FRAME_MS;
        if now - last_rtc >= 1000 {
            clock = local_time(desktop.utc_offset());
            last_rtc = now;
        }

        keyboard.latam = desktop.latam_keyboard();
        while let Some(event) = keyboard.next_event() {
            desktop.handle(event, now, clock);
        }
        while let Some(byte) = mouse::pop_byte() {
            if let Some(packet) = mouse_decoder.push(byte) {
                desktop.handle(Event::Mouse(packet), now, clock);
            }
        }

        let start = time::millis();
        let written = virtio_blk::WRITTEN_BYTES.load(Ordering::Relaxed);
        let requests = virtio_blk::REQUESTS.load(Ordering::Relaxed);
        let write_requests = virtio_blk::WRITE_REQUESTS.load(Ordering::Relaxed);
        let waited = virtio_blk::WAIT_TSC.load(Ordering::Relaxed);
        let (Some(frame), Some(bg)) = (surfaces.frame.as_mut(), surfaces.bg.as_mut()) else {
            continue;
        };
        let dirty = desktop.render(frame, bg, now, clock);
        let drawn = time::millis();
        let touched = SCREEN
            .lock()
            .as_mut()
            .and_then(|screen| desktop.present(screen, frame, &dirty));
        if let Some(r) = touched {
            surfaces.flush(r);
        }
        let took = time::millis() - start;
        render_ms += took;
        if took > SLOW_FRAME_MS {
            // Para encontrar lo que traba la interfaz (por ejemplo, guardar una captura).
            serial_println!(
                "FRAME_LENTO {} ms (dibujar {} ms, pantalla {} ms; disco: +{} KiB, {} pedidos ({} de escritura), {} ms esperando)",
                took,
                drawn - start,
                time::millis() - drawn,
                (virtio_blk::WRITTEN_BYTES.load(Ordering::Relaxed) - written) / 1024,
                virtio_blk::REQUESTS.load(Ordering::Relaxed) - requests,
                virtio_blk::WRITE_REQUESTS.load(Ordering::Relaxed) - write_requests,
                time::tsc_to_us(virtio_blk::WAIT_TSC.load(Ordering::Relaxed) - waited) / 1000
            );
        }
        frames += 1;

        let requests = desktop.take_requests();
        for req in requests.net {
            serial_println!("RED_PEDIDO {}", req.url);
            match net.as_mut() {
                Some(n) => {
                    if let Some((id, result)) = n.fetch(req, now) {
                        desktop.net_response(id, result);
                    }
                }
                None => desktop.net_response(req.id, Err("no hay placa de red".into())),
            }
        }
        for op in requests.streams {
            match net.as_mut() {
                Some(n) => n.stream(op, now),
                None => {
                    if let jarvis_desktop::StreamOp::Connect(r) = op {
                        desktop.stream_event(
                            r.id,
                            jarvis_desktop::StreamEvent::Closed(Some("no hay placa de red".into())),
                        );
                    }
                }
            }
        }
        // Otro reparto de los monitores (Win+P, Configuración → Pantallas).
        if let Some(l) = requests.display
            && surfaces.apply(&l)
        {
            // El escritorio ya dibujó este cuadro con las superficies viejas: todo de nuevo.
            if let Some(bg) = surfaces.bg.as_mut() {
                desktop.draw_background(bg);
            }
            desktop.invalidate();
        }
        if let Some(hz) = requests.tone {
            speaker::tone(hz);
        }
        for line in desktop.take_logs() {
            serial_println!("{}", line);
        }
        if let Some(p) = requests.power {
            power_off(p);
        }

        if first {
            // La CI busca esta línea para saber que el kernel arrancó y dibujó sin errores.
            serial_println!("JARVIS_BOOT_OK");
            first = false;
        }
        if now - last_stats >= STATS_MS {
            let tsc_now = time::rdtsc();
            let elapsed = (tsc_now - stats_tsc).max(1);
            let busy = elapsed.saturating_sub(idle_ticks);
            let secs = (now - last_stats).max(1);
            let (heap_used, heap_total) = allocator::usage();
            stats.cpu_pct = (busy * 100 / elapsed).min(100) as u8;
            stats.heap_used = heap_used;
            stats.heap_total = heap_total;
            stats.fps = (frames * 1000 / secs) as u32;
            stats.frame_ms = (render_ms / frames.max(1)) as u32;
            stats.uptime_ms = now;
            stats.disk_read = virtio_blk::READ_BYTES.load(Ordering::Relaxed);
            stats.disk_written = virtio_blk::WRITTEN_BYTES.load(Ordering::Relaxed);
            stats.net_rx = virtio_net::RX_BYTES.load(Ordering::Relaxed);
            stats.net_tx = virtio_net::TX_BYTES.load(Ordering::Relaxed);
            stats.net = net.as_ref().map(|n| n.info().clone()).unwrap_or_default();
            stats.temp_c = thermal.as_ref().and_then(cpu::Thermal::read);
            desktop.set_stats(stats.clone());
            if now - last_report >= REPORT_MS {
                serial_println!(
                    "rendimiento: {} fps, {} ms por frame, CPU {} %, heap {} MiB",
                    stats.fps,
                    stats.frame_ms,
                    stats.cpu_pct,
                    heap_used / (1024 * 1024)
                );
                last_report = now;
            }
            (frames, render_ms, idle_ticks, stats_tsc, last_stats) = (0, 0, 0, tsc_now, now);
        }
    }
}

/// Apagar o reiniciar. El disco no necesita nada especial: cada escritura ya se hizo en el
/// momento (la caché es write-through).
fn power_off(p: Power) -> ! {
    speaker::tone(0);
    match p {
        Power::Reboot => {
            serial_println!("JARVIS_REINICIO");
            power::reboot()
        }
        Power::Shutdown => {
            serial_println!("JARVIS_APAGADO");
            power::shutdown();
            // Si seguimos acá, el apagado por ACPI no funcionó (PC real): se avisa en pantalla.
            x86_64::instructions::interrupts::disable();
            if let Some(canvas) = SCREEN.lock().as_mut() {
                canvas.fill(Color::hex(0x050b14));
                let st = text::Style::new(text::Weight::Regular, text::Size::Size32, Color::WHITE);
                text::draw(canvas, 60, 60, "Ya podés apagar la computadora.", &st);
            }
            halt();
        }
    }
}

/// Detiene la CPU hasta la próxima interrupción, para siempre (sin quemar ciclos).
fn halt() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    x86_64::instructions::interrupts::disable();
    serial_println!("PANIC: {}", info);
    // Si la pantalla está libre, mostrar el error en rojo. Si estaba tomada (el panic ocurrió
    // dibujando), no se fuerza: el mensaje ya salió por el puerto serie.
    if let Some(mut guard) = SCREEN.try_lock()
        && let Some(canvas) = guard.as_mut()
    {
        canvas.fill(Color::hex(0x2a0008));
        let title = text::Style::new(text::Weight::Bold, text::Size::Size32, Color::hex(0xff2a55));
        text::draw(canvas, 40, 40, "JARVIS-OS: ERROR DEL NÚCLEO", &title);
        let mut msg = StrBuf::<256>::new();
        let _ = write!(msg, "{}", info.message());
        let body = text::Style::new(text::Weight::Regular, text::Size::Size16, Color::WHITE);
        text::draw(canvas, 40, 100, msg.as_str(), &body);
    }
    halt();
}
