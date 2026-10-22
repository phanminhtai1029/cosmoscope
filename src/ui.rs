//! Layout and widgets: the space view, the info panel, the status bar and the help overlay.

use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::app::{App, SPEEDS};
use crate::ephemeris;
use crate::kitty::Placed;
use crate::math::Rgb;
use crate::render::{self, Canvas, Sprite};

thread_local! {
    static CARD: std::cell::RefCell<Canvas> = std::cell::RefCell::new(Canvas::new(1, 1));
}
use crate::time;

static TRUECOLOR: AtomicBool = AtomicBool::new(true);

pub fn set_truecolor(on: bool) {
    TRUECOLOR.store(on, Ordering::Relaxed);
}

/// Terminal colour for an RGB value, mapped to the 256-colour cube when needed.
pub fn color(c: Rgb) -> Color {
    let (r, g, b) = c.to_u8();
    if TRUECOLOR.load(Ordering::Relaxed) {
        Color::Rgb(r, g, b)
    } else {
        let q = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
        Color::Indexed(16 + 36 * q(r) + 6 * q(g) + q(b))
    }
}

const DIM: Rgb = Rgb::hex(0x6b7194);
const TEXT: Rgb = Rgb::hex(0xc9cde6);
const BRIGHT: Rgb = Rgb::hex(0xf1f2fb);
const ACCENT: Rgb = Rgb::hex(0x8fb4ff);
const WARN: Rgb = Rgb::hex(0xffb35c);

struct SpaceView<'a>(&'a Canvas);

impl Widget for SpaceView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let cv = self.0;
        for y in 0..area.height.min(cv.h as u16) {
            for x in 0..area.width.min(cv.w as u16) {
                let c = cv.cell(x as usize, y as usize);
                if c.ch == ' ' {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                    cell.set_char(c.ch);
                    if let Some(fg) = c.fg {
                        cell.set_fg(color(fg));
                    }
                    if let Some(bg) = c.bg {
                        cell.set_bg(color(bg));
                    }
                }
            }
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    // the panel shows itself on wide terminals; 'i' forces it either way
    let wide = main.width >= 110;
    app.panel_auto = wide;
    let show = app.show_panel.unwrap_or(wide) && main.width >= 60;
    let panel_w = if show {
        ((main.width as f32 * 0.28) as u16)
            .clamp(38, 48)
            .min(main.width - 20)
    } else {
        0
    };
    app.canvas.resize(main.width as usize, main.height as usize);
    let frame = render::Frame {
        bodies: &app.bodies,
        jd: app.jd,
        spin_jd: app.spin_jd,
        spin_rate: app.spin_rate(),
        t: app.t,
        cam: app.cam,
        selected: app.selected,
        focused: app.focused,
        center_x: (main.width - panel_w) as f64 * 0.5,
        // images would cover the help box, so bodies fall back to text while it is open
        hd: app.hd.filter(|_| !app.show_help),
        sent: Some(&app.sent),
        budget: Some(&app.budget),
    };
    app.budget.set(render::REDRAW_BUDGET);
    app.canvas.label_limit = (main.width - panel_w) as usize;
    let (hits, sprites) =
        crate::profile::time("scene", || app.scene.render(&mut app.canvas, &frame));
    app.hits = hits;
    let redrawn: Vec<(u32, render::Sent)> = sprites
        .iter()
        .filter(|s| !s.keep)
        .map(|s| (s.slot, s.sent(app.t, app.hd.unwrap_or((1, 1)))))
        .collect();
    let space = Rect::new(main.x, main.y, main.width - panel_w, main.height);
    let cell = app.hd.unwrap_or((1, 1));
    let mut images: Vec<Placed> = sprites
        .into_iter()
        .filter_map(|s| place(s, main, space, cell))
        .collect();
    f.render_widget(SpaceView(&app.canvas), main);

    if panel_w > 0 {
        let panel = Rect::new(main.right() - panel_w, main.y, panel_w, main.height);
        if let Some((card, sent)) =
            draw_panel(f, app, panel, &frame_for_card(app, main.width - panel_w))
        {
            images.push(card);
            if let Some(sent) = sent {
                app.sent.insert(render::CARD_SLOT, sent);
            }
        }
    }
    app.sent.extend(redrawn);
    app.images = images;
    draw_status(f, app, status);
    if app.show_help {
        draw_help(f, main);
    }
}

fn frame_for_card(app: &App, _w: u16) -> render::Frame<'_> {
    render::Frame {
        bodies: &app.bodies,
        jd: app.jd,
        spin_jd: app.spin_jd,
        spin_rate: app.spin_rate(),
        t: app.t,
        cam: app.cam,
        selected: app.selected,
        focused: app.focused,
        center_x: 0.0,
        hd: app.hd.filter(|_| !app.show_help),
        sent: Some(&app.sent),
        budget: Some(&app.budget),
    }
}

fn st(c: Rgb) -> Style {
    Style::default().fg(color(c))
}

fn thousands(v: f64) -> String {
    let s = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0.0 {
        format!("-{out}")
    } else {
        out
    }
}

fn duration_hours(h: f64) -> String {
    let h = h.abs();
    if h < 72.0 {
        let total = (h * 60.0).round() as u64;
        format!("{} h {:02} m", total / 60, total % 60)
    } else {
        format!("{:.1} Earth days", h / 24.0)
    }
}

fn duration_days(d: f64) -> String {
    if d < 1000.0 {
        format!("{d:.1} Earth days")
    } else {
        format!("{:.2} Earth years", d / 365.25)
    }
}

fn light_time(au: f64) -> String {
    let s = (au * 499.004_784).round() as u64;
    if s >= 3600 {
        format!("{} h {:02} m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{} m {:02} s", s / 60, s % 60)
    }
}

fn ordinal(n: usize) -> String {
    let suffix = match n {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn row<'a>(label: &'a str, value: String) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {label:<12}"), st(DIM)),
        Span::styled(value, st(TEXT)),
    ])
}

fn header(label: &str, col: Rgb) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {label}"),
        st(col).add_modifier(Modifier::BOLD),
    ))
}

/// A thin progress bar made of heavy and light line segments.
fn bar(frac: f64, width: usize, col: Rgb) -> Vec<Span<'static>> {
    let filled = ((frac.clamp(0.0, 1.0)) * width as f64).round() as usize;
    vec![
        Span::styled("━".repeat(filled), st(col)),
        Span::styled("─".repeat(width - filled), st(Rgb::hex(0x3a3f5c))),
        Span::styled(format!(" {:>3.0}%", frac * 100.0), st(TEXT)),
    ]
}

fn draw_panel(
    f: &mut Frame,
    app: &App,
    area: Rect,
    card_frame: &render::Frame,
) -> Option<(Placed, Option<render::Sent>)> {
    let i = app.selected;
    let b = &app.bodies[i];
    let col = b.rgb();
    let earth_au = ephemeris::position(2, app.jd);
    let mut lines: Vec<Line> = Vec::new();
    let subtitle = if i == 0 {
        "the centre".to_string()
    } else {
        format!("{} from the Sun", ordinal(i))
    };
    lines.push(Line::from(vec![
        Span::styled(format!(" {}", b.kind), st(col.mix(TEXT, 0.4))),
        Span::styled(format!(" · {subtitle}"), st(DIM)),
    ]));
    lines.push(Line::default());
    lines.push(header("LIVE", ACCENT));
    if i == 0 {
        let d = earth_au.len();
        lines.push(row("From Earth", format!("{d:.3} AU")));
        lines.push(row("Light time", light_time(d)));
    } else {
        let orbit = ephemeris::orbit(i - 1, app.jd);
        let pos = orbit.position();
        let r = pos.len();
        lines.push(row(
            "From Sun",
            format!("{r:.2} AU · {} M km", thousands(r * 149.597_870_7)),
        ));
        if i != 3 {
            let d = (pos - earth_au).len();
            lines.push(row("From Earth", format!("{d:.2} AU")));
            lines.push(row("Light time", light_time(d)));
        }
        let v = 29.784_7 * (2.0 / r - 1.0 / orbit.a).sqrt();
        lines.push(row("Speed", format!("{v:.1} km/s")));
        let mut spans = vec![Span::styled(format!(" {:<12}", "Orbit"), st(DIM))];
        let bar_w = (area.width as usize).saturating_sub(22).clamp(6, 20);
        spans.extend(bar(orbit.mean_anomaly / std::f64::consts::TAU, bar_w, col));
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines.push(header("PROFILE", ACCENT));
    let d_km = b.radius_km * 2.0;
    lines.push(row("Diameter", format!("{} km", thousands(d_km))));
    lines.push(row(
        "Size",
        format!("{:.1}× Earth's width", b.radius_km / 6378.0),
    ));
    lines.push(row(
        "Mass",
        if b.mass_earths >= 10.0 {
            format!("{}× Earth", thousands(b.mass_earths))
        } else {
            format!("{:.3}× Earth", b.mass_earths)
        },
    ));
    lines.push(row(
        "Gravity",
        format!("{:.1} m/s² · {:.2} g", b.gravity, b.gravity / 9.81),
    ));
    if i == 0 {
        lines.push(row("Rotation", duration_hours(b.rotation_hours)));
        lines.push(row("Surface", format!("{} °C", thousands(b.mean_temp_c))));
        lines.push(row("Planets", "8".to_string()));
    } else {
        let retro = if b.rotation_hours < 0.0 {
            " (retrograde)"
        } else {
            ""
        };
        lines.push(row(
            "Day",
            format!("{}{retro}", duration_hours(b.day_hours)),
        ));
        lines.push(row("Year", duration_days(b.year_days)));
        lines.push(row("Avg temp", format!("{:.0} °C", b.mean_temp_c)));
        lines.push(row("Moons", b.moons.to_string()));
        lines.push(row("Axial tilt", format!("{:.1}°", b.tilt_deg)));
    }
    lines.push(Line::default());
    lines.push(header("DID YOU KNOW?", WARN));
    let fact = &b.facts[app.fact.min(b.facts.len() - 1)];
    let shown = (((app.t - app.fact_since) * 55.0) as usize).min(fact.chars().count());
    let mut text: String = fact.chars().take(shown).collect();
    if shown < fact.chars().count() {
        text.push('▏');
    }
    let dots: String = (0..b.facts.len())
        .map(|k| if k == app.fact { '●' } else { '·' })
        .collect::<Vec<_>>()
        .iter()
        .map(|c| format!("{c} "))
        .collect();

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(st(col.mix(Rgb::hex(0x30344d), 0.55)))
        .title(Span::styled(
            format!(" {} ", b.name.to_uppercase()),
            st(col.mix(BRIGHT, 0.25)).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(" enter: fly here · ←/→: next ", st(DIM)))
                .alignment(Alignment::Right),
        );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let fixed = lines.len() as u16;
    // the close-up takes whatever height is left after the text, within reason
    let spare = inner.height.saturating_sub(fixed + 5);
    let card_h = if spare >= 8 {
        spare.min(inner.width / 2).min(18)
    } else {
        0
    };
    let [card_area, top, fact_area, dots_area] = Layout::vertical([
        Constraint::Length(card_h),
        Constraint::Length(fixed),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut card = None;
    if card_h > 0 {
        CARD.with(|c| {
            let mut cv = c.borrow_mut();
            cv.resize(card_area.width as usize, card_area.height as usize);
            if app.focused == Some(i) {
                render::render_minimap(&mut cv, card_frame, i);
            } else {
                card = crate::profile::time("card", || {
                    render::render_card(&mut cv, &app.scene, card_frame, i)
                })
                .and_then(|s| {
                    let sent = (!s.keep).then(|| s.sent(app.t, app.hd.unwrap_or((1, 1))));
                    place(s, card_area, card_area, app.hd.unwrap_or((1, 1))).map(|p| (p, sent))
                });
            }
            f.render_widget(SpaceView(&cv), card_area);
        });
    }
    f.render_widget(Paragraph::new(lines), top);
    f.render_widget(
        Paragraph::new(Span::styled(text, st(BRIGHT)))
            .wrap(Wrap { trim: true })
            .block(Block::new().padding(ratatui::widgets::Padding::horizontal(1))),
        fact_area,
    );
    f.render_widget(
        Paragraph::new(Span::styled(format!(" {dots}"), st(DIM))),
        dots_area,
    );
    card
}

/// Turn a sprite (cells relative to `origin`) into an absolute placement, cropped to `clip`.
fn place(s: Sprite, origin: Rect, clip: Rect, cell: (u32, u32)) -> Option<Placed> {
    let (cw, ch) = (cell.0 as i64, cell.1 as i64);
    // absolute top-left in screen pixels, and the clip rectangle in pixels
    let (ax, ay) = (
        origin.x as i64 * cw + s.x as i64,
        origin.y as i64 * ch + s.y as i64,
    );
    let (cx0, cy0) = (clip.x as i64 * cw, clip.y as i64 * ch);
    let (cx1, cy1) = (clip.right() as i64 * cw, clip.bottom() as i64 * ch);
    if s.cols > 0 {
        // terminal-scaled image on whole cells: crop in cells, map the crop to image pixels
        let (c0, r0) = (ax / cw, ay / ch);
        let (k0, k1) = (c0.max(cx0 / cw), (c0 + s.cols as i64).min(cx1 / cw));
        let (q0, q1) = (r0.max(cy0 / ch), (r0 + s.rows as i64).min(cy1 / ch));
        if k0 >= k1 || q0 >= q1 {
            return None;
        }
        let (fx, fy) = (s.w as f64 / s.cols as f64, s.h as f64 / s.rows as f64);
        let sx0 = ((k0 - c0) as f64 * fx).round() as u32;
        let sy0 = ((q0 - r0) as f64 * fy).round() as u32;
        let sx1 = (((k1 - c0) as f64 * fx).round() as u32).min(s.w);
        let sy1 = (((q1 - r0) as f64 * fy).round() as u32).min(s.h);
        return Some(Placed {
            slot: s.slot,
            col: k0 as u16,
            row: q0 as u16,
            x_off: 0,
            y_off: 0,
            src: (
                sx0,
                sy0,
                sx1.saturating_sub(sx0).max(1),
                sy1.saturating_sub(sy0).max(1),
            ),
            cols: (k1 - k0) as u32,
            rows: (q1 - q0) as u32,
            w: s.w,
            h: s.h,
            rgba: s.rgba,
            keep: s.keep,
            z: s.z,
        });
    }
    let (sx0, sy0) = ((cx0 - ax).max(0), (cy0 - ay).max(0));
    let (sx1, sy1) = ((cx1 - ax).min(s.w as i64), (cy1 - ay).min(s.h as i64));
    if sx1 <= sx0 || sy1 <= sy0 {
        return None;
    }
    let (px, py) = (ax + sx0, ay + sy0);
    Some(Placed {
        slot: s.slot,
        col: (px / cw) as u16,
        row: (py / ch) as u16,
        x_off: (px % cw) as u32,
        y_off: (py % ch) as u32,
        src: (
            sx0 as u32,
            sy0 as u32,
            (sx1 - sx0) as u32,
            (sy1 - sy0) as u32,
        ),
        cols: 0,
        rows: 0,
        w: s.w,
        h: s.h,
        rgba: s.rgba,
        keep: s.keep,
        z: s.z,
    })
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let year = time::year_of(app.jd);
    let speed = if app.paused {
        Span::styled("■ paused", st(WARN))
    } else {
        let arrow = if app.reverse { "◄" } else { "►" };
        Span::styled(format!("{arrow} {}", SPEEDS[app.speed].1), st(ACCENT))
    };
    let mut left = vec![
        Span::styled(" ◉ cosmoscope ", st(BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled("│ ", st(DIM)),
        Span::styled(format!("{} UTC", time::format_jd(app.jd)), st(TEXT)),
        Span::styled(" │ ", st(DIM)),
        speed,
    ];
    if !(1800.0..2050.0).contains(&year) {
        left.push(Span::styled(
            "  positions approximate outside 1800-2050",
            st(WARN),
        ));
    }
    if app.hd.is_some() {
        left.push(Span::styled(" │ ", st(DIM)));
        left.push(Span::styled(
            "HD",
            st(Rgb::hex(0x7ee2a8)).add_modifier(Modifier::BOLD),
        ));
    }
    if app.tour.is_some() {
        left.push(Span::styled(" │ ", st(DIM)));
        left.push(Span::styled("tour", st(WARN)));
    }
    if let Some(i) = app.focused {
        left.push(Span::styled(" │ ", st(DIM)));
        left.push(Span::styled(
            format!("◎ {}", app.bodies[i].name),
            st(app.bodies[i].rgb()),
        ));
    }
    let hints = "←→ select  enter fly  space pause  [ ] speed  drag/scroll view  ? help  q quit ";
    let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let free = (area.width as usize).saturating_sub(used);
    let mut spans = left;
    if free > 12 {
        let h: String = if hints.chars().count() <= free {
            format!("{hints:>free$}")
        } else {
            let cut: String = hints.chars().take(free.saturating_sub(1)).collect();
            format!("{cut:>free$}")
        };
        spans.push(Span::styled(h, st(DIM)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let keys: [(&str, &str); 16] = [
        ("← → / Tab", "select previous / next body"),
        ("0 - 8", "select Sun, Mercury ... Neptune"),
        ("Enter / click", "fly to the selected body (again to leave)"),
        ("Esc / Backspace", "back to the whole system"),
        ("Space", "pause / resume time"),
        ("[  ]", "slower / faster time"),
        ("b", "run time backwards"),
        ("t", "jump to now"),
        ("drag / a d / ↑ ↓", "rotate and tilt the view"),
        ("scroll / + -", "zoom"),
        ("r", "toggle auto-rotation"),
        ("i", "toggle the info panel"),
        ("T", "start / stop the guided tour"),
        ("?", "toggle this help"),
        ("q", "quit"),
        ("", ""),
    ];
    let mut lines: Vec<Line> = keys
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("  {k:<18}"), st(ACCENT)),
                Span::styled(*d, st(TEXT)),
            ])
        })
        .collect();
    lines.insert(0, Line::default());
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Distances and sizes are compressed so everything fits.",
        st(DIM),
    )));
    lines.push(Line::from(Span::styled(
        "  Positions are real for the date shown (JPL elements).",
        st(DIM),
    )));
    let w = 64.min(area.width.saturating_sub(4));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(st(ACCENT))
                .title(Span::styled(
                    " keys ",
                    st(BRIGHT).add_modifier(Modifier::BOLD),
                )),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite(x: f64, y: f64, w: u32, h: u32) -> Sprite {
        Sprite {
            slot: 1,
            x,
            y,
            w,
            h,
            rgba: vec![0; (w * h * 4) as usize],
            keep: false,
            z: 1,
            cols: 0,
            rows: 0,
            low: false,
        }
    }

    #[test]
    fn place_positions_to_the_pixel_and_crops_to_the_clip() {
        // 10x20 cells; a 40x30 image at pixel (95, 47) inside an area that starts at cell 2,
        // clipped where the panel starts at cell 12 (pixel 120)
        let p = place(
            sprite(95.0, 47.0, 40, 30),
            Rect::new(2, 0, 20, 10),
            Rect::new(2, 0, 10, 10),
            (10, 20),
        )
        .unwrap();
        assert_eq!((p.col, p.row, p.x_off, p.y_off), (11, 2, 5, 7));
        assert_eq!(p.src, (0, 0, 5, 30));
    }

    #[test]
    fn place_crops_the_top_left_too() {
        let p = place(
            sprite(-6.0, -4.0, 20, 20),
            Rect::new(0, 0, 20, 10),
            Rect::new(0, 0, 20, 10),
            (10, 20),
        )
        .unwrap();
        assert_eq!((p.col, p.row, p.x_off, p.y_off), (0, 0, 0, 0));
        assert_eq!(p.src, (6, 4, 14, 16));
    }

    #[test]
    fn scaled_sprites_crop_in_whole_cells() {
        // a 4x2-cell image rendered at 20x10 pixels, starting at cell 9; the clip ends at cell 12
        let s = Sprite {
            x: 90.0,
            y: 20.0,
            cols: 4,
            rows: 2,
            ..sprite(0.0, 0.0, 20, 10)
        };
        let p = place(
            s,
            Rect::new(0, 0, 20, 10),
            Rect::new(0, 0, 12, 10),
            (10, 20),
        )
        .unwrap();
        assert_eq!((p.col, p.row, p.cols, p.rows, p.x_off), (9, 1, 3, 2, 0));
        assert_eq!(p.src, (0, 0, 15, 10));
    }

    #[test]
    fn place_drops_sprites_outside() {
        assert!(place(
            sprite(400.0, 0.0, 20, 20),
            Rect::new(0, 0, 20, 10),
            Rect::new(0, 0, 20, 10),
            (10, 20)
        )
        .is_none());
    }
}
