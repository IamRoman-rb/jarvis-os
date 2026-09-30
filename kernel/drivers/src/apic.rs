//! El formato de las interrupciones con APIC: entradas del IOAPIC y mensajes MSI.
//!
//! Con el PIC 8259, cada dispositivo tiene un **cable** (una línea IRQ) y hay 15 para toda la
//! máquina. Una PC moderna usa el **APIC**: un APIC local en cada CPU y uno o más IOAPIC que
//! reciben los cables viejos (teclado, timer y las líneas INTx de PCI) y los convierten en
//! mensajes al APIC local. PCI Express va más lejos con **MSI**: el dispositivo no tiene cable;
//! para interrumpir **escribe en memoria** (en la dirección 0xFEEx_xxxx, que el APIC local
//! atrapa) un dato que dice qué vector disparar. Cada dispositivo tiene su propio vector, así
//! que no hay que preguntarle a todos quién fue.
//!
//! Referencias: Intel SDM vol. 3A §11.11 (MSI), el datasheet del 82093AA (IOAPIC) y PCI Local
//! Bus 3.0 §6.8 (capacidades MSI y MSI-X).

/// Una entrada de la tabla de redirección del IOAPIC (64 bits).
///
/// Bits 0–7: vector. 8–10: entrega (000 = fija). 11: destino físico (0). 13: polaridad (1 =
/// activa en bajo). 15: disparo (1 = por nivel). 16: enmascarada. 56–63: el APIC de destino.
pub fn redirection(vector: u8, active_low: bool, level: bool, masked: bool, dest: u8) -> u64 {
    vector as u64
        | (active_low as u64) << 13
        | (level as u64) << 15
        | (masked as u64) << 16
        | (dest as u64) << 56
}

/// La dirección del mensaje MSI: la ventana del APIC local con el ID de la CPU de destino.
pub fn msi_address(apic_id: u8) -> u64 {
    0xFEE0_0000 | (apic_id as u64) << 12
}

/// El dato del mensaje MSI: el vector, entrega fija y por flanco (bits 8–15 en cero).
pub fn msi_data(vector: u8) -> u32 {
    vector as u32
}

/// Dónde están los campos de la capacidad MSI (id 0x05) según su registro de control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiLayout {
    /// Desplazamiento de la parte alta de la dirección (solo si el dispositivo es de 64 bits).
    pub address_high: Option<u8>,
    pub data: u8,
    /// Registro de máscara por vector, si lo tiene.
    pub mask: Option<u8>,
}

impl MsiLayout {
    /// `control`: los 16 bits de arriba de la primera palabra de la capacidad.
    pub fn new(control: u16) -> MsiLayout {
        let wide = control & (1 << 7) != 0;
        let per_vector_mask = control & (1 << 8) != 0;
        let data = if wide { 0x0C } else { 0x08 };
        MsiLayout {
            address_high: wide.then_some(0x08),
            data,
            mask: per_vector_mask.then_some(data + 4),
        }
    }
}

/// Control de MSI con la función habilitada y un solo vector (bits 4–6 = 0).
pub fn msi_enable(control: u16) -> u16 {
    (control & !(0b111 << 4)) | 1
}

/// Dónde está la tabla de MSI-X: en qué BAR y a qué distancia, y cuántas entradas tiene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsixTable {
    pub bar: u8,
    pub offset: u32,
    pub entries: u16,
}

impl MsixTable {
    /// `control`: bits 16–31 de la primera palabra; `table`: la segunda palabra de la capacidad.
    pub fn new(control: u16, table: u32) -> MsixTable {
        MsixTable {
            bar: (table & 0b111) as u8,
            offset: table & !0b111,
            entries: (control & 0x7FF) + 1,
        }
    }
}

/// Control de MSI-X: habilitado (bit 15) y sin la máscara de toda la función (bit 14).
pub fn msix_enable(control: u16) -> u16 {
    (control | 1 << 15) & !(1 << 14)
}

/// Una entrada de la tabla de MSI-X (16 bytes): dirección baja, alta, dato y control (bit 0 =
/// enmascarada).
pub fn msix_entry(apic_id: u8, vector: u8) -> [u32; 4] {
    let a = msi_address(apic_id);
    [a as u32, (a >> 32) as u32, msi_data(vector), 0]
}
