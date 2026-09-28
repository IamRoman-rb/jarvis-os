//! Paginación propia (hito K8): el kernel arma sus tablas de páginas y deja las del bootloader.
//!
//! Al arrancar, el bootloader deja la CPU con sus tablas: el kernel mapeado, la pila, el
//! framebuffer, la información de arranque y toda la RAM en `physical_memory_offset`. Acá:
//!
//! 1. Un allocator de marcos físicos (`jarvis_mem::frames`) con la RAM usable que informa el
//!    firmware, menos la del heap y el primer MiB (lo del BIOS y los dispositivos viejos).
//! 2. Una PML4 nueva, hecha con esos marcos. Se recorren las tablas del bootloader y se copia
//!    cada mapeo, salvo el de toda la RAM, que se rehace con páginas de 2 MiB (pocas tablas y
//!    menos entradas en la TLB).
//! 3. Las páginas del kernel se ajustan a su segmento del ELF: el código se lee y ejecuta pero
//!    no se escribe; los datos se escriben pero no se ejecutan (W^X). Un bug que escriba sobre el
//!    código, o que salte a datos, ahora es un page fault en vez de corrupción silenciosa.
//! 4. Se activan EFER.NXE (el bit "no ejecutar") y CR0.WP (el kernel también respeta "solo
//!    lectura") y se carga la PML4 nueva en CR3.
//!
//! Después, `map_mmio` agrega los registros de los dispositivos a estas tablas (sin caché).
//! La lógica está en el crate `jarvis-mem`, con tests en el host; acá solo se le presta la
//! memoria física y se tocan los registros de control.
//! Referencias: Intel SDM vol. 3A §4.5 (paginación de 4 niveles) y §4.6 (permisos),
//! <https://os.phil-opp.com/paging-implementation/>.

use alloc::vec::Vec;

use bootloader_api::BootInfo;
use bootloader_api::info::MemoryRegionKind;
use jarvis_mem::elf::{self, Segment};
use jarvis_mem::frames::FrameAllocator;
use jarvis_mem::table::{Leaf, PageTable, PhysMem, Size, flags};
use spin::Mutex;
use x86_64::PhysAddr;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr3, Cr3Flags, Efer, EferFlags};
use x86_64::structures::paging::PhysFrame;

/// Lo de la memoria que se sigue usando después del arranque.
struct Paging {
    offset: u64,
    table: PageTable,
    frames: FrameAllocator,
}

static PAGING: Mutex<Option<Paging>> = Mutex::new(None);

/// La RAM física, leída por el mapeo de toda la memoria (`offset + física`).
struct OffsetMem(u64);

impl PhysMem for OffsetMem {
    fn read(&self, phys: u64) -> u64 {
        // SAFETY: toda la RAM está mapeada en `offset` (en las tablas del bootloader y en las
        // nuestras), y solo se leen entradas de tablas de páginas, alineadas a 8 bytes.
        unsafe { core::ptr::read_volatile((self.0 + phys) as *const u64) }
    }
    fn write(&mut self, phys: u64, value: u64) {
        // SAFETY: ídem; solo se escriben tablas de páginas que armó este módulo (marcos que
        // entregó el allocator de marcos) o la tabla activa, siempre de a una entrada alineada.
        unsafe { core::ptr::write_volatile((self.0 + phys) as *mut u64, value) }
    }
}

/// Lo que se informa por el puerto serie al terminar.
pub struct Report {
    pub tables: usize,
    pub free_mib: u64,
    pub ram_mapped_mib: u64,
    pub kernel_segments: usize,
    pub w_xor_x: bool,
}

/// Arma las tablas propias y las activa. `heap` es la región física (inicio, tamaño) que ya usa
/// el heap: esos marcos no se entregan.
pub fn init(boot_info: &BootInfo, offset: u64, heap: (u64, u64)) -> Option<Report> {
    let regions = &boot_info.memory_regions;
    // El mapa de bits llega hasta la última RAM usable (el mapa del firmware también trae zonas
    // de dispositivos muy arriba, que no son marcos para entregar).
    let max = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .map(|r| r.end)
        .max()?;
    let mut frames = FrameAllocator::new(max);
    for r in regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
    {
        frames.add_free(r.start, r.end);
    }
    frames.reserve(0, 0x10_0000);
    frames.reserve(heap.0, heap.0 + heap.1);

    let mut mem = OffsetMem(offset);
    let (old_frame, _) = Cr3::read();
    let old = PageTable {
        root: old_frame.start_address().as_u64(),
    };
    let segments = kernel_segments(boot_info, offset);
    let w_xor_x = !segments.is_empty();

    // Las hojas del bootloader: las de la RAM (en `offset`) se rehacen; el resto se copia.
    let mut leaves: Vec<Leaf> = Vec::new();
    old.walk(&mem, &mut |l| {
        if !(l.virt >= offset && l.phys == l.virt - offset) {
            leaves.push(l);
        }
    });

    // El mapeo de la RAM: hasta la última región de RAM (usable o del bootloader) y todo lo de
    // abajo de 4 GiB (tablas del firmware, registros de dispositivos viejos). El bootloader
    // mapeaba además las zonas reservadas que declara el firmware, a veces cerca de 1 TiB: eso
    // son mil tablas (4 MiB) para memoria que nadie usa. Lo que haga falta de ahí arriba lo mapea
    // `map_mmio`, página por página y sin caché (que es lo correcto para un dispositivo).
    // Páginas de 1 GiB si la CPU las tiene; si no, de 2 MiB.
    let table = PageTable::new(&mut mem, &mut frames)?;
    let dense_end = regions
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                MemoryRegionKind::Usable | MemoryRegionKind::Bootloader
            ) || r.end <= 1 << 32
        })
        .map(|r| r.end)
        .max()?;
    let page = if gib_pages() { Size::Huge } else { Size::Large };
    let ram_end = dense_end.div_ceil(page.bytes()) * page.bytes();
    let ram_flags = flags::WRITABLE | flags::NO_EXECUTE | flags::GLOBAL;
    let mut phys = 0;
    while phys < ram_end {
        table
            .map(&mut mem, &mut frames, offset + phys, phys, page, ram_flags)
            .ok()?;
        phys += page.bytes();
    }
    for l in &leaves {
        let f = elf::tighten(l.flags, l.virt, l.size.bytes(), &segments);
        // Sin ACCESSED/DIRTY: los pone la CPU cuando usa la página.
        let f = f & !flags::ACCESSED & !flags::DIRTY & !flags::PRESENT;
        table
            .map(&mut mem, &mut frames, l.virt, l.phys, l.size, f)
            .ok()?;
    }

    // SAFETY: la PML4 nueva mapea todo lo que estaba mapeado (el código que corre, la pila, los
    // estáticos, la información de arranque, el framebuffer y la RAM en `offset`), así que al
    // cargarla la ejecución sigue igual. NXE y WP solo agregan controles que las tablas nuevas
    // respetan (el código no tiene NO_EXECUTE; lo que se escribe tiene WRITABLE). Sin
    // interrupciones de por medio: el cambio es atómico para este único procesador.
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        Efer::update(|f| f.insert(EferFlags::NO_EXECUTE_ENABLE));
        Cr0::update(|f| f.insert(Cr0Flags::WRITE_PROTECT));
        Cr3::write(
            PhysFrame::containing_address(PhysAddr::new(table.root)),
            Cr3Flags::empty(),
        );
    });

    // Se comprueba en las tablas ya activas: el código de esta función no se puede escribir y
    // los datos (este estático) no se pueden ejecutar.
    let code = table.leaf(&mem, init as *const () as u64);
    let data = table.leaf(&mem, &PAGING as *const _ as u64);
    let w_xor_x = w_xor_x
        && code.is_some_and(|l| l.flags & (flags::WRITABLE | flags::NO_EXECUTE) == 0)
        && data.is_some_and(|l| l.flags & flags::WRITABLE != 0 && l.flags & flags::NO_EXECUTE != 0);

    let report = Report {
        tables: table.tables(&mem),
        free_mib: frames.free_frames() as u64 * 4096 / (1024 * 1024),
        ram_mapped_mib: ram_end / (1024 * 1024),
        kernel_segments: segments.len(),
        w_xor_x,
    };
    *PAGING.lock() = Some(Paging {
        offset,
        table,
        frames,
    });
    Some(report)
}

/// ¿La CPU tiene páginas de 1 GiB? (`cpuid` 0x8000_0001, EDX bit 26, "Page1GB").
fn gib_pages() -> bool {
    use core::arch::x86_64::__cpuid;
    __cpuid(0x8000_0000).eax >= 0x8000_0001 && __cpuid(0x8000_0001).edx & (1 << 26) != 0
}

/// Los segmentos del ELF del kernel, si se pueden leer y cuadran con el código que corre (si no,
/// las páginas del kernel quedan con los permisos del bootloader).
fn kernel_segments(boot_info: &BootInfo, offset: u64) -> Vec<Segment> {
    if boot_info.kernel_addr == 0 || boot_info.kernel_len == 0 {
        return Vec::new();
    }
    // SAFETY: el bootloader dejó el archivo ELF del kernel en `kernel_addr` (memoria física que
    // no libera) y toda la RAM está mapeada en `offset`; solo se lee.
    let file = unsafe {
        core::slice::from_raw_parts(
            (offset + boot_info.kernel_addr) as *const u8,
            boot_info.kernel_len as usize,
        )
    };
    let here = init as *const () as u64;
    for load_offset in [boot_info.kernel_image_offset, 0] {
        if let Some(segs) = elf::load_segments(file, load_offset)
            && segs.iter().any(|s| s.executable && s.contains(here))
        {
            return segs;
        }
    }
    Vec::new()
}

/// Mapea `size` bytes de registros que empiezan en la dirección física `phys`. Devuelve el
/// puntero virtual (`offset + phys`, como el resto de la memoria física). Si ya estaban
/// mapeados (dentro del mapeo de la RAM), se usan así.
pub fn map_mmio(phys: u64, size: usize) -> Option<*mut u8> {
    let mut guard = PAGING.lock();
    let p = guard.as_mut()?;
    if size == 0 {
        return None;
    }
    let mut mem = OffsetMem(p.offset);
    let flags = flags::WRITABLE | flags::NO_CACHE | flags::WRITE_THROUGH | flags::NO_EXECUTE;
    let first = phys & !0xFFF;
    let last = (phys + size as u64 - 1) & !0xFFF;
    let mut frame = first;
    while frame <= last {
        let virt = p.offset + frame;
        if p.table.leaf(&mem, virt).is_none() {
            p.table
                .map(&mut mem, &mut p.frames, virt, frame, Size::Small, flags)
                .ok()?;
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
        }
        frame += 4096;
    }
    Some((p.offset + phys) as *mut u8)
}

/// (marcos libres, tablas en uso), para el estado del sistema.
pub fn usage() -> Option<(usize, usize)> {
    let guard = PAGING.try_lock()?;
    let p = guard.as_ref()?;
    Some((p.frames.free_frames(), p.table.tables(&OffsetMem(p.offset))))
}
