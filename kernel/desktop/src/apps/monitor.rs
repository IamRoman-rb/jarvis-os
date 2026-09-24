//! Monitor del sistema (como el Administrador de tareas de Windows): CPU, memoria, disco y red
//! con gráficos del último minuto, y la lista de apps abiertas con "Finalizar tarea".

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::hud::icon;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::{Click, Ctx, SysView, icon_of};
use crate::files::format_size;
use crate::input::Key;
use crate::widgets::{
    WINDOW_BG, bar, big, button, card, draw_fit, duration, graph, ip, label, light, rate, s16,
};

const ROW_H: i32 = 34;

pub struct Monitor {
    pub dirty: bool,
    /// Fila elegida en la lista de apps (con ↑ ↓; Supr la finaliza).
    selected: usize,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

struct Layout {
    cards: [Rect; 4],
    list: Rect,
}

impl Layout {
    fn new(r: Rect) -> Layout {
        let pad = 14;
        let top = r.y + 46;
        let list_h = (r.h / 3).clamp(110, 200);
        let cards_h = r.h - 46 - list_h - pad * 2;
        let cw = (r.w - pad * 3) / 2;
        let ch = (cards_h - pad) / 2;
        let at =
            |i: i32, j: i32| Rect::new(r.x + pad + i * (cw + pad), top + j * (ch + pad), cw, ch);
        Layout {
            cards: [at(0, 0), at(1, 0), at(0, 1), at(1, 1)],
            list: Rect::new(r.x + pad, top + cards_h + pad, r.w - pad * 2, list_h),
        }
    }

    fn row(&self, i: usize) -> Rect {
        Rect::new(
            self.list.x + 8,
            self.list.y + 34 + i as i32 * ROW_H,
            self.list.w - 16,
            ROW_H - 2,
        )
    }

    fn end_button(&self, i: usize) -> Rect {
        let r = self.row(i);
        Rect::new(r.x + r.w - 128, r.y + 3, 124, r.h - 6)
    }

    fn visible_rows(&self) -> usize {
        ((self.list.h - 40) / ROW_H).max(0) as usize
    }
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            dirty: true,
            selected: 0,
        }
    }

    pub fn title(&self) -> String {
        "Monitor del sistema".into()
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, sys: &SysView<'_>) {
        c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
        let st = sys.stats;
        let h = sys.history;
        let l = Layout::new(r);

        text::draw(c, r.x + 16, r.y + 16, "RENDIMIENTO", &label(theme::CYAN));
        let summary = format!(
            "ENCENDIDO HACE {} · {} FPS · {} MS POR FRAME",
            duration(st.uptime_ms).to_uppercase(),
            st.fps,
            st.frame_ms
        );
        text::draw_right(
            c,
            r.x + r.w - 16,
            r.y + 16,
            &summary,
            &label(theme::TEXT_DIM),
        );

        // CPU
        let inner = card(c, l.cards[0], "PROCESADOR");
        text::draw(
            c,
            inner.x,
            inner.y,
            &format!("{} %", st.cpu_pct),
            &big(theme::CYAN),
        );
        let name = if st.cpu_name.is_empty() {
            "x86_64"
        } else {
            st.cpu_name.as_str()
        };
        draw_fit(
            c,
            inner.x + 110,
            inner.y + 8,
            name,
            &light(theme::TEXT_DIM),
            inner.w - 110,
        );
        let g = Rect::new(inner.x, inner.y + 42, inner.w, inner.h - 42);
        graph(c, g, &h.cpu, 100, theme::CYAN);

        // Memoria
        let inner = card(c, l.cards[1], "MEMORIA (HEAP DEL NÚCLEO)");
        let pct = (st.heap_used * 100).checked_div(st.heap_total).unwrap_or(0) as u32;
        text::draw(
            c,
            inner.x,
            inner.y,
            &format!("{pct} %"),
            &big(theme::PARTICLE_BRIGHT),
        );
        let detail = format!(
            "{} de {} · RAM {}",
            format_size(st.heap_used),
            format_size(st.heap_total),
            format_size(st.ram_total)
        );
        draw_fit(
            c,
            inner.x + 110,
            inner.y + 8,
            &detail,
            &light(theme::TEXT_DIM),
            inner.w - 110,
        );
        let g = Rect::new(inner.x, inner.y + 42, inner.w, inner.h - 42);
        graph(c, g, &h.mem, 100, theme::PARTICLE_BRIGHT);

        // Disco
        let inner = card(c, l.cards[2], "DISCO");
        match sys.disk {
            Some((name, free, total)) => {
                let used = total.saturating_sub(free);
                let pct = (used * 100).checked_div(total).unwrap_or(0) as u32;
                text::draw(c, inner.x, inner.y, &format!("{pct} %"), &big(theme::AMBER));
                let detail = format!(
                    "{name} · {} libres de {} · E/S {}",
                    format_size(free),
                    format_size(total),
                    rate(h.disk.last().unwrap_or(0))
                );
                draw_fit(
                    c,
                    inner.x + 110,
                    inner.y + 8,
                    &detail,
                    &light(theme::TEXT_DIM),
                    inner.w - 110,
                );
                bar(
                    c,
                    Rect::new(inner.x, inner.y + 40, inner.w, 5),
                    pct,
                    theme::AMBER,
                );
            }
            None => {
                text::draw(c, inner.x, inner.y + 8, "Sin disco", &s16(theme::TEXT_DIM));
            }
        }
        let g = Rect::new(inner.x, inner.y + 52, inner.w, inner.h - 52);
        graph(c, g, &h.disk, 0, theme::AMBER);

        // Red
        let inner = card(c, l.cards[3], "RED");
        let net = &st.net;
        let (headline, color) = match (net.present, net.ip) {
            (false, _) => (String::from("Sin placa de red"), theme::TEXT_DIM),
            (true, None) => (String::from("Pidiendo dirección (DHCP)..."), theme::AMBER),
            (true, Some(a)) => (ip(a), theme::CYAN),
        };
        text::draw(c, inner.x, inner.y + 2, &headline, &s16(color));
        let traffic = format!(
            "RX {} · TX {}",
            rate(h.rx.last().unwrap_or(0)),
            rate(h.tx.last().unwrap_or(0))
        );
        text::draw_right(
            c,
            inner.x + inner.w,
            inner.y + 2,
            &traffic,
            &light(theme::TEXT_DIM),
        );
        if net.present {
            let m = net.mac;
            let mac = format!(
                "MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}{}",
                m[0],
                m[1],
                m[2],
                m[3],
                m[4],
                m[5],
                net.gateway
                    .map(|g| format!(" · puerta {}", ip(g)))
                    .unwrap_or_default()
            );
            draw_fit(
                c,
                inner.x,
                inner.y + 22,
                &mac,
                &light(theme::TEXT_DIM),
                inner.w,
            );
        }
        let g = Rect::new(inner.x, inner.y + 44, inner.w, inner.h - 44);
        let max = h.rx.max().max(h.tx.max()).max(1024);
        graph(c, g, &h.rx, max, theme::CYAN);
        // La transmisión encima, en ámbar (sin relleno para que se vean las dos).
        overlay_line(c, g, h.tx.iter().collect(), h.tx.capacity(), max);

        // Apps abiertas
        let inner = card(c, l.list, "APLICACIONES");
        let _ = inner;
        if sys.tasks.is_empty() {
            text::draw(
                c,
                l.list.x + 16,
                l.list.y + 40,
                "No hay apps abiertas.",
                &s16(theme::TEXT_DIM),
            );
        }
        self.selected = self.selected.min(sys.tasks.len().saturating_sub(1));
        for (i, t) in sys.tasks.iter().take(l.visible_rows()).enumerate() {
            let row = l.row(i);
            if i == self.selected {
                c.fill_rect(row.x, row.y, row.w, row.h, crate::widgets::SELECTED_BG);
            }
            icon(
                c,
                icon_of(t.kind),
                row.x + 18,
                row.y + row.h / 2,
                theme::CYAN,
            );
            let state = if t.minimized {
                "minimizada"
            } else {
                "en ejecución"
            };
            draw_fit(
                c,
                row.x + 40,
                row.y + 8,
                &t.title,
                &s16(theme::TEXT),
                row.w - 360,
            );
            text::draw_right(
                c,
                row.x + row.w - 150,
                row.y + 8,
                state,
                &light(theme::TEXT_DIM),
            );
            button(c, l.end_button(i), "FINALIZAR", theme::CRIMSON, 25);
        }
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, ctx: &mut Ctx<'_, D>) -> bool {
        let n = ctx.tasks.len();
        match key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            Key::Delete => {
                if let Some(t) = ctx.tasks.get(self.selected) {
                    ctx.out.close.push(t.id);
                }
            }
            Key::Enter => {
                if let Some(t) = ctx.tasks.get(self.selected) {
                    ctx.out.activate = Some(t.id);
                }
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        let l = Layout::new(content);
        for (i, t) in ctx.tasks.iter().take(l.visible_rows()).enumerate() {
            if l.end_button(i).contains(click.x, click.y) {
                ctx.log.push(format!("MONITOR_FINALIZAR {}", t.title));
                ctx.out.close.push(t.id);
                return;
            }
            if l.row(i).contains(click.x, click.y) {
                self.selected = i;
                self.dirty = true;
                if click.double {
                    ctx.out.activate = Some(t.id);
                }
            }
        }
    }
}

/// Una segunda línea sobre un gráfico ya dibujado (la transmisión de red, en ámbar).
fn overlay_line(c: &mut Canvas<'_>, r: Rect, values: Vec<u32>, capacity: usize, max: u32) {
    let n = capacity as i32;
    let offset = n - values.len() as i32;
    let mut prev: Option<(i32, i32)> = None;
    for (i, v) in values.into_iter().enumerate() {
        let x = r.x + (r.w - 1) * (offset + i as i32) / (n - 1).max(1);
        let hh = (v.min(max) as i64 * (r.h - 2) as i64 / max.max(1) as i64) as i32;
        let y = r.y + r.h - 1 - hh;
        if let Some((px, py)) = prev {
            jarvis_gfx::shapes::line(c, px, py, x, y, theme::AMBER);
        }
        prev = Some((x, y));
    }
}
