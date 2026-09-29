//! IDT (Interrupt Descriptor Table): qué función atiende cada interrupción.
//!
//! Hay dos tipos:
//! - **Excepciones** (0–31): las genera la CPU ante un error (división por cero, fallo de página…).
//!   Acá se loguean y se muestra la pantalla de error.
//! - **IRQ de hardware** (32+): las genera un dispositivo. El PIC 8259 las recibe y se las pasa a la
//!   CPU. Por defecto las mapea en 0–15, encima de las excepciones, así que se remapean a 32–47.
//!
//! Los manejadores de IRQ hacen lo mínimo (guardar un byte, leer un registro de estado), avisan
//! "fin de interrupción" al PIC y, si corresponde, despiertan a una tarea (K9): el trabajo real
//! lo hace esa tarea, con las interrupciones habilitadas. El timer, además, le da al planificador
//! la oportunidad de cambiar de tarea (desalojo).
//!
//! Disco y red avisan por su línea PCI (INTx), que eligió el firmware y puede ser compartida:
//! el manejador de una línea le pregunta a cada dispositivo anotado en ella si fue él.
//! Referencia: <https://wiki.osdev.org/Interrupts>, <https://wiki.osdev.org/8259_PIC> y
//! <https://wiki.osdev.org/PCI#Interrupt_Line>.

use core::sync::atomic::{AtomicU32, Ordering};

use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, keyboard, mouse, paging, serial_println, task};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

#[derive(Clone, Copy)]
#[repr(u8)]
enum Irq {
    Timer = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
    /// IRQ12: la línea 4 del PIC esclavo.
    Mouse = PIC_2_OFFSET + 4,
}

// SAFETY: 32 y 40 no se superponen con las excepciones de la CPU (0–31).
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint);
        idt.page_fault.set_handler_fn(page_fault);
        idt.general_protection_fault
            .set_handler_fn(general_protection);
        // SAFETY: el índice corresponde a un stack válido cargado en la TSS por `gdt::init`.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[Irq::Timer as u8].set_handler_fn(timer);
        idt[Irq::Keyboard as u8].set_handler_fn(keyboard_irq);
        idt[Irq::Mouse as u8].set_handler_fn(mouse_irq);
        for (line, handler) in PCI_HANDLERS {
            idt[PIC_1_OFFSET + line].set_handler_fn(handler);
        }
        idt
    });
    idt.load();

    let mut pics = PICS.lock();
    // SAFETY: se inicializa una sola vez, antes de habilitar interrupciones. Después se enmascara
    // todo menos el timer (IRQ0), el teclado (IRQ1), la cascada al PIC esclavo (IRQ2) y el mouse
    // (IRQ12). Las líneas del disco y la red se habilitan cuando se anotan (`enable_pci_irq`).
    unsafe {
        pics.initialize();
        pics.write_masks(0b1111_1000, 0b1110_1111);
    }
}

// --- excepciones ------------------------------------------------------------------------------

extern "x86-interrupt" fn breakpoint(frame: InterruptStackFrame) {
    serial_println!("EXCEPCIÓN: breakpoint\n{:#?}", frame);
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    let addr = x86_64::registers::control::Cr2::read();
    stack_overflow_check();
    panic!("fallo de página en {:?} ({:?})\n{:#?}", addr, code, frame);
}

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, code: u64) {
    panic!("fallo de protección general (código {code})\n{:#?}", frame);
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    // Un desborde de pila casi siempre termina acá: el fallo de página no puede apilar su marco
    // en una pila que ya no tiene lugar, y la CPU pasa al stack de emergencia (IST).
    stack_overflow_check();
    panic!("doble fallo\n{:#?}", frame);
}

/// Si el fallo fue en la página de guarda de una tarea, lo dice con nombre (paging.rs).
fn stack_overflow_check() {
    if let Ok(addr) = x86_64::registers::control::Cr2::read()
        && let Some(id) = paging::stack_overflow(addr.as_u64())
    {
        panic!(
            "la tarea \"{}\" desbordó su pila ({:?})",
            task::name(id),
            addr
        );
    }
}

// --- IRQ de hardware --------------------------------------------------------------------------

/// El timer despierta a la CPU (de la tarea ociosa) y le da el turno al planificador: el tiempo
/// se mide con el TSC, no contando estas interrupciones.
extern "x86-interrupt" fn timer(_frame: InterruptStackFrame) {
    end_of_interrupt(Irq::Timer);
    // Después del "fin de interrupción": si se cambia de tarea, este manejador termina recién
    // cuando la tarea interrumpida vuelva a correr, y el PIC no puede quedar esperándolo.
    task::on_timer();
}

extern "x86-interrupt" fn keyboard_irq(_frame: InterruptStackFrame) {
    // SAFETY: 0x60 es el puerto de datos del controlador PS/2. Hay que leerlo en cada IRQ1 o el
    // controlador no manda la próxima tecla.
    let scancode: u8 = unsafe { Port::new(0x60).read() };
    keyboard::push_scancode(scancode);
    end_of_interrupt(Irq::Keyboard);
}

extern "x86-interrupt" fn mouse_irq(_frame: InterruptStackFrame) {
    // SAFETY: en IRQ12 el byte del puerto 0x60 viene del mouse; hay que leerlo siempre.
    let byte: u8 = unsafe { Port::new(0x60).read() };
    mouse::push_byte(byte);
    end_of_interrupt(Irq::Mouse);
}

fn end_of_interrupt(irq: Irq) {
    eoi_vector(irq as u8);
}

fn eoi_vector(vector: u8) {
    // SAFETY: se llama al final del manejador del vector `vector`. El lock de PICS solo se toma
    // con las interrupciones deshabilitadas, así que acá nunca está tomado.
    unsafe { PICS.lock().notify_end_of_interrupt(vector) };
}

// --- dispositivos PCI (K9) --------------------------------------------------------------------

/// Un dispositivo virtio legacy que avisa por una línea: su registro ISR (leerlo dice si fue él
/// y baja la línea) y el evento que despierta.
#[derive(Clone, Copy)]
struct Source {
    line: u8,
    isr_port: u16,
    event: u32,
}

const MAX_SOURCES: usize = 4;
/// Se escribe solo con las interrupciones deshabilitadas (`enable_pci_irq`).
static SOURCES: Mutex<[Option<Source>; MAX_SOURCES]> = Mutex::new([None; MAX_SOURCES]);
/// Interrupciones seguidas que ningún dispositivo reclamó, por línea.
static UNCLAIMED: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
/// Después de tantas sin dueño, la línea se apaga: alguien la mantiene activa y no la
/// atendemos. Los drivers siguen andando igual (esperan con plazo y revisan su anillo).
const STORM: u32 = 100_000;

macro_rules! pci_handlers {
    ($($name:ident = $line:literal),*) => {
        $(extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) { pci_irq($line) })*
        /// Las líneas que el firmware puede darle a un dispositivo PCI (todas menos el timer, el
        /// teclado, la cascada, el reloj, el mouse y el coprocesador), con su manejador.
        const PCI_HANDLERS: [(u8, extern "x86-interrupt" fn(InterruptStackFrame)); 10] =
            [$(($line, $name)),*];
    };
}
pci_handlers!(
    irq3 = 3,
    irq4 = 4,
    irq5 = 5,
    irq6 = 6,
    irq7 = 7,
    irq9 = 9,
    irq10 = 10,
    irq11 = 11,
    irq14 = 14,
    irq15 = 15
);

fn pci_irq(line: u8) {
    let mut events = 0;
    let mut claimed = false;
    for s in SOURCES.lock().iter().flatten().filter(|s| s.line == line) {
        // SAFETY: `isr_port` es el registro ISR (BAR0 + 0x13) de un virtio legacy que anotó su
        // driver. Leerlo lo pone en cero y el dispositivo baja la línea: hay que hacerlo antes
        // del "fin de interrupción" o la interrupción (por nivel) vuelve enseguida.
        let isr: u8 = unsafe { Port::new(s.isr_port).read() };
        if isr != 0 {
            claimed = true;
            events |= s.event;
        }
    }
    if claimed {
        UNCLAIMED[line as usize].store(0, Ordering::Relaxed);
    } else if UNCLAIMED[line as usize].fetch_add(1, Ordering::Relaxed) + 1 == STORM {
        set_masked(line, true);
        serial_println!("IRQ {line}: nadie la reclama; se apaga (disco y red siguen por polling)");
    }
    eoi_vector(PIC_1_OFFSET + line);
    if events != 0 {
        task::signal(events);
    }
}

/// Hay que llamarla sin interrupciones (el lock de PICS lo toman los manejadores).
fn set_masked(line: u8, masked: bool) {
    let mut pics = PICS.lock();
    // SAFETY: leer y escribir las máscaras solo cambia qué líneas llegan a la CPU.
    unsafe {
        let [mut m1, mut m2] = pics.read_masks();
        let (mask, bit) = if line < 8 {
            (&mut m1, line)
        } else {
            (&mut m2, line - 8)
        };
        if masked {
            *mask |= 1 << bit;
        } else {
            *mask &= !(1 << bit);
        }
        pics.write_masks(m1, m2);
    }
}

/// Anota un dispositivo virtio legacy que avisa por `line` y habilita esa línea en el PIC.
/// Devuelve `false` si no se puede (una línea reservada, o ya hay demasiados).
pub fn enable_pci_irq(line: u8, isr_port: u16, event: u32) -> bool {
    if !PCI_HANDLERS.iter().any(|&(l, _)| l == line) {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sources = SOURCES.lock();
        let Some(free) = sources.iter_mut().find(|s| s.is_none()) else {
            return false;
        };
        *free = Some(Source {
            line,
            isr_port,
            event,
        });
        drop(sources);
        set_masked(line, false);
        true
    })
}
