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
mod gdt;
mod interrupts;
mod keyboard;
mod mouse;
mod pci;
mod pit;
mod power;
mod queue;
mod rtc;
mod serial;
mod speaker;
mod time;
mod virtio_blk;
mod virtio_net;

use alloc::vec;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::info::{MemoryRegionKind, PixelFormat as BootPixelFormat};
use bootloader_api::{BootInfo, entry_point};
use jarvis_desktop::{Desktop, Event, MouseDecoder, NetInfo, Power, SystemStats};
use jarvis_fs::{BlockCache, FileSystem};
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

type Disk = BlockCache<VirtioBlk>;

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
    let cpu_name = cpu::brand();
    serial_println!("CPU: {}", cpu_name);

    // Disco: virtio-blk + caché de sectores + FAT32. Si no hay disco, el sistema arranca igual.
    let disk = VirtioBlk::init(phys_offset).and_then(|blk| {
        match FileSystem::mount(BlockCache::new(blk, DISK_CACHE_SECTORS)) {
            Ok(fs) => Some(fs),
            Err(e) => {
                serial_println!("disco: no se pudo montar: {}", e);
                None
            }
        }
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
    let canvas = |buf: &'static mut [u8]| {
        Canvas::new(
            buf,
            info.width,
            info.height,
            info.stride,
            info.bytes_per_pixel,
            format,
        )
        .expect("geometría de framebuffer inválida")
    };
    let screen = canvas(fb.buffer_mut());
    // Doble buffer en el heap: `bg` con la capa estática y `frame` donde se compone cada frame.
    // `leak` los vuelve `'static`: viven mientras corra el kernel.
    let len = info.stride * info.height * info.bytes_per_pixel;
    let mut bg = canvas(vec![0u8; len].leak());
    let mut frame = canvas(vec![0u8; len].leak());

    let mut desktop = Desktop::new(info.width, info.height, PARTICLES, disk);
    desktop.draw_background(&mut bg);
    serial_println!(
        "configuración: zona UTC{:+}, teclado {}",
        desktop.utc_offset(),
        if desktop.latam_keyboard() {
            "latinoamericano"
        } else {
            "EE. UU."
        }
    );
    *SCREEN.lock() = Some(screen);

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
    run(&mut desktop, &mut frame, &mut bg, &mut net, base, decoder)
}

/// Bucle principal: dormir hasta el próximo tick, atender la red, pasarle la entrada al
/// escritorio, dibujar y hacer lo que el escritorio pidió.
fn run(
    desktop: &mut Desktop<Disk>,
    frame: &mut Canvas<'static>,
    bg: &mut Canvas<'static>,
    net: &mut Option<Net<VirtioNet>>,
    mut stats: SystemStats,
    mut mouse_decoder: MouseDecoder,
) -> ! {
    let mut keyboard = keyboard::Keyboard::new();
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

        // La red se atiende en cada vuelta (cada ≤ 4 ms), no solo en cada frame.
        if let Some(n) = net.as_mut() {
            for (id, result) in n.poll(now) {
                desktop.net_response(id, result);
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
        let dirty = desktop.render(frame, bg, now, clock);
        if let Some(screen) = SCREEN.lock().as_mut() {
            desktop.present(screen, frame, &dirty);
        }
        render_ms += time::millis() - start;
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
