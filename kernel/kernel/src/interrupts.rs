//! IDT (Interrupt Descriptor Table): qué función atiende cada interrupción.
//!
//! Hay dos tipos:
//! - **Excepciones** (0–31): las genera la CPU ante un error (división por cero, fallo de página…).
//!   Acá se loguean y se muestra la pantalla de error.
//! - **IRQ de hardware** (32+): las genera un dispositivo. El PIC 8259 las recibe y se las pasa a la
//!   CPU. Por defecto las mapea en 0–15, encima de las excepciones, así que se remapean a 32–47.
//!
//! Los manejadores de IRQ hacen lo mínimo (sumar un contador, guardar un byte) y avisan "fin de
//! interrupción" al PIC. El trabajo real se hace en el bucle principal, con las interrupciones
//! habilitadas. Referencia: <https://wiki.osdev.org/Interrupts> y <https://wiki.osdev.org/8259_PIC>.

use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, keyboard, serial_println};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

#[derive(Clone, Copy)]
#[repr(u8)]
enum Irq {
    Timer = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
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
        idt
    });
    idt.load();

    let mut pics = PICS.lock();
    // SAFETY: se inicializa una sola vez, antes de habilitar interrupciones. Después se enmascara
    // todo menos el timer (IRQ0) y el teclado (IRQ1): los demás dispositivos todavía no tienen driver.
    unsafe {
        pics.initialize();
        pics.write_masks(0b1111_1100, 0b1111_1111);
    }
}

// --- excepciones ------------------------------------------------------------------------------

extern "x86-interrupt" fn breakpoint(frame: InterruptStackFrame) {
    serial_println!("EXCEPCIÓN: breakpoint\n{:#?}", frame);
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    let addr = x86_64::registers::control::Cr2::read();
    panic!("fallo de página en {:?} ({:?})\n{:#?}", addr, code, frame);
}

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, code: u64) {
    panic!("fallo de protección general (código {code})\n{:#?}", frame);
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("doble fallo\n{:#?}", frame);
}

// --- IRQ de hardware --------------------------------------------------------------------------

/// El timer solo despierta a la CPU del `hlt` del bucle principal: el tiempo se mide con el TSC.
extern "x86-interrupt" fn timer(_frame: InterruptStackFrame) {
    end_of_interrupt(Irq::Timer);
}

extern "x86-interrupt" fn keyboard_irq(_frame: InterruptStackFrame) {
    // SAFETY: 0x60 es el puerto de datos del controlador PS/2. Hay que leerlo en cada IRQ1 o el
    // controlador no manda la próxima tecla.
    let scancode: u8 = unsafe { Port::new(0x60).read() };
    keyboard::push_scancode(scancode);
    end_of_interrupt(Irq::Keyboard);
}

fn end_of_interrupt(irq: Irq) {
    // SAFETY: se llama al final del manejador de esa misma IRQ.
    unsafe { PICS.lock().notify_end_of_interrupt(irq as u8) };
}
