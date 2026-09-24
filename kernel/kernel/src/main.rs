//! Kernel de JARVIS-OS — hito K0: arrancar y mostrar el HUD.
//!
//! No hay sistema operativo debajo: este código corre directamente sobre el hardware (o QEMU).
//! El crate `bootloader` se encarga de lo previo: pasar la CPU a modo 64 bits, armar las tablas
//! de páginas iniciales y pedirle al firmware UEFI un framebuffer. Después salta a `kernel_main`.

#![no_std]
#![no_main]

mod rtc;
mod serial;

use core::fmt::Write;
use core::panic::PanicInfo;

use bootloader_api::config::BootloaderConfig;
use bootloader_api::info::{MemoryRegionKind, PixelFormat as BootPixelFormat};
use bootloader_api::{BootInfo, entry_point};
use jarvis_gfx::clock::StrBuf;
use jarvis_gfx::hud::{self, Hud};
use jarvis_gfx::{Canvas, Color, PixelFormat, text};
use spin::Mutex;

/// Hora local de Argentina respecto de UTC (el reloj del hardware guarda UTC).
const UTC_OFFSET_HOURS: i8 = -3;
/// Partículas de la esfera. Sin SSE (llega en K1) el punto flotante es por software, así que
/// hay que medir antes de subir mucho este número.
const PARTICLES: u32 = 22_000;

// Sin pedir resolución mínima: el bootloader deja el modo de video que eligió el firmware
// (1280×800 en QEMU, la nativa del monitor en una PC real). Si se pide un mínimo, salta al
// modo MÁS GRANDE que lo cumpla (en QEMU, 2560×1600).
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 256 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// La pantalla, compartida con el manejador de panic para poder mostrar el error.
static SCREEN: Mutex<Option<Canvas<'static>>> = Mutex::new(None);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial_println!("JARVIS-OS {} arrancando", env!("CARGO_PKG_VERSION"));

    let memory_mib: u64 = boot_info
        .memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .map(|r| r.end - r.start)
        .sum::<u64>()
        / (1024 * 1024);
    serial_println!("memoria usable: {} MiB", memory_mib);

    let now = rtc::read_utc().map(|t| t.offset_hours(UTC_OFFSET_HOURS));
    match now {
        Some(t) => serial_println!(
            "reloj: {:04}-{:02}-{:02} {:02}:{:02} (UTC{})",
            t.year,
            t.month,
            t.day,
            t.hour,
            t.minute,
            UTC_OFFSET_HOURS
        ),
        None => serial_println!("reloj: lectura inválida del RTC"),
    }

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
    let Some(mut canvas) = Canvas::new(
        fb.buffer_mut(),
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        format,
    ) else {
        panic!("geometría de framebuffer inválida: {:?}", info);
    };

    let mut mem = StrBuf::<24>::new();
    let _ = write!(mem, "{} MiB", memory_mib);
    let mut res = StrBuf::<24>::new();
    let _ = write!(res, "{}×{}", info.width, info.height);
    let info_rows = [
        ("NÚCLEO", concat!("jarvis ", env!("CARGO_PKG_VERSION"))),
        ("MEMORIA", mem.as_str()),
        ("PANTALLA", res.as_str()),
        ("RELOJ", "RTC CMOS · UTC-3"),
        ("CEREBRO", "sin conectar"),
    ];
    hud::draw(
        &mut canvas,
        &Hud {
            now,
            message: "Sistema en línea. ¿En qué te ayudo?",
            info: &info_rows,
            particles: PARTICLES,
        },
    );
    *SCREEN.lock() = Some(canvas);

    // La CI busca esta línea para saber que el kernel arrancó y dibujó sin errores.
    serial_println!("JARVIS_BOOT_OK");
    halt();
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
