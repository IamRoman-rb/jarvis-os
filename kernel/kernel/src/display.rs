//! Las superficies donde se dibuja: el `frame` (donde se compone cada cuadro), el `bg` (la capa
//! quieta del fondo) y la pantalla (lo que ve el usuario).
//!
//! - **Sin placa de video** (una PC real hoy, o QEMU con la VGA común): la pantalla es el
//!   framebuffer del firmware, del tamaño que eligió el firmware. Todo como en K0–K5.
//! - **Con virtio-gpu**: la pantalla es una imagen nuestra en el heap, y la placa muestra en cada
//!   monitor la parte que el escritorio decidió (`jarvis_desktop::display`). Al cambiar de modo
//!   (extender, duplicar…) cambia el tamaño de todo: las tres superficies se vuelven a armar sobre
//!   la misma memoria, que se reservó una sola vez para el tamaño más grande posible.

use alloc::vec;
use jarvis_desktop::display::Layout;
use jarvis_gfx::{Canvas, PixelFormat, Rect};

use crate::virtio_gpu::VirtioGpu;
use crate::{SCREEN, serial_println};

/// Un buffer reservado para siempre (no se libera: vive lo que el kernel).
struct Buffer {
    ptr: *mut u8,
    len: usize,
}

impl Buffer {
    fn new(len: usize) -> Buffer {
        let b = vec![0u8; len].leak();
        Buffer {
            ptr: b.as_mut_ptr(),
            len,
        }
    }

    /// Un canvas sobre este buffer.
    ///
    /// # Safety
    /// No puede haber otro canvas vivo sobre el mismo buffer (el que había se descarta antes).
    unsafe fn canvas(
        &self,
        w: usize,
        h: usize,
        stride: usize,
        bpp: usize,
        format: PixelFormat,
    ) -> Option<Canvas<'static>> {
        // SAFETY: `ptr` apunta a `len` bytes del heap que nunca se liberan, y el llamador
        // garantiza que no hay otra referencia viva a ellos.
        let buf = unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) };
        Canvas::new(buf, w, h, stride, bpp, format)
    }
}

pub struct Surfaces {
    frame_buf: Buffer,
    bg_buf: Buffer,
    pub frame: Option<Canvas<'static>>,
    pub bg: Option<Canvas<'static>>,
    /// La placa y la imagen que muestra (su memoria).
    gpu: Option<(VirtioGpu, Buffer)>,
}

// SAFETY: los punteros son de memoria propia del heap; el kernel tiene un solo hilo.
unsafe impl Send for Surfaces {}

/// Geometría del framebuffer del firmware.
pub struct Firmware {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bpp: usize,
    pub format: PixelFormat,
}

impl Surfaces {
    /// `capacity`: píxeles del escritorio más grande que se puede llegar a pedir.
    pub fn new(fw: &Firmware, gpu: Option<VirtioGpu>, capacity: usize) -> Surfaces {
        let fw_len = fw.stride * fw.height * fw.bpp;
        let len = fw_len.max(capacity * 4);
        let mut s = Surfaces {
            frame_buf: Buffer::new(len),
            bg_buf: Buffer::new(len),
            frame: None,
            bg: None,
            gpu: gpu.map(|g| (g, Buffer::new(capacity * 4))),
        };
        // SAFETY: son buffers recién creados; todavía no hay canvas sobre ellos.
        unsafe {
            s.frame = s
                .frame_buf
                .canvas(fw.width, fw.height, fw.stride, fw.bpp, fw.format);
            s.bg = s
                .bg_buf
                .canvas(fw.width, fw.height, fw.stride, fw.bpp, fw.format);
        }
        s
    }

    pub fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Arma todo para un reparto de monitores nuevo. `false` si no se pudo (queda como estaba).
    pub fn apply(&mut self, l: &Layout) -> bool {
        let Some((gpu, image)) = self.gpu.as_mut() else {
            return false;
        };
        let (w, h) = (l.size.0 as usize, l.size.1 as usize);
        if w * h * 4 > image.len || w * h * 4 > self.frame_buf.len {
            serial_println!("pantallas: {}x{} no entra en la memoria reservada", w, h);
            return false;
        }
        // Primero se descartan los canvas viejos: después no queda ninguno sobre esa memoria.
        self.frame = None;
        self.bg = None;
        *SCREEN.lock() = None;
        // SAFETY: se acaban de descartar los canvas anteriores de los tres buffers.
        unsafe {
            self.frame = self.frame_buf.canvas(w, h, w, 4, PixelFormat::Bgr);
            self.bg = self.bg_buf.canvas(w, h, w, 4, PixelFormat::Bgr);
            *SCREEN.lock() = image.canvas(w, h, w, 4, PixelFormat::Bgr);
        }
        if !gpu.set_image(image.ptr, w as u32, h as u32) {
            serial_println!("pantallas: la placa no aceptó la imagen de {}x{}", w, h);
            return false;
        }
        for (i, r) in l.scanouts.iter().enumerate() {
            let rect = r.map(|r| (r.x as u32, r.y as u32, r.w as u32, r.h as u32));
            gpu.show(i as u32, rect);
        }
        serial_println!(
            "PANTALLAS_LISTAS {}x{} en {} salida(s)",
            w,
            h,
            l.scanouts.len()
        );
        true
    }

    /// Manda a la placa lo que cambió (sin placa, no hace falta: el firmware muestra la memoria).
    pub fn flush(&mut self, r: Rect) {
        if let Some((gpu, _)) = self.gpu.as_mut()
            && r.w > 0
            && r.h > 0
        {
            gpu.flush(r.x.max(0) as u32, r.y.max(0) as u32, r.w as u32, r.h as u32);
        }
    }
}
