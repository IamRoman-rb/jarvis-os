//! Apagar y reiniciar.
//!
//! - **Apagar**: lo correcto es ACPI (leer las tablas del firmware para saber qué escribir y
//!   dónde). Todavía no hay intérprete de ACPI, así que se usa la dirección que QEMU usa en la
//!   máquina q35 (bloque PM en 0x600, registro de control en 0x604: SLP_EN con tipo 5 = apagado).
//!   En una PC real eso no hace nada y el kernel se queda detenido con la pantalla de aviso.
//! - **Reiniciar**: el pulso de reset del controlador de teclado 8042 (comando 0xFE), que existe
//!   en todas las PC desde la AT. Si no funciona, se fuerza un "triple fault".
//!   Referencia: <https://wiki.osdev.org/Shutdown> y <https://wiki.osdev.org/Reboot>.

use x86_64::instructions::port::Port;

pub fn shutdown() {
    // SAFETY: en QEMU q35 el puerto 0x604 es PM1a_CNT; 0x2000 = SLP_EN | SLP_TYP(0).
    unsafe { Port::<u16>::new(0x604).write(0x2000) };
    // SAFETY: máquinas viejas de QEMU (i440fx) y Bochs usan 0xB004.
    unsafe { Port::<u16>::new(0xB004).write(0x2000) };
}

pub fn reboot() -> ! {
    x86_64::instructions::interrupts::disable();
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
