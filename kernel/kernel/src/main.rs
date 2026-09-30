//! Kernel de JARVIS-OS — hito K9: multitarea.
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
//! Desde K9 hay varias **tareas** (task.rs): la del arranque sigue como la del escritorio, la
//! red tiene la suya (nettask.rs) y la ociosa duerme la CPU cuando nadie más puede correr. El
//! disco y la red avisan por interrupción en vez de esperarlos dando vueltas.
//!
//! Orden de arranque: serie → GDT → IDT/PIC → PIT → TSC → heap → tablas de páginas propias →
//! tareas → disco → red → mouse → pantalla → interrupciones → bucle del escritorio.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod acpi;
mod ahci;
mod allocator;
mod apic;
mod audio;
mod bootlog;
mod cpu;
mod display;
mod dma;
mod e1000;
mod entropy;
mod fw_cfg;
mod gdt;
mod hda;
mod hw;
mod installer;
mod interrupts;
mod irqlock;
mod keyboard;
mod mmio;
mod mouse;
mod nettask;
mod nic;
mod nvme;
mod paging;
mod pci;
mod pit;
mod power;
mod process;
mod queue;
mod rtc;
mod rtl8139;
mod rtl8169;
mod serial;
mod speaker;
mod storage;
mod syscall;
mod task;
mod time;
mod tls;
mod virtio_blk;
mod virtio_gpu;
mod virtio_modern;
mod virtio_net;
mod virtio_sound;
mod xhci;

use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::info::{MemoryRegionKind, PixelFormat as BootPixelFormat};
use bootloader_api::{BootInfo, entry_point};
use jarvis_desktop::{Desktop, Event, MouseDecoder, NetInfo, Power, SystemStats};
use jarvis_drivers::gpt::PartitionDevice;
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

/// El disco del sistema: el virtio (con caché), la partición de JARVIS de un disco real (SATA,
/// NVMe; K13) o, en modo en vivo, uno en RAM.
enum Disk {
    Virtio(BlockCache<VirtioBlk>),
    Hw(BlockCache<PartitionDevice<storage::AnyDisk>>),
    Ram(MemDisk),
}

impl BlockDevice for Disk {
    fn sector_count(&self) -> u64 {
        match self {
            Disk::Virtio(d) => d.sector_count(),
            Disk::Hw(d) => d.sector_count(),
            Disk::Ram(d) => d.sector_count(),
        }
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        match self {
            Disk::Virtio(d) => d.read(lba, buf),
            Disk::Hw(d) => d.read(lba, buf),
            Disk::Ram(d) => d.read(lba, buf),
        }
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        match self {
            Disk::Virtio(d) => d.write(lba, buf),
            Disk::Hw(d) => d.write(lba, buf),
            Disk::Ram(d) => d.write(lba, buf),
        }
    }
}

/// Modo en vivo: un FAT32 nuevo en RAM con las carpetas de siempre. Lo que se guarde se pierde
/// al apagar (para guardar, se instala en un disco: Configuración → Hardware, K13).
fn live_disk() -> Option<FileSystem<Disk>> {
    let mut ram = MemDisk::new(alloc::vec![0u8; LIVE_DISK_BYTES]);
    jarvis_fs::format_fat32(&mut ram, "JARVIS VIVO").ok()?;
    let mut fs = FileSystem::mount(Disk::Ram(ram)).ok()?;
    storage::populate(
        &mut fs,
        "JARVIS-OS en modo en vivo: lo que guardes se pierde al apagar.
",
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
    let heap_phys = heap;
    let heap = heap.1;
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
    // K8: el kernel deja las tablas de páginas del bootloader y arma las suyas.
    let pg = paging::init(boot_info, phys_offset, heap_phys)
        .expect("no se pudieron armar las tablas de páginas propias");
    dma::init(phys_offset);
    serial_println!(
        "PAGINACION_PROPIA {} tablas, RAM mapeada {} MiB, {} MiB de marcos libres, kernel {} segmentos{}",
        pg.tables,
        pg.ram_mapped_mib,
        pg.free_mib,
        pg.kernel_segments,
        if pg.w_xor_x {
            " con W^X"
        } else {
            " (sin W^X: no se pudo leer el ELF)"
        }
    );
    // La pantalla del firmware, lo antes posible (K13): desde acá el registro del arranque se ve
    // en pantalla (en una PC real no hay puerto serie) y un panic muestra sus últimas líneas.
    let rsdp = boot_info.rsdp_addr.into_option();
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
    bootlog::start_console(draw_boot_console);

    // K13: las tablas ACPI del firmware. Si describen el APIC, las interrupciones pasan del PIC
    // al APIC (hace falta la paginación propia: sus registros se mapean con `map_mmio`), y el
    // AML (la DSDT) se carga para saber cómo apagar y a dónde va cada línea PCI.
    match rsdp.and_then(acpi::init) {
        Some(a) => {
            if time::needs_recalibration()
                && let Some(mhz) = a.fadt.as_ref().and_then(time::calibrate_with_pm_timer)
            {
                serial_println!("TSC recalibrado con el timer PM de ACPI: {mhz} MHz");
            }
            if let Some(madt) = a.madt.clone()
                && !apic::init(madt)
            {
                serial_println!("APIC: la MADT no sirve; sigo con el PIC");
            }
            if let Some(rsdp) = rsdp {
                acpi::load_aml(rsdp);
            }
        }
        None => serial_println!("ACPI: no hay tablas; sigo con el PIC"),
    }
    // K11: `syscall`/`sysret` y SSE para los programas del anillo 3.
    syscall::init();
    if pg.user_space {
        serial_println!("ESPACIO_USUARIO listo: los primeros 512 GiB son de los procesos");
    } else {
        serial_println!("ESPACIO_USUARIO no disponible (la memoria baja está ocupada)");
    }
    // K10: entropía para las claves de TLS (RDSEED/RDRAND si hay, y la variación del TSC).
    let e = entropy::init();
    serial_println!(
        "{} {} bits (rdseed: {}, rdrand: {}, variación del TSC: {} bits)",
        if e.ready {
            "ENTROPIA_LISTA"
        } else {
            "ENTROPIA_INSUFICIENTE"
        },
        e.bits,
        if e.rdseed { "sí" } else { "no" },
        if e.rdrand { "sí" } else { "no" },
        e.jitter_bits
    );
    // K10: el cliente TLS (rustls con nuestro proveedor) y una prueba sin red: las claves
    // efímeras y el primer mensaje del handshake.
    let t = tls::init();
    match &t.hello {
        Ok((len, us)) => serial_println!(
            "TLS_LISTO {} raíces de confianza, hora {}, ClientHello de {} bytes en {} µs",
            t.roots,
            if t.clock_ok { "del RTC" } else { "desconocida" },
            len,
            us
        ),
        Err(err) => serial_println!("TLS_ERROR {}", err),
    }
    // K9: desde acá hay tareas (y el timer puede cambiar de una a otra, una vez habilitadas las
    // interrupciones). Hace falta la paginación propia: las pilas se mapean con página de guarda.
    task::init("escritorio");
    let cpu_name = cpu::brand();
    serial_println!("CPU: {}", cpu_name);
    hw::note("Procesador", cpu_name.clone());
    // K13: el sensor de Intel, el Tctl de los Ryzen o las zonas térmicas de ACPI.
    let mut thermal = cpu::Thermal::detect();
    if let Some(t) = &thermal {
        hw::note("Sensores", t.describe());
    }
    match thermal.as_mut().and_then(cpu::Thermal::read) {
        Some(t) => serial_println!("TEMP {} C", t),
        None => serial_println!("TEMP sin sensor"),
    }

    // Disco: virtio-blk + caché de sectores + FAT32. Si no hay disco, el sistema arranca igual.
    let disk = VirtioBlk::init(phys_offset).and_then(|mut blk| {
        // Que avise por interrupción (se usa recién con las tareas andando; el montaje de acá
        // abajo todavía espera dando vueltas).
        if let Some((dev, line, isr)) = blk.irq()
            && interrupts::enable_pci_irq(dev, line, isr, task::EV_DISK)
        {
            blk.use_irq();
            serial_println!("virtio-blk: avisa por la IRQ {line}");
        }
        match FileSystem::mount(Disk::Virtio(BlockCache::new(blk, DISK_CACHE_SECTORS))) {
            Ok(fs) => Some(fs),
            Err(e) => {
                serial_println!("disco: no se pudo montar: {}", e);
                None
            }
        }
    });
    // K13: los discos reales (SATA, NVMe). El del sistema es el que tiene la partición de JARVIS;
    // los demás quedan guardados para el instalador, sin montar.
    let mut found = storage::probe();
    let disk = disk.or_else(|| {
        let (name, part) = storage::take_system(&mut found)?;
        match FileSystem::mount(Disk::Hw(BlockCache::new(part, DISK_CACHE_SECTORS))) {
            Ok(fs) => {
                serial_println!("DISCO_SISTEMA {name}");
                Some(fs)
            }
            Err(e) => {
                serial_println!("disco {name}: la partición de JARVIS no se pudo montar: {e}");
                None
            }
        }
    });
    storage::park(found);
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

    // Red: virtio-net o una placa real (Intel, Realtek; K13) + smoltcp, en su propia tarea
    // (K9). La dirección se pide por DHCP.
    let nic = match VirtioNet::init(phys_offset) {
        Some(dev) => {
            if let Some((pci_dev, line, isr)) = dev.irq()
                && interrupts::enable_pci_irq(pci_dev, line, isr, task::EV_NET)
            {
                serial_println!("virtio-net: avisa por la IRQ {line}");
            }
            hw::note("Red", "virtio-net (QEMU)".into());
            Some(nic::Nic::Virtio(dev))
        }
        None => nic::probe().map(|(dev, name)| {
            serial_println!("RED_PLACA {name}");
            hw::note("Red", name.into());
            nic::Nic::Ring(dev)
        }),
    };
    let has_net = nic.is_some_and(|dev| {
        let mac = dev.mac();
        nettask::start(Net::new(dev, mac, time::rdtsc(), time::millis()))
    });
    if !has_net {
        serial_println!("red: no hay placa de red");
    }

    // Placa de video con varias salidas (virtio-gpu). Sin ella, la pantalla del firmware.
    // Micrófono (virtio-sound). Sin placa, Configuración → Micrófono lo dice.
    // Micrófono y parlantes (virtio-sound). Los parlantes los alimenta su propia tarea (K12).
    let (mic, speaker) = virtio_sound::init(phys_offset);
    if mic.is_none() {
        serial_println!("microfono: no hay placa virtio-sound");
    }
    // Sin virtio-sound, la placa de sonido de una PC (HDA, K13).
    let output: Option<alloc::boxed::Box<dyn audio::Output>> = match speaker {
        Some(sp) => Some(alloc::boxed::Box::new(sp)),
        None => {
            hda::probe().map(|h| alloc::boxed::Box::new(h) as alloc::boxed::Box<dyn audio::Output>)
        }
    };
    if output.is_some() {
        hw::note(
            "Sonido",
            if mic.is_some() {
                "virtio-sound (QEMU)".into()
            } else {
                "Intel HDA".into()
            },
        );
    }
    let sound_rate = match output {
        Some(out) => Some(audio::start(out)),
        None => {
            serial_println!("parlantes: no hay salida de audio");
            None
        }
    };

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

    let mut disk = disk;
    if let Some(fs) = disk.as_mut() {
        save_boot_log(fs);
    }
    let mut desktop = Desktop::new(info.width, info.height, PARTICLES, disk);
    if let Some(rate) = sound_rate {
        desktop.enable_sound(rate);
    }
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
        // K13: los discos que no son el del sistema, para Configuración → Hardware.
        disks: installer::disks(),
        ..Default::default()
    };
    let decoder = if mouse == Some(true) {
        MouseDecoder::with_wheel()
    } else {
        MouseDecoder::new()
    };
    let (tasks, _) = task::snapshot();
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    serial_println!("MULTITAREA {} tareas: {}", tasks.len(), names.join(", "));
    x86_64::instructions::interrupts::enable();
    run(&mut desktop, &mut surfaces, base, decoder, thermal, mic)
}

/// Lo que dejaron los programas (salida, pedidos de archivos, consola y red) pasa al escritorio,
/// y sus respuestas vuelven a los programas (K11).
fn serve_processes(desktop: &mut Desktop<Disk>) {
    for ev in process::take_events() {
        desktop.proc_event(ev);
    }
    for (pid, reply) in desktop.take_proc_replies() {
        process::reply(pid, reply);
    }
}

/// Un frame que tarda más que esto se anota en el log.
const SLOW_FRAME_MS: u64 = 300;

/// La tarea del escritorio (la del arranque): dormir hasta el próximo cuadro, pasarle al
/// escritorio la entrada y lo que contestó la red, dibujar y hacer lo que pidió. La red corre en
/// su propia tarea (nettask.rs); cuando no hay nada que hacer, la CPU queda en la tarea ociosa.
fn run(
    desktop: &mut Desktop<Disk>,
    surfaces: &mut display::Surfaces,
    mut stats: SystemStats,
    mut mouse_decoder: MouseDecoder,
    mut thermal: Option<cpu::Thermal>,
    mut mic: Option<virtio_sound::Mic>,
) -> ! {
    let mut keyboard = keyboard::Keyboard::new();
    // Los mouse USB (K13) llegan como paquetes PS/2 de 4 bytes, por su propia cola.
    let mut usb_mouse = MouseDecoder::with_wheel();
    let mut last_mic = 0u64;
    let mut clock = local_time(desktop.utc_offset());
    let mut last_rtc = 0;
    let mut next_frame = 0;
    let mut first = true;
    let (mut frames, mut render_ms) = (0u64, 0u64);
    let (mut last_stats, mut last_report) = (0u64, 0u64);
    let (mut stats_tsc, mut idle_before) = (time::rdtsc(), 0u64);
    let has_net = nettask::present();

    desktop.start(time::millis());
    loop {
        // Dormir hasta el próximo cuadro (~60 por segundo): mientras tanto corren la red, los
        // programas o la tarea ociosa (K9). Pero si un programa pide algo (un archivo, K11), se
        // lo atiende enseguida: él está esperando esa respuesta.
        loop {
            let woke = task::wait(task::EV_PROC, Some(next_frame));
            serve_processes(desktop);
            if woke & task::EV_PROC == 0 || time::millis() >= next_frame {
                break;
            }
        }
        let now = time::millis();
        next_frame = now + FRAME_MS;

        // Lo que contestó la red desde el cuadro anterior.
        for answer in nettask::take_answers() {
            match answer {
                nettask::Answer::Response(id, result) => desktop.net_response(id, result),
                nettask::Answer::Stream(id, event) => desktop.stream_event(id, event),
            }
        }
        serve_processes(desktop);

        // El micrófono (buffers de 20 ms, cuatro en la cola: alcanza con mirarlo por cuadro); el
        // nivel va al escritorio 10 veces por segundo (el medidor de Configuración → Micrófono).
        // K12: el audio que falta para los parlantes (la tarea "audio" lo lleva a la placa).
        desktop.set_audio_played(audio::played());
        let room = audio::room();
        if room > 0 {
            audio::fill(desktop.audio_render(room));
        }
        if let Some(m) = mic.as_mut() {
            m.poll();
            if now - last_mic >= 100 {
                last_mic = now;
                desktop.set_mic(Some(m.info.clone()));
            }
        }
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
        while let Some(byte) = mouse::pop_usb_byte() {
            if let Some(packet) = usb_mouse.push(byte) {
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
            if has_net {
                nettask::fetch(req);
            } else {
                desktop.net_response(req.id, Err("no hay placa de red".into()));
            }
        }
        for op in requests.streams {
            if has_net {
                nettask::stream(op);
            } else if let jarvis_desktop::StreamOp::Connect(r) = op {
                desktop.stream_event(
                    r.id,
                    jarvis_desktop::StreamEvent::Closed(Some("no hay placa de red".into())),
                );
            }
        }
        // K11: programas de Linux que pidió la Terminal, y los que hay que terminar.
        for req in requests.spawn {
            let pid = req.pid;
            if let Err(why) = process::spawn(req) {
                serial_println!("PROCESO_ERROR {} {}", pid, why);
                desktop.spawn_failed(pid, why);
            }
        }
        for pid in requests.kill {
            process::kill(pid);
        }
        serve_processes(desktop);
        nettask::set_https_bridge(desktop.config().https_bridge);
        // Un solo aviso por cuadro a la tarea de la red (si hubo pedidos).
        nettask::kick();
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
        // K13: instalar en un disco vacío (ya confirmado dos veces en Configuración). Tarda unos
        // segundos: la pantalla queda quieta mientras tanto.
        if let Some(target) = requests.install {
            let name = stats
                .disks
                .get(target)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            stats.install = jarvis_desktop::InstallState::Working(name);
            desktop.set_stats(stats.clone());
            let result = installer::install(target);
            if let Err(e) = &result {
                serial_println!("INSTALAR_ERROR {e}");
            }
            stats.install = jarvis_desktop::InstallState::Done(result);
            stats.disks = installer::disks();
            desktop.set_stats(stats.clone());
        }

        if first {
            // La CI busca esta línea para saber que el kernel arrancó y dibujó sin errores.
            bootlog::stop_console();
            serial_println!("JARVIS_BOOT_OK");
            first = false;
        }
        if now - last_stats >= STATS_MS {
            let tsc_now = time::rdtsc();
            let elapsed = (tsc_now - stats_tsc).max(1);
            let (tasks, switches) = task::snapshot();
            // La CPU estuvo ocupada todo el tiempo que no corrió la tarea ociosa.
            let idle_ms: u64 = tasks.iter().filter(|t| t.idle).map(|t| t.cpu_ms).sum();
            let idle = idle_ms.saturating_sub(idle_before);
            idle_before = idle_ms;
            let elapsed_ms = (time::tsc_to_us(elapsed) / 1000).max(1);
            let secs = (now - last_stats).max(1);
            let (heap_used, heap_total) = allocator::usage();
            stats.cpu_pct = (100 - (idle * 100 / elapsed_ms).min(100)) as u8;
            stats.heap_used = heap_used;
            stats.heap_total = heap_total;
            if let Some((free, tables)) = paging::usage() {
                stats.ram_free = free as u64 * 4096;
                stats.page_tables = tables as u32;
            }
            stats.fps = (frames * 1000 / secs) as u32;
            stats.frame_ms = (render_ms / frames.max(1)) as u32;
            stats.uptime_ms = now;
            stats.disk_read = virtio_blk::READ_BYTES.load(Ordering::Relaxed);
            stats.disk_written = virtio_blk::WRITTEN_BYTES.load(Ordering::Relaxed);
            stats.net_rx = virtio_net::RX_BYTES.load(Ordering::Relaxed);
            stats.net_tx = virtio_net::TX_BYTES.load(Ordering::Relaxed);
            stats.net = nettask::info();
            stats.temp_c = thermal.as_mut().and_then(cpu::Thermal::read);
            stats.kernel_tasks = tasks;
            stats.context_switches = switches;
            stats.devices = hw::list();
            desktop.set_stats(stats.clone());
            if now - last_report >= REPORT_MS {
                serial_println!(
                    "rendimiento: {} fps, {} ms por frame, CPU {} %, heap {} MiB, {} cambios de contexto, {} cortes de audio",
                    stats.fps,
                    stats.frame_ms,
                    stats.cpu_pct,
                    heap_used / (1024 * 1024),
                    switches,
                    audio::underruns()
                );
                last_report = now;
            }
            (frames, render_ms, stats_tsc, last_stats) = (0, 0, tsc_now, now);
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

/// La consola del arranque: las últimas líneas del registro, abajo a la izquierda, sobre el
/// framebuffer del firmware (se llama con cada línea, hasta el primer cuadro del escritorio).
fn draw_boot_console(tail: &str) {
    let Some(mut guard) = SCREEN.try_lock() else {
        return;
    };
    let Some(canvas) = guard.as_mut() else {
        return;
    };
    draw_log_lines(canvas, tail, Color::hex(0x050b14), Color::hex(0x7fdcff));
}

/// Dibuja líneas de registro de abajo hacia arriba en la mitad inferior de la pantalla.
fn draw_log_lines(canvas: &mut Canvas<'_>, tail: &str, bg: Color, fg: Color) {
    const LINE: i32 = 18;
    let (w, h) = (canvas.width() as i32, canvas.height() as i32);
    let lines: Vec<&str> = tail.lines().collect();
    let area = (lines.len() as i32 * LINE + 16).min(h / 2);
    canvas.fill_rect(0, h - area, w, area, bg);
    let st = text::Style::new(text::Weight::Regular, text::Size::Size16, fg);
    let mut y = h - 8 - LINE;
    for line in lines.iter().rev() {
        if y < h - area {
            break;
        }
        // Sin los códigos de escape de la consola del firmware.
        let clean: alloc::string::String = line.chars().filter(|c| !c.is_control()).collect();
        text::draw(canvas, 16, y, &clean, &st);
        y -= LINE;
    }
}

/// `/Sistema/arranque.log`: el registro del arranque hasta montar el disco (K13).
fn save_boot_log<D: BlockDevice>(fs: &mut FileSystem<D>) {
    let now = jarvis_desktop::apps::timestamp(rtc::read_utc());
    bootlog::with_all(|text, dropped| {
        let mut out = alloc::string::String::from(text);
        if dropped > 0 {
            out.push_str(&alloc::format!(
                "[{dropped} bytes más no entraron]
"
            ));
        }
        let _ = fs.mkdir("/Sistema", now);
        match fs.write_file("/Sistema/arranque.log", out.as_bytes(), now) {
            Ok(()) => serial_println!("REGISTRO_ARRANQUE {} bytes", out.len()),
            Err(e) => serial_println!("REGISTRO_ARRANQUE no se pudo guardar: {e}"),
        }
    });
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
        // Lo último que pasó antes del error (en una PC real es lo único que hay para mirar).
        bootlog::with_tail(20, |tail| {
            draw_log_lines(canvas, tail, Color::hex(0x2a0008), Color::hex(0xffb0c0));
        });
    }
    halt();
}
