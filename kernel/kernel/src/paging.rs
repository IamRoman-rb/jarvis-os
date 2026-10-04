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
//!
//! **Espacio de usuario (K11, ADR 0010).** La entrada 0 de la PML4 (los primeros 512 GiB) queda
//! libre para los procesos: al armar las tablas se descartan los mapeos "identidad" que dejó ahí
//! el bootloader (su código para saltar al kernel, que ya no se usa). Todas las demás entradas se
//! crean al arrancar, aunque estén vacías, porque cada proceso copia las 511 de arriba en su PML4:
//! si el kernel agregara una entrada nueva después, los procesos no la verían.
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
    /// La entrada 0 quedó libre para los procesos.
    user_ok: bool,
}

/// Se toma siempre **sin interrupciones** (`with`): el fallo de página de un programa pide
/// marcos desde un manejador de interrupción, y si una tarea desalojada tuviera el lock, se
/// trabaría para siempre.
static PAGING: Mutex<Option<Paging>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Paging) -> R) -> Option<R> {
    x86_64::instructions::interrupts::without_interrupts(|| PAGING.lock().as_mut().map(f))
}

/// Fin del espacio de un proceso: la entrada 0 de la PML4.
pub const USER_END: u64 = jarvis_linux::mm::USER_END;

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
    /// Hay lugar para procesos (la entrada 0 quedó libre).
    pub user_space: bool,
}

/// Arma las tablas propias y las activa. `heap` es la región física (inicio, tamaño) que ya usa
/// el heap y `dma32`, el banco de DMA de 32 bits (inicio, fin): esos marcos no se entregan.
pub fn init(
    boot_info: &BootInfo,
    offset: u64,
    heap: (u64, u64),
    dma32: Option<(u64, u64)>,
) -> Option<Report> {
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
    if let Some((start, end)) = dma32 {
        frames.reserve(start, end);
    }

    let mut mem = OffsetMem(offset);
    let (old_frame, _) = Cr3::read();
    let old = PageTable {
        root: old_frame.start_address().as_u64(),
    };
    let segments = kernel_segments(boot_info, offset);
    let w_xor_x = !segments.is_empty();

    // Las hojas del bootloader: las de la RAM (en `offset`) se rehacen; el resto se copia.
    // Las de abajo de 512 GiB que son identidad (virtual = física) son del bootloader y ya no
    // se usan: se descartan para dejarle ese lugar a los procesos. Si hubiera otra cosa ahí, no
    // hay espacio de usuario.
    let mut leaves: Vec<Leaf> = Vec::new();
    let mut user_ok = offset >= USER_END;
    old.walk(&mem, &mut |l| {
        if l.virt >= offset && l.phys == l.virt - offset {
            return;
        }
        if l.virt < USER_END {
            if l.virt == l.phys {
                return;
            }
            user_ok = false;
        }
        leaves.push(l);
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

    // Todas las entradas de la mitad del kernel existen desde ya (ver arriba).
    for slot in 1..512u64 {
        if mem.read(table.root + slot * 8) & flags::PRESENT == 0 {
            let t = frames.alloc()?;
            for i in 0..512 {
                mem.write(t + i * 8, 0);
            }
            mem.write(table.root + slot * 8, t | flags::PRESENT | flags::WRITABLE);
        }
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
        user_space: user_ok,
    };
    x86_64::instructions::interrupts::without_interrupts(|| {
        *PAGING.lock() = Some(Paging {
            offset,
            table,
            frames,
            user_ok,
        })
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
    with(|p| map_mmio_in(p, phys, size))?
}

fn map_mmio_in(p: &mut Paging, phys: u64, size: usize) -> Option<*mut u8> {
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
    with(|p| (p.frames.free_frames(), p.table.tables(&OffsetMem(p.offset))))
}

/// Zona virtual de las pilas de las tareas (K9), lejos de todo lo demás (una entrada de la PML4
/// que el bootloader no usa; `map_stack` lo verifica página por página).
const STACKS_BASE: u64 = 0xFFFF_E000_0000_0000;
/// Cada tarea tiene una ventana de 2 MiB: su pila ocupa el final y lo de abajo queda **sin
/// mapear**. Una pila crece hacia abajo, así que si se desborda toca esa zona y la CPU da un
/// fallo de página en vez de pisar en silencio la memoria de otro (la "página de guarda").
const STACK_WINDOW: u64 = 2 * 1024 * 1024;

/// Mapea una pila de `bytes` (redondeado a páginas) para la tarea `slot`, con marcos del
/// allocator. Devuelve la dirección de su tope (donde empieza, porque crece hacia abajo).
pub fn map_stack(slot: usize, bytes: usize) -> Option<u64> {
    let bytes = (bytes as u64).div_ceil(4096) * 4096;
    if bytes == 0 || bytes > STACK_WINDOW - 4096 || slot >= jarvis_task::MAX_TASKS {
        return None;
    }
    with(|p| map_stack_in(p, slot, bytes))?
}

fn map_stack_in(p: &mut Paging, slot: usize, bytes: u64) -> Option<u64> {
    let mut mem = OffsetMem(p.offset);
    let top = STACKS_BASE + (slot as u64 + 1) * STACK_WINDOW;
    let mut virt = top - bytes;
    while virt < top {
        if p.table.leaf(&mem, virt).is_some() {
            // La ventana es solo de esta ranura: lo que haya es la pila de una tarea que ya
            // terminó (un proceso, K11). Se vuelve a usar.
            virt += 4096;
            continue;
        }
        let frame = p.frames.alloc()?;
        p.table
            .map(
                &mut mem,
                &mut p.frames,
                virt,
                frame,
                Size::Small,
                flags::WRITABLE | flags::NO_EXECUTE,
            )
            .ok()?;
        virt += 4096;
    }
    Some(top)
}

/// Si `addr` cae en la ventana de pila de una tarea (y dio un fallo), esa tarea desbordó su
/// pila: devuelve su número.
pub fn stack_overflow(addr: u64) -> Option<usize> {
    let end = STACKS_BASE + jarvis_task::MAX_TASKS as u64 * STACK_WINDOW;
    (STACKS_BASE..end)
        .contains(&addr)
        .then(|| ((addr - STACKS_BASE) / STACK_WINDOW) as usize)
}

// --- espacio de usuario (K11) ------------------------------------------------------------------

/// ¿Se pueden crear procesos? (La entrada 0 de la PML4 quedó libre al arrancar.)
pub fn user_space() -> bool {
    with(|p| p.user_ok).unwrap_or(false)
}

/// La PML4 del kernel (la de las tareas que no son procesos).
pub fn kernel_root() -> u64 {
    with(|p| p.table.root).unwrap_or(0)
}

/// Un espacio de direcciones nuevo: una PML4 con la mitad del kernel compartida (las entradas
/// 1–511, copiadas) y la entrada 0 vacía, para el proceso. Devuelve la física de la PML4.
pub fn new_space() -> Option<u64> {
    with(|p| {
        if !p.user_ok {
            return None;
        }
        let mut mem = OffsetMem(p.offset);
        let root = p.frames.alloc()?;
        mem.write(root, 0);
        for slot in 1..512u64 {
            let e = mem.read(p.table.root + slot * 8);
            mem.write(root + slot * 8, e);
        }
        Some(root)
    })?
}

/// Desarma el espacio de un proceso: sus páginas, sus tablas y su PML4. No tiene que ser el
/// activo (quien termina un proceso primero vuelve a la PML4 del kernel).
pub fn free_space(root: u64) {
    with(|p| {
        let mut mem = OffsetMem(p.offset);
        let table = PageTable { root };
        let mut leaves = Vec::new();
        table.free_slot(&mut mem, &mut p.frames, 0, &mut |l| leaves.push(l.phys));
        for f in leaves {
            p.frames.free(f);
        }
        p.frames.free(root);
    });
}

/// Los bits de una página del proceso según sus permisos (`prot::*`). Sin permisos (`PROT_NONE`)
/// la página queda presente pero sin el bit USER: el programa no la puede tocar y el contenido
/// sobrevive a un `mprotect` que la vuelva a habilitar.
fn user_flags(prot: u32) -> u64 {
    use jarvis_linux::abi::prot as p;
    let mut f = 0;
    if prot != p::NONE {
        f |= flags::USER;
    }
    if prot & p::WRITE != 0 {
        f |= flags::WRITABLE;
    }
    if prot & p::EXEC == 0 {
        f |= flags::NO_EXECUTE;
    }
    f
}

/// Pone una página en cero en `page` del espacio `root`. Si ya había una, queda la que estaba.
pub fn map_user(root: u64, page: u64, prot: u32) -> bool {
    if page >= USER_END || !page.is_multiple_of(4096) {
        return false;
    }
    with(|p| {
        let mut mem = OffsetMem(p.offset);
        let table = PageTable { root };
        if table.leaf(&mem, page).is_some() {
            return true;
        }
        let Some(frame) = p.frames.alloc() else {
            return false;
        };
        // SAFETY: el marco lo acaba de entregar el allocator (nadie más lo usa) y toda la RAM
        // está mapeada en `offset`.
        unsafe { core::ptr::write_bytes((p.offset + frame) as *mut u8, 0, 4096) };
        if table
            .map(
                &mut mem,
                &mut p.frames,
                page,
                frame,
                Size::Small,
                user_flags(prot),
            )
            .is_err()
        {
            p.frames.free(frame);
            return false;
        }
        true
    })
    .unwrap_or(false)
}

/// Libera las páginas del proceso en `[start, end)`.
pub fn unmap_user(root: u64, start: u64, end: u64) {
    with(|p| {
        let mut mem = OffsetMem(p.offset);
        let mut freed = Vec::new();
        PageTable { root }.unmap_range(&mut mem, start, end.min(USER_END), &mut |l| {
            freed.push(l.phys);
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(l.virt));
        });
        for f in freed {
            p.frames.free(f);
        }
    });
}

/// Cambia los permisos de las páginas que haya en `[start, end)`.
pub fn protect_user(root: u64, start: u64, end: u64, prot: u32) {
    with(|p| {
        let mut mem = OffsetMem(p.offset);
        let table = PageTable { root };
        let mut page = start & !0xFFF;
        while page < end.min(USER_END) {
            if table.set_flags(&mut mem, page, user_flags(prot)).is_some() {
                x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(page));
            }
            page += 4096;
        }
    });
}

/// Copia entre el kernel y la memoria de un proceso **recorriendo sus tablas** (no a través de
/// su mapeo): así una dirección inválida es un error y no un fallo de página adentro del kernel,
/// y un puntero del programa que apunte al kernel se rechaza (las páginas del kernel no tienen
/// el bit USER).
fn copy_user(
    root: u64,
    addr: u64,
    len: usize,
    write: bool,
    force: bool,
    f: &mut dyn FnMut(*mut u8, usize, usize),
) -> Result<(), jarvis_linux::sys::Fault> {
    use jarvis_linux::sys::Fault;
    if addr
        .checked_add(len as u64)
        .is_none_or(|end| end > USER_END)
    {
        return Err(Fault::Denied);
    }
    with(|p| {
        let mem = OffsetMem(p.offset);
        let table = PageTable { root };
        let mut done = 0;
        while done < len {
            let a = addr + done as u64;
            let page = a & !0xFFF;
            let Some(l) = table.leaf(&mem, page) else {
                return Err(Fault::Missing(page));
            };
            if !force && (l.flags & flags::USER == 0 || (write && l.flags & flags::WRITABLE == 0)) {
                return Err(Fault::Denied);
            }
            let n = (4096 - (a & 0xFFF) as usize).min(len - done);
            f((p.offset + l.phys + (a & 0xFFF)) as *mut u8, done, n);
            done += n;
        }
        Ok(())
    })
    .unwrap_or(Err(Fault::Denied))
}

pub fn read_user(root: u64, addr: u64, buf: &mut [u8]) -> Result<(), jarvis_linux::sys::Fault> {
    let dst = buf.as_mut_ptr();
    copy_user(root, addr, buf.len(), false, false, &mut |src, at, n| {
        // SAFETY: `src` es memoria del proceso (un marco mapeado, visto por el mapeo de la RAM)
        // con al menos `n` bytes dentro de su página; `dst + at` está dentro de `buf`.
        unsafe { core::ptr::copy_nonoverlapping(src, dst.add(at), n) }
    })
}

pub fn write_user(
    root: u64,
    addr: u64,
    data: &[u8],
    force: bool,
) -> Result<(), jarvis_linux::sys::Fault> {
    copy_user(root, addr, data.len(), true, force, &mut |dst, at, n| {
        // SAFETY: como en `read_user`, al revés.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr().add(at), dst, n) }
    })
}
