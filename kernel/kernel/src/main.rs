//! Kernel de JARVIS-OS — hito K1: HUD animado en tiempo real.
//!
//! No hay sistema operativo debajo: este código corre directamente sobre el hardware (o QEMU).
//! El crate `bootloader` se encarga de lo previo: pasar la CPU a modo 64 bits, armar las tablas
//! de páginas iniciales, mapear la memoria física y pedirle al firmware UEFI un framebuffer.
//! Después salta a `kernel_main`.
//!
//! Orden de arranque: serie → GDT → IDT/PIC → PIT → TSC → heap → pantalla → interrupciones → bucle.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod gdt;
mod interrupts;
mod keyboard;
mod pit;
mod rtc;
mod serial;
mod time;

use alloc::vec;
use core::fmt::Write;
use core::panic::PanicInfo;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::info::{MemoryRegionKind, PixelFormat as BootPixelFormat};
use bootloader_api::{BootInfo, entry_point};
use jarvis_gfx::clock::{DateTime, StrBuf};
use jarvis_gfx::scene::Scene;
use jarvis_gfx::{Canvas, Color, PixelFormat, text};
use pc_keyboard::DecodedKey;
use spin::Mutex;

/// Hora local de Argentina respecto de UTC (el reloj del hardware guarda UTC).
const UTC_OFFSET_HOURS: i8 = -3;
/// Partículas de la esfera.
const PARTICLES: usize = 22_000;
/// Tope de ~60 frames por segundo.
const FRAME_MS: u64 = 16;
/// Cada cuánto se loguea el rendimiento por el puerto serie.
const REPORT_MS: u64 = 5_000;

const GREETING: &str = "Sistema en línea. ¿En qué te ayudo?";
/// Frases de demo (Espacio o Enter). En K4 las respuestas van a venir de Claude.
const DEMO_PHRASES: [&str; 4] = [
    "Todos los sistemas funcionan con normalidad.",
    "Mido el tiempo con el contador de ciclos de la CPU, calibrado al arrancar.",
    "Todavía no tengo voz propia, pero ya sé cómo moverme cuando hable.",
    "Cuando me conectes con Claude, voy a poder responderte de verdad.",
];

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 256 * 1024;
    // Mapear toda la memoria física: el heap la usa (ver allocator.rs).
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // Sin pedir resolución mínima: el bootloader deja el modo de video que eligió el firmware
    // (1280×800 en QEMU, la nativa del monitor en una PC real). Si se pide un mínimo, salta al
    // modo MÁS GRANDE que lo cumpla (en QEMU, 2560×1600).
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// La pantalla, compartida con el manejador de panic para poder mostrar el error.
static SCREEN: Mutex<Option<Canvas<'static>>> = Mutex::new(None);

fn local_time() -> Option<DateTime> {
    rtc::read_utc().map(|t| t.offset_hours(UTC_OFFSET_HOURS))
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
    let memory_mib: u64 = boot_info
        .memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .map(|r| r.end - r.start)
        .sum::<u64>()
        / (1024 * 1024);
    serial_println!(
        "memoria usable: {} MiB, heap: {} MiB",
        memory_mib,
        heap / (1024 * 1024)
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

    let mut mem = StrBuf::<24>::new();
    let _ = write!(mem, "{} MiB", memory_mib);
    let mut res = StrBuf::<24>::new();
    let _ = write!(res, "{}×{}", info.width, info.height);
    let rows = [
        ("NÚCLEO", concat!("jarvis ", env!("CARGO_PKG_VERSION"))),
        ("MEMORIA", mem.as_str()),
        ("PANTALLA", res.as_str()),
        ("RELOJ", "RTC CMOS · UTC-3"),
        ("CEREBRO", "sin conectar"),
        ("DEMO", "ESPACIO = HABLAR"),
    ];
    let mut scene = Scene::new(info.width, info.height, PARTICLES);
    scene.draw_background(&mut bg, &rows);
    *SCREEN.lock() = Some(screen);

    x86_64::instructions::interrupts::enable();
    run(&mut scene, &mut frame, &bg)
}

/// Bucle principal: dormir hasta el próximo tick, atender el teclado y dibujar un frame.
fn run(scene: &mut Scene, frame: &mut Canvas<'static>, bg: &Canvas<'static>) -> ! {
    let mut keyboard = keyboard::Keyboard::new();
    let mut clock = local_time();
    let mut last_rtc = 0;
    let mut next_frame = 0;
    let mut phrase = 0;
    let mut first = true;
    let (mut frames, mut render_ms, mut last_report) = (0u64, 0u64, 0u64);

    scene.assistant.say(GREETING, time::millis());
    loop {
        x86_64::instructions::hlt(); // duerme hasta la próxima interrupción (≤ 4 ms)
        let now = time::millis();
        if now < next_frame {
            continue;
        }
        next_frame = now + FRAME_MS;

        while let Some(key) = keyboard.next_key() {
            if matches!(key, DecodedKey::Unicode(' ' | '\n')) {
                let text = DEMO_PHRASES[phrase % DEMO_PHRASES.len()];
                phrase += 1;
                scene.assistant.say(text, now);
                serial_println!("JARVIS_HABLA: {}", text);
            }
        }
        if scene.assistant.take_finished(now) {
            serial_println!("JARVIS_REPOSO");
        }
        if now - last_rtc >= 1000 {
            clock = local_time();
            last_rtc = now;
        }

        let start = time::millis();
        let dirty = scene.render(frame, bg, now, clock);
        if let Some(screen) = SCREEN.lock().as_mut() {
            for r in dirty.iter() {
                screen.copy_from(frame, r);
            }
        }
        render_ms += time::millis() - start;
        frames += 1;

        if first {
            // La CI busca esta línea para saber que el kernel arrancó y dibujó sin errores.
            serial_println!("JARVIS_BOOT_OK");
            first = false;
        }
        if now - last_report >= REPORT_MS {
            let secs = (now - last_report).max(1);
            serial_println!(
                "rendimiento: {} fps, {} ms por frame",
                frames * 1000 / secs,
                render_ms / frames.max(1)
            );
            (frames, render_ms, last_report) = (0, 0, now);
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
