//! La puerta entre los programas y el kernel (K11): `syscall`/`sysret` y el primer salto al
//! anillo 3.
//!
//! Un programa pide algo al kernel con la instrucción `syscall`: la CPU salta a la dirección del
//! MSR LSTAR ya en el anillo 0, guarda a dónde volver en `rcx` y los flags en `r11`, y apaga los
//! bits que dice SFMASK (las interrupciones, entre ellos). **No cambia de pila**: `rsp` sigue
//! siendo la del programa, que no es de fiar. Lo primero que hace la entrada es pasar a la pila
//! del kernel de esa tarea (`JARVIS_SYSCALL_STACK`, que task.rs actualiza en cada cambio).
//! Recién ahí se pueden volver a habilitar las interrupciones.
//!
//! `sysret` hace lo contrario. Tiene una trampa conocida (la de CVE-2012-0217): si `rcx` no es
//! una dirección canónica, la CPU da el fallo **en el anillo 0** con la pila del programa. Por
//! eso antes de volver se verifica la dirección.
//! Referencias: Intel SDM vol. 2B (SYSCALL, SYSRET) y vol. 3A §5.8.8; <https://wiki.osdev.org/SYSENTER>.

use core::arch::global_asm;

use x86_64::VirtAddr;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;

use crate::gdt;

/// Los registros del programa al entrar, en el orden en que los apila la entrada.
#[repr(C)]
pub struct Frame {
    pub rax: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
    /// A dónde vuelve (`rcx`).
    pub rip: u64,
    /// Sus flags (`r11`).
    pub rflags: u64,
    pub rsp: u64,
}

/// La pila del programa mientras se pasa a la del kernel (un solo núcleo, interrupciones
/// apagadas: nadie más la usa en ese momento).
#[unsafe(no_mangle)]
static JARVIS_USER_RSP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

global_asm!(
    ".global jarvis_syscall_entry",
    "jarvis_syscall_entry:",
    "mov [rip + JARVIS_USER_RSP], rsp",
    "mov rsp, [rip + JARVIS_SYSCALL_STACK]",
    "push qword ptr [rip + JARVIS_USER_RSP]",
    "push r11",
    "push rcx",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "push r9",
    "push r8",
    "push r10",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rax",
    // 16 palabras: rsp sigue alineado a 16, como pide la ABI antes de un `call`.
    "mov rdi, rsp",
    "call jarvis_syscall",
    "cli",
    "pop rax",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop r10",
    "pop r8",
    "pop r9",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "pop rcx",
    "pop r11",
    "pop rsp",
    "sysretq",
    // enter_user(rip [rdi], rsp [rsi], cs [rdx], ss [rcx]): el primer salto al programa.
    ".global jarvis_enter_user",
    "jarvis_enter_user:",
    "push rcx",
    "push rsi",
    "push 0x202", // IF encendido: el programa se puede desalojar
    "push rdx",
    "push rdi",
    // Nada del kernel tiene que quedar en los registros que ve el programa.
    "xor eax, eax",
    "xor ebx, ebx",
    "xor ecx, ecx",
    "xor edx, edx",
    "xor esi, esi",
    "xor edi, edi",
    "xor ebp, ebp",
    "xor r8d, r8d",
    "xor r9d, r9d",
    "xor r10d, r10d",
    "xor r11d, r11d",
    "xor r12d, r12d",
    "xor r13d, r13d",
    "xor r14d, r14d",
    "xor r15d, r15d",
    "iretq",
);

unsafe extern "C" {
    fn jarvis_syscall_entry();
    fn jarvis_enter_user(rip: u64, rsp: u64, cs: u64, ss: u64) -> !;
}

/// Habilita `syscall` y SSE para los programas. Una vez, al arrancar (después de `gdt::init`).
pub fn init() {
    let sel = gdt::selectors();
    // SAFETY: los selectores son los de la GDT cargada, en el orden que exige `sysret` (datos y
    // código de usuario, detrás de código y datos del kernel; `Star::write` lo verifica). La
    // entrada es la función de arriba. SFMASK apaga las interrupciones, la dirección y el paso a
    // paso al entrar. SSE: CR4.OSFXSR y OSXMMEXCPT dicen que el sistema guarda los XMM (task.rs);
    // CR0.EM apagado y MP encendido, como pide Intel para SSE.
    unsafe {
        Star::write(sel.user_code, sel.user_data, sel.code, sel.data)
            .expect("selectores en el orden de sysret");
        LStar::write(VirtAddr::new(jarvis_syscall_entry as *const () as u64));
        SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::TRAP_FLAG);
        Efer::update(|f| f.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
        Cr4::update(|f| {
            f.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        });
        Cr0::update(|f| {
            f.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
            f.insert(Cr0Flags::MONITOR_COPROCESSOR);
        });
    }
}

/// Salta al programa: `rip` en el anillo 3 con la pila `rsp`. No vuelve (el programa vuelve al
/// kernel solo por `syscall` o por una interrupción).
pub fn enter_user(rip: u64, rsp: u64) -> ! {
    let sel = gdt::selectors();
    // SAFETY: la tarea ya corre con el espacio del proceso (CR3), la TSS y la pila de `syscall`
    // apuntan a su pila del kernel (task.rs), y `rip`/`rsp` son direcciones del proceso (las puso
    // el cargador). Los selectores de usuario llevan RPL 3.
    unsafe {
        jarvis_enter_user(
            rip,
            rsp,
            u64::from(sel.user_code.0),
            u64::from(sel.user_data.0),
        )
    }
}

/// Lo llama la entrada con los registros del programa. `rax` vuelve con el resultado.
#[unsafe(no_mangle)]
extern "C" fn jarvis_syscall(frame: &mut Frame) {
    // Ya estamos en la pila del kernel: se puede desalojar a esta tarea.
    x86_64::instructions::interrupts::enable();
    let args = [
        frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
    ];
    frame.rax = crate::process::syscall(frame.rax, args) as u64;
    // `sysret` con una dirección no canónica fallaría en el anillo 0: el programa se termina.
    if frame.rip >= 0x0000_8000_0000_0000 {
        crate::process::kill_current(139, "direccion de retorno invalida");
    }
    x86_64::instructions::interrupts::disable();
}
