//! Monitor del sistema (como el Administrador de tareas de Windows): CPU, memoria, disco y red
//! con gráficos del último minuto, y la lista de apps abiertas con "Finalizar tarea".

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::hud::icon;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::{Click, Ctx, SysView, icon_of};
use crate::files::format_size;
use crate::i18n::{tr, trf};
use crate::input::Key;
use crate::widgets::{
    bar, big, button, card, draw_fit, duration, graph, ip, label, light, rate, s16, window_bg,
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
    /// Procesador, memoria, temperatura (arriba); disco y red (abajo).
    cards: [Rect; 5],
    list: Rect,
}

impl Layout {
    fn new(r: Rect) -> Layout {
        let pad = 14;
        let top = r.y + 46;
        let list_h = (r.h / 3).clamp(110, 200);
        let cards_h = r.h - 46 - list_h - pad * 2;
        let ch = (cards_h - pad) / 2;
        let cw3 = (r.w - pad * 4) / 3;
        let cw2 = (r.w - pad * 3) / 2;
        let top3 = |i: i32| Rect::new(r.x + pad + i * (cw3 + pad), top, cw3, ch);
        let bottom2 = |i: i32| Rect::new(r.x + pad + i * (cw2 + pad), top + ch + pad, cw2, ch);
        Layout {
            cards: [top3(0), top3(1), bottom2(0), bottom2(1), top3(2)],
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
        tr("Monitor del sistema").into()
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, sys: &SysView<'_>) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        let st = sys.stats;
        let h = sys.history;
        let l = Layout::new(r);

        text::draw(
            c,
            r.x + 16,
            r.y + 16,
            tr("RENDIMIENTO"),
            &label(theme::cyan()),
        );
        let summary = trf(
            "ENCENDIDO HACE {} · {} FPS · {} MS POR FRAME",
            &[
                &duration(st.uptime_ms).to_uppercase(),
                &st.fps.to_string(),
                &st.frame_ms.to_string(),
            ],
        );
        text::draw_right(
            c,
            r.x + r.w - 16,
            r.y + 16,
            &summary,
            &label(theme::text_dim()),
        );

        // CPU
        let inner = card(c, l.cards[0], tr("PROCESADOR"));
        text::draw(
            c,
            inner.x,
            inner.y,
            &format!("{} %", st.cpu_pct),
            &big(theme::cyan()),
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
            &light(theme::text_dim()),
            inner.w - 110,
        );
        let g = Rect::new(inner.x, inner.y + 42, inner.w, inner.h - 42);
        graph(c, g, &h.cpu, 100, theme::cyan());

        // Temperatura
        let inner = card(c, l.cards[4], tr("TEMPERATURA"));
        match st.temp_c {
            Some(t) => {
                let color = if t >= 85 {
                    theme::crimson()
                } else if t >= 70 {
                    theme::amber()
                } else {
                    theme::cyan()
                };
                text::draw(
                    c,
                    inner.x,
                    inner.y,
                    &crate::shell::temp_text(Some(t)),
                    &big(color),
                );
                let g = Rect::new(inner.x, inner.y + 42, inner.w, inner.h - 42);
                graph(c, g, &h.temp, 100, theme::crimson());
            }
            None => {
                text::draw(c, inner.x, inner.y, "--", &big(theme::text_dim()));
                let st = light(theme::text_dim());
                let mut y = inner.y + 44;
                for l in [
                    tr("Sin sensor térmico."),
                    tr("Las máquinas virtuales no lo emulan;"),
                    tr("en una PC Intel se lee de la CPU."),
                ] {
                    draw_fit(c, inner.x, y, l, &st, inner.w);
                    y += 20;
                }
            }
        }

        // Memoria
        let inner = card(c, l.cards[1], tr("MEMORIA (HEAP DEL NÚCLEO)"));
        let pct = (st.heap_used * 100).checked_div(st.heap_total).unwrap_or(0) as u32;
        text::draw(
            c,
            inner.x,
            inner.y,
            &format!("{pct} %"),
            &big(theme::particle_bright()),
        );
        let heap = format!(
            "{} de {}",
            format_size(st.heap_used),
            format_size(st.heap_total)
        );
        // La RAM física que el allocator de marcos todavía no entregó (paginación propia, K8).
        let ram = format!("{} {}", tr("RAM libre"), format_size(st.ram_free));
        for (i, line) in [heap, ram].iter().enumerate() {
            draw_fit(
                c,
                inner.x + 110,
                inner.y + 1 + 18 * i as i32,
                line,
                &light(theme::text_dim()),
                inner.w - 110,
            );
        }
        let g = Rect::new(inner.x, inner.y + 42, inner.w, inner.h - 42);
        graph(c, g, &h.mem, 100, theme::particle_bright());

        // Disco
        let inner = card(c, l.cards[2], tr("DISCO"));
        match sys.disk {
            Some((name, free, total)) => {
                let used = total.saturating_sub(free);
                let pct = (used * 100).checked_div(total).unwrap_or(0) as u32;
                text::draw(
                    c,
                    inner.x,
                    inner.y,
                    &format!("{pct} %"),
                    &big(theme::amber()),
                );
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
                    &light(theme::text_dim()),
                    inner.w - 110,
                );
                bar(
                    c,
                    Rect::new(inner.x, inner.y + 40, inner.w, 5),
                    pct,
                    theme::amber(),
                );
            }
            None => {
                text::draw(
                    c,
                    inner.x,
                    inner.y + 8,
                    tr("Sin disco"),
                    &s16(theme::text_dim()),
                );
            }
        }
        let g = Rect::new(inner.x, inner.y + 52, inner.w, inner.h - 52);
        graph(c, g, &h.disk, 0, theme::amber());

        // Red
        let inner = card(c, l.cards[3], tr("RED"));
        let net = &st.net;
        let (headline, color) = match (net.present, net.ip) {
            (false, _) => (String::from(tr("Sin placa de red")), theme::text_dim()),
            (true, None) => (
                String::from(tr("Pidiendo dirección (DHCP)...")),
                theme::amber(),
            ),
            (true, Some(a)) => (ip(a), theme::cyan()),
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
            &light(theme::text_dim()),
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
                &light(theme::text_dim()),
                inner.w,
            );
        }
        let g = Rect::new(inner.x, inner.y + 44, inner.w, inner.h - 44);
        let max = h.rx.max().max(h.tx.max()).max(1024);
        graph(c, g, &h.rx, max, theme::cyan());
        // La transmisión encima, en ámbar (sin relleno para que se vean las dos).
        overlay_line(c, g, h.tx.iter().collect(), h.tx.capacity(), max);

        // Apps abiertas
        let inner = card(c, l.list, tr("APLICACIONES"));
        let _ = inner;
        if sys.tasks.is_empty() {
            text::draw(
                c,
                l.list.x + 16,
                l.list.y + 40,
                tr("No hay apps abiertas."),
                &s16(theme::text_dim()),
            );
        }
        self.selected = self.selected.min(sys.tasks.len().saturating_sub(1));
        for (i, t) in sys.tasks.iter().take(l.visible_rows()).enumerate() {
            let row = l.row(i);
            if i == self.selected {
                c.fill_rect(row.x, row.y, row.w, row.h, crate::widgets::selected_bg());
            }
            icon(
                c,
                icon_of(t.kind),
                row.x + 18,
                row.y + row.h / 2,
                theme::cyan(),
            );
            let state = if t.minimized {
                tr("minimizada")
            } else {
                tr("en ejecución")
            };
            draw_fit(
                c,
                row.x + 40,
                row.y + 8,
                &t.title,
                &s16(theme::text()),
                row.w - 360,
            );
            text::draw_right(
                c,
                row.x + row.w - 150,
                row.y + 8,
                state,
                &light(theme::text_dim()),
            );
            button(c, l.end_button(i), tr("FINALIZAR"), theme::crimson(), 25);
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
            jarvis_gfx::shapes::line(c, px, py, x, y, theme::amber());
        }
        prev = Some((x, y));
    }
}
