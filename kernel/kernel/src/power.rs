//! Apagar y reiniciar.
//!
//! - **Apagar** (K13): lo correcto es ACPI. El AML dice qué valor (SLP_TYP) escribir para el
//!   estado S5 ("apagado por software") en el objeto `\_S5`, y la FADT dice dónde está el
//!   registro de control PM1 (en QEMU, el puerto 0x604; en una PC, el que sea). Antes se le avisa
//!   a la placa con `\_PTS(5)` ("preparate para dormir") y se pasa a modo ACPI. Si no hay ACPI,
//!   quedan las direcciones que usa QEMU (q35 y las máquinas viejas).
//! - **Reiniciar**: el registro de reset de la FADT (en las PC, casi siempre el puerto 0xCF9) y,
//!   si no está, el pulso de reset del controlador de teclado 8042 (comando 0xFE), que existe en
//!   todas las PC desde la AT. Si nada funciona, se fuerza un "triple fault".
//!   Referencia: ACPI 6.5 §7.4 (`\_Sx`) y §4.8.3.2 (PM1_CNT), <https://wiki.osdev.org/Shutdown> y
//!   <https://wiki.osdev.org/Reboot>.

use jarvis_drivers::acpi::Register;
use x86_64::instructions::port::Port;

use crate::{acpi, paging, serial_println};

/// PM1_CNT: bits 10–12 = SLP_TYP, bit 13 = SLP_EN (dormir ya).
const SLP_TYP_SHIFT: u16 = 10;
const SLP_EN: u16 = 1 << 13;

/// Lee un registro de 16 bits de la FADT.
pub fn read16(reg: Register) -> u16 {
    match reg {
        // SAFETY: un puerto de energía que declaró la FADT.
        Register::Io(port) => unsafe { Port::<u16>::new(port).read() },
        Register::Memory(addr) => match paging::map_mmio(addr, 2) {
            // SAFETY: registro de energía en memoria, mapeado sin caché.
            Some(p) => unsafe { core::ptr::read_volatile(p as *const u16) },
            None => 0,
        },
    }
}

/// Escribe un registro de 16 bits de la FADT.
pub fn write16(reg: Register, value: u16) {
    match reg {
        // SAFETY: ídem `read16`.
        Register::Io(port) => unsafe { Port::<u16>::new(port).write(value) },
        Register::Memory(addr) => {
            if let Some(p) = paging::map_mmio(addr, 2) {
                // SAFETY: ídem `read16`.
                unsafe { core::ptr::write_volatile(p as *mut u16, value) }
            }
        }
    }
}

/// Escribe SLP_TYP y SLP_EN en PM1a (y PM1b, si hay): la placa entra al estado pedido.
pub fn enter_sleep_state(fadt: &jarvis_drivers::acpi::Fadt, typ: (u8, u8)) {
    let set = |reg: Option<Register>, t: u8| {
        if let Some(reg) = reg {
            let v = read16(reg) & !(0b111 << SLP_TYP_SHIFT);
            write16(reg, v | (t as u16) << SLP_TYP_SHIFT | SLP_EN);
        }
    };
    // PM1b primero y PM1a después: en las placas con los dos bloques, la escritura en PM1a
    // es la que dispara.
    set(fadt.pm1b_control, typ.1);
    set(fadt.pm1a_control, typ.0);
}

pub fn shutdown() {
    if let Some(a) = acpi::get()
        && let Some(fadt) = &a.fadt
        && let Some(typ) = a.sleep_type(5)
    {
        serial_println!("ACPI: apagando (S5, SLP_TYP {}/{})", typ.0, typ.1);
        a.call(r"\_PTS", Some(5));
        a.enter_acpi_mode();
        x86_64::instructions::interrupts::disable();
        enter_sleep_state(fadt, typ);
    }
    // SAFETY: en QEMU q35 el puerto 0x604 es PM1a_CNT; 0x2000 = SLP_EN | SLP_TYP(0).
    unsafe { Port::<u16>::new(0x604).write(0x2000) };
    // SAFETY: máquinas viejas de QEMU (i440fx) y Bochs usan 0xB004.
    unsafe { Port::<u16>::new(0xB004).write(0x2000) };
}

pub fn reboot() -> ! {
    x86_64::instructions::interrupts::disable();
    if let Some(fadt) = acpi::get().and_then(|a| a.fadt.as_ref())
        && let Some(reg) = fadt.reset
    {
        match reg {
            // SAFETY: el registro de reset que declaró la FADT (0xCF9 en casi todas las PC).
            Register::Io(port) => unsafe { Port::<u8>::new(port).write(fadt.reset_value) },
            Register::Memory(addr) => {
                if let Some(p) = paging::map_mmio(addr, 1) {
                    // SAFETY: ídem, en memoria.
                    unsafe { core::ptr::write_volatile(p, fadt.reset_value) }
                }
            }
        }
    }
    // Esperar a que el 8042 acepte comandos y pedir el reset.
    for _ in 0..100_000 {
        // SAFETY: 0x64 es el registro de estado del 8042; leerlo no tiene efectos.
        let status: u8 = unsafe { Port::new(0x64).read() };
        if status & 0x02 == 0 {
            break;
        }
    }
    // SAFETY: 0xFE al puerto de comandos del 8042 activa la línea de reset de la CPU.
    unsafe { Port::<u8>::new(0x64).write(0xFE) };
    // Si no reinició: una IDT vacía hace que la próxima interrupción sea un triple fault.
    let empty = x86_64::structures::DescriptorTablePointer {
        limit: 0,
        base: x86_64::VirtAddr::new(0),
    };
    // SAFETY: es a propósito: sin IDT válida, la interrupción siguiente reinicia la CPU.
    unsafe {
        x86_64::instructions::tables::lidt(&empty);
        core::arch::asm!("int3");
    }
    loop {
        x86_64::instructions::hlt();
    }
}
