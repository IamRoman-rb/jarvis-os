//! Arma cada frame con **doble buffer** y **rectángulos sucios**.
//!
//! Hay tres buffers con la misma geometría:
//! - `fondo`: la capa estática, dibujada una sola vez.
//! - `frame`: donde se compone el frame nuevo (fuera de la pantalla).
//! - la pantalla real, a la que el kernel copia solo los rectángulos que cambiaron.
//!
//! En cada frame, para cada rectángulo sucio: se restaura desde `fondo`, se dibuja encima lo
//! dinámico que lo toca (con recorte a los rectángulos sucios), y después el kernel lo copia
//! entero a la pantalla. Así nunca se ve algo a
//! medio dibujar (sin parpadeo) y no se copia la pantalla completa cada vez.

use crate::Canvas;
use crate::assistant::Assistant;
use crate::canvas::Rect;
use crate::clock::DateTime;
use crate::hud;
use crate::sphere::ParticleCloud;

/// Hasta tres zonas cambian por frame: esfera, reloj y mensaje.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dirty {
    rects: [Option<Rect>; 3],
}

impl Dirty {
    pub fn push(&mut self, r: Rect) {
        if let Some(slot) = self.rects.iter_mut().find(|s| s.is_none()) {
            *slot = Some(r);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Rect> + '_ {
        self.rects.iter().flatten().copied()
    }

    pub fn touches(&self, r: &Rect) -> bool {
        self.iter().any(|d| d.intersects(r))
    }
}

type ClockKey = Option<(u16, u8, u8, u8, u8)>;

pub struct Scene {
    cloud: ParticleCloud,
    pub assistant: Assistant,
    width: usize,
    height: usize,
    last_clock: Option<ClockKey>,
    last_message: Option<(usize, bool)>,
}

impl Scene {
    pub fn new(width: usize, height: usize, particles: usize) -> Self {
        Scene {
            cloud: ParticleCloud::new(particles),
            assistant: Assistant::new(),
            width,
            height,
            last_clock: None,
            last_message: None,
        }
    }

    /// El próximo frame se redibuja completo (por ejemplo, al volver de otra app que tapó todo).
    pub fn invalidate(&mut self) {
        self.last_clock = None;
        self.last_message = None;
    }

    /// Dibuja la capa estática en el buffer de fondo (una sola vez).
    pub fn draw_background(&self, bg: &mut Canvas<'_>, info: &[(&str, &str)]) {
        hud::draw_static(bg, info);
    }

    /// Compone el frame del instante `now_ms` en `frame` y devuelve qué zonas hay que copiar a la
    /// pantalla. El primer frame devuelve la pantalla completa.
    pub fn render(
        &mut self,
        frame: &mut Canvas<'_>,
        bg: &Canvas<'_>,
        now_ms: u64,
        clock: Option<DateTime>,
    ) -> Dirty {
        let (w, h) = (self.width, self.height);
        let view = hud::sphere_view(w, h, now_ms);
        let pulse = self.assistant.pulse(now_ms);
        let sphere_rect = ParticleCloud::bounds(&view);
        let clock_rect = hud::clock_rect(w, h);
        let message_rect = hud::message_rect(w, h);

        let clock_key = clock.map(|t| (t.year, t.month, t.day, t.hour, t.minute));
        let speaking = self.assistant.is_speaking(now_ms);
        let message_key = (self.assistant.visible_chars(now_ms), speaking);

        let mut dirty = Dirty::default();
        let full = self.last_clock.is_none();
        if full {
            dirty.push(Rect::new(0, 0, w as i32, h as i32)); // primer frame: todo
        } else {
            dirty.push(sphere_rect);
            if self.last_clock != Some(clock_key) {
                dirty.push(clock_rect);
            }
            if self.last_message != Some(message_key) {
                dirty.push(message_rect);
            }
        }

        for r in dirty.iter() {
            frame.copy_from(bg, r);
        }
        // Solo se pinta dentro de lo que se acaba de restaurar: si el reloj toca la zona de la
        // esfera, se redibuja solo esa parte, y el resto del reloj (que no se restauró) queda igual.
        frame.set_clip(dirty.iter());
        // Orden de capas: esfera, y encima el texto.
        if dirty.touches(&sphere_rect) {
            hud::draw_voice_glow(frame, &view, &pulse);
            self.cloud.draw(frame, &view, pulse);
        }
        if dirty.touches(&clock_rect) {
            hud::draw_clock(frame, clock);
        }
        if dirty.touches(&message_rect) {
            hud::draw_message(frame, self.assistant.visible_text(now_ms), speaking);
        }
        if full {
            hud::draw_toolbar(frame, hud::TOOLBAR_JARVIS);
        }
        frame.clear_clip();

        self.last_clock = Some(clock_key);
        self.last_message = Some(message_key);
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use std::vec;
    use std::vec::Vec;

    const W: usize = 1024;
    const H: usize = 640;
    const INFO: &[(&str, &str)] = &[("NÚCLEO", "jarvis"), ("MEMORIA", "502 MiB")];

    fn hora(minute: u8) -> Option<DateTime> {
        Some(DateTime {
            year: 2026,
            month: 9,
            day: 23,
            hour: 15,
            minute,
            second: 0,
        })
    }

    fn buffer() -> Vec<u8> {
        vec![0u8; W * H * 4]
    }

    fn canvas(buf: &mut [u8]) -> Canvas<'_> {
        Canvas::new(buf, W, H, W, 4, PixelFormat::Bgr).unwrap()
    }

    /// Lo importante del doble buffer: dibujar solo lo sucio frame a frame tiene que dar
    /// exactamente la misma imagen que dibujar todo de cero en ese instante.
    #[test]
    fn frames_incrementales_igual_a_dibujar_de_cero() {
        let mut bg_buf = buffer();
        let mut bg = canvas(&mut bg_buf);

        let mut a = Scene::new(W, H, 3000);
        a.draw_background(&mut bg, INFO);
        let mut frame_a = buffer();
        {
            let mut fa = canvas(&mut frame_a);
            a.render(&mut fa, &bg, 0, hora(24));
            a.assistant
                .say("Todos los sistemas funcionan con normalidad.", 100);
            a.render(&mut fa, &bg, 900, hora(24));
            a.render(&mut fa, &bg, 1700, hora(25)); // cambió el minuto a mitad de la frase
            a.render(&mut fa, &bg, 2600, hora(25));
        }

        let mut b = Scene::new(W, H, 3000);
        b.assistant
            .say("Todos los sistemas funcionan con normalidad.", 100);
        let mut frame_b = buffer();
        b.render(&mut canvas(&mut frame_b), &bg, 2600, hora(25));

        assert!(
            frame_a == frame_b,
            "el render incremental dejó restos de frames anteriores"
        );
    }

    #[test]
    fn el_primer_frame_es_completo_y_despues_solo_lo_que_cambia() {
        let mut bg_buf = buffer();
        let mut bg = canvas(&mut bg_buf);
        let mut s = Scene::new(W, H, 500);
        s.draw_background(&mut bg, INFO);
        let mut f = buffer();
        let mut frame = canvas(&mut f);

        let primero: Vec<Rect> = s.render(&mut frame, &bg, 0, hora(10)).iter().collect();
        assert_eq!(primero, [Rect::new(0, 0, W as i32, H as i32)]);

        // Nada cambió salvo el giro: solo la esfera.
        assert_eq!(s.render(&mut frame, &bg, 16, hora(10)).iter().count(), 1);
        // Cambia el minuto: esfera + reloj.
        assert_eq!(s.render(&mut frame, &bg, 32, hora(11)).iter().count(), 2);
        // Empieza a hablar: esfera + mensaje.
        s.assistant.say("Hola", 40);
        assert_eq!(s.render(&mut frame, &bg, 200, hora(11)).iter().count(), 2);
    }
}
