//! Application state: simulated time, selection, camera and input handling.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::data::{self, Body};
use crate::math::V3;
use crate::render::{body_position, body_radius, has_rings, Camera, Canvas, Hit, Scene};
use crate::time;

/// Time speed presets: simulated days per real second, and their label.
pub const SPEEDS: [(f64, &str); 9] = [
    (1.0 / 86_400.0, "real time"),
    (1.0 / 1440.0, "1 min/s"),
    (1.0 / 24.0, "1 hour/s"),
    (0.25, "6 hours/s"),
    (1.0, "1 day/s"),
    (7.0, "1 week/s"),
    (30.4375, "1 month/s"),
    (365.25, "1 year/s"),
    (3652.5, "10 years/s"),
];
pub const DEFAULT_SPEED: usize = 5;
const SPEED_NAMES: [&str; 9] = [
    "realtime", "minute", "hour", "6hours", "day", "week", "month", "year", "decade",
];

const SYSTEM_DIST: f64 = 8.8;
const SYSTEM_EL: f64 = 0.52;
const FACT_SECONDS: f64 = 11.0;

/// Guided tour: what to show at each stop (None = the whole system), and how long to stay.
const TOUR: [Option<usize>; 10] = [
    None,
    Some(3),
    Some(5),
    Some(6),
    None,
    Some(0),
    Some(4),
    Some(7),
    Some(8),
    Some(1),
];
const TOUR_STOP: f64 = 9.0;

pub fn speed_from_name(s: &str) -> Option<usize> {
    SPEED_NAMES.iter().position(|n| n.eq_ignore_ascii_case(s))
}

pub fn speed_names() -> String {
    SPEED_NAMES.join(", ")
}

pub struct App {
    pub bodies: Vec<Body>,
    pub scene: Scene,
    pub canvas: Canvas,
    pub jd: f64,
    pub spin_jd: f64,
    pub speed: usize,
    pub paused: bool,
    pub reverse: bool,
    pub selected: usize,
    pub focused: Option<usize>,
    pub cam: Camera,
    goal_az: f64,
    goal_el: f64,
    system_dist: f64,
    focus_mul: f64,
    focus_age: f64,
    pub auto_rotate: bool,
    /// None follows the terminal width; Some after the user toggles it.
    pub show_panel: Option<bool>,
    pub panel_auto: bool,
    pub show_help: bool,
    /// Animation clock in seconds since start.
    pub t: f64,
    pub fact: usize,
    pub fact_since: f64,
    pub hits: Vec<Hit>,
    drag: Option<Drag>,
    /// Index of the current tour stop and when it began.
    pub tour: Option<(usize, f64)>,
    /// Cell size in pixels when drawing bodies as images (HD mode).
    pub hd: Option<(u32, u32)>,
    /// Images prepared by the last draw, for the graphics back end.
    pub images: Vec<crate::kitty::Placed>,
    /// Last redraw of each image slot.
    pub sent: std::collections::HashMap<u32, crate::render::Sent>,
    pub quit: bool,
}

#[derive(Clone, Copy)]
struct Drag {
    last: (u16, u16),
    moved: bool,
}

impl App {
    pub fn new(jd: f64) -> App {
        App {
            bodies: data::load(),
            scene: Scene::new(),
            canvas: Canvas::new(1, 1),
            jd,
            spin_jd: jd,
            speed: DEFAULT_SPEED,
            paused: false,
            reverse: false,
            selected: 3,
            focused: None,
            // start far out and high above, then glide in
            cam: Camera {
                target: V3::ZERO,
                dist: 42.0,
                az: -2.6,
                el: 1.25,
            },
            goal_az: -1.75,
            goal_el: SYSTEM_EL,
            system_dist: SYSTEM_DIST,
            focus_mul: 7.0,
            focus_age: 0.0,
            auto_rotate: true,
            show_panel: None,
            panel_auto: true,
            show_help: false,
            t: 0.0,
            fact: 0,
            fact_since: 0.0,
            hits: Vec::new(),
            drag: None,
            tour: None,
            hd: None,
            images: Vec::new(),
            sent: std::collections::HashMap::new(),
            quit: false,
        }
    }

    pub fn body_index(&self, name: &str) -> Option<usize> {
        self.bodies
            .iter()
            .position(|b| b.id.eq_ignore_ascii_case(name) || b.name.eq_ignore_ascii_case(name))
    }

    pub fn update(&mut self, dt: f64) {
        self.t += dt;
        if !self.paused {
            let dir = if self.reverse { -1.0 } else { 1.0 };
            self.jd += SPEEDS[self.speed].0 * dt * dir;
            // keep to years 1000..3000; the elements are best within 1800..2050
            self.jd = self.jd.clamp(
                time::jd_from_calendar(1000, 1, 1.0),
                time::jd_from_calendar(3000, 1, 1.0),
            );
            // spinning faster than an hour per second only strobes, so cap the visible spin
            if SPEEDS[self.speed].0 <= 1.0 / 24.0 {
                self.spin_jd = self.jd;
            } else {
                self.spin_jd += dir * dt / 24.0;
            }
        }
        if self.auto_rotate && self.drag.is_none() {
            self.goal_az += if self.focused.is_some() { 0.02 } else { 0.035 } * dt;
        }
        let k = 1.0 - (-dt * 2.6).exp();
        self.cam.az += (self.goal_az - self.cam.az) * k;
        self.cam.el += (self.goal_el - self.cam.el) * k;
        let (target, dist) = match self.focused {
            Some(i) => {
                self.focus_age += dt;
                let mul = if i == 0 {
                    self.focus_mul * 0.45
                } else {
                    self.focus_mul
                };
                (
                    body_position(i, self.jd),
                    body_radius(i, &self.bodies) * mul,
                )
            }
            None => (V3::ZERO, self.system_dist),
        };
        // ease the target in, then lock on so a fast-moving planet stays centred
        let kt = if self.focused.is_some() && self.focus_age > 1.5 {
            1.0
        } else {
            1.0 - (-dt * 3.0).exp()
        };
        self.cam.target = self.cam.target.lerp(target, kt);
        self.cam.dist = (self.cam.dist.ln() + (dist.ln() - self.cam.dist.ln()) * k).exp();
        if self.t - self.fact_since > FACT_SECONDS {
            self.next_fact();
        }
        if let Some((stop, since)) = self.tour {
            if self.t - since > TOUR_STOP {
                self.tour_stop((stop + 1) % TOUR.len());
            }
        }
    }

    pub fn start_tour(&mut self) {
        self.tour_stop(0);
    }

    fn tour_stop(&mut self, stop: usize) {
        self.tour = Some((stop, self.t));
        match TOUR[stop] {
            Some(i) => self.focus(i),
            None => {
                self.unfocus();
                self.select(TOUR[(stop + 1) % TOUR.len()].unwrap_or(3));
            }
        }
    }

    fn next_fact(&mut self) {
        let n = self.bodies[self.selected].facts.len().max(1);
        self.fact = (self.fact + 1) % n;
        self.fact_since = self.t;
    }

    pub fn select(&mut self, i: usize) {
        if i < self.bodies.len() && i != self.selected {
            self.selected = i;
            self.fact = 0;
            self.fact_since = self.t;
            if self.focused.is_some() {
                self.focus(i);
            }
        }
    }

    pub fn focus(&mut self, i: usize) {
        self.selected = i;
        self.focused = Some(i);
        self.focus_age = 0.0;
        self.focus_mul = 7.0;
        // look from about 45 degrees off the Sun direction so the body is mostly lit,
        // and from the sunlit side of the rings for ringed planets
        let pos = body_position(i, self.jd);
        let sun = if i == 0 {
            V3::new(1.0, 0.0, 0.0)
        } else {
            (-pos).norm()
        };
        let mut v = (sun.rot_z(0.8) + V3::Z * 0.25).norm();
        if has_rings(&self.bodies[i].id) {
            let pole = self.bodies[i].pole();
            let side = if sun.dot(pole) >= 0.0 { 1.0 } else { -1.0 };
            let open = v.dot(pole) * side;
            if open < 0.45 {
                v = (v + pole * (side * (0.45 - open) * 1.6)).norm();
            }
        }
        self.goal_el = v.z.clamp(-1.0, 1.0).asin().clamp(-1.3, 1.3);
        let az = v.y.atan2(v.x);
        let wrap = (az - self.cam.az + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        self.goal_az = self.cam.az + wrap;
    }

    pub fn unfocus(&mut self) {
        self.focused = None;
        self.goal_el = SYSTEM_EL;
    }

    fn zoom(&mut self, factor: f64) {
        if self.focused.is_some() {
            self.focus_mul = (self.focus_mul * factor).clamp(2.2, 60.0);
        } else {
            self.system_dist = (self.system_dist * factor).clamp(2.0, 40.0);
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        let n = self.bodies.len();
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        // any other key takes the controls back from the tour
        if !matches!(key.code, KeyCode::Char('T')) {
            self.tour = None;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('T') => match self.tour {
                Some(_) => self.tour = None,
                None => self.start_tour(),
            },
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                } else if self.focused.is_some() {
                    self.unfocus();
                }
            }
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.select((self.selected + 1) % n)
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.select((self.selected + n - 1) % n)
            }
            KeyCode::Char(c @ '0'..='8') => self.select(c as usize - '0' as usize),
            KeyCode::Enter | KeyCode::Char('f') => match self.focused {
                Some(i) if i == self.selected => self.unfocus(),
                _ => self.focus(self.selected),
            },
            KeyCode::Backspace => self.unfocus(),
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char(']') | KeyCode::Char('.') => {
                self.speed = (self.speed + 1).min(SPEEDS.len() - 1)
            }
            KeyCode::Char('[') | KeyCode::Char(',') => self.speed = self.speed.saturating_sub(1),
            KeyCode::Char('b') | KeyCode::Char('\\') => self.reverse = !self.reverse,
            KeyCode::Char('t') => {
                self.jd = time::jd_now();
                self.reverse = false;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom(0.8),
            KeyCode::Char('-') | KeyCode::Char('_') => self.zoom(1.25),
            KeyCode::Up | KeyCode::Char('k') => self.goal_el = (self.goal_el + 0.12).min(1.45),
            KeyCode::Down | KeyCode::Char('j') => self.goal_el = (self.goal_el - 0.12).max(-1.45),
            KeyCode::Char('a') => self.goal_az -= 0.25,
            KeyCode::Char('d') => self.goal_az += 0.25,
            KeyCode::Char('r') => self.auto_rotate = !self.auto_rotate,
            KeyCode::Char('i') => {
                self.show_panel = Some(!self.show_panel.unwrap_or(self.panel_auto))
            }
            _ => {}
        }
    }

    pub fn mouse(&mut self, ev: MouseEvent) {
        if !matches!(ev.kind, MouseEventKind::Moved) {
            self.tour = None;
        }
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag = Some(Drag {
                    last: (ev.column, ev.row),
                    moved: false,
                })
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = self.drag.as_mut() {
                    let dx = ev.column as f64 - d.last.0 as f64;
                    let dy = ev.row as f64 - d.last.1 as f64;
                    d.last = (ev.column, ev.row);
                    d.moved = true;
                    self.goal_az -= dx * 0.03;
                    self.goal_el = (self.goal_el + dy * 0.06).clamp(-1.45, 1.45);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(d) = self.drag.take() {
                    if !d.moved {
                        self.click(ev.column as f64 + 0.5, ev.row as f64 + 0.5);
                    }
                }
            }
            MouseEventKind::ScrollUp => self.zoom(0.85),
            MouseEventKind::ScrollDown => self.zoom(1.18),
            _ => {}
        }
    }

    /// Click a body to select it; click the selected body again to fly to it.
    fn click(&mut self, x: f64, y: f64) {
        let best = self
            .hits
            .iter()
            .map(|h| {
                (
                    h.body,
                    ((h.x - x).powi(2) + ((h.y - y) * 2.0).powi(2)).sqrt() - h.r,
                )
            })
            .filter(|&(_, d)| d < 2.5)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            if i == self.selected {
                self.focus(i);
            } else {
                self.select(i);
            }
        }
    }
}
