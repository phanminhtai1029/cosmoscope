//! Software renderer. The scene is drawn into a cell grid with three layers:
//! block "pixels" (2x2 per cell, shown with half-block and quadrant glyphs) for the Sun and
//! planets, braille dots (2x4 per cell)
//! for orbits and small particles, and plain glyphs for the sky. Empty cells keep the
//! terminal's own background, so transparent terminals stay transparent.

use crate::data::Body;
use crate::ephemeris;
use crate::math::{fbm, hashf, smoothstep, vnoise, Rgb, TAU, V3};
use crate::textures;
use crate::time::J2000;

/// Final content of one terminal cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
}

impl Cell {
    pub const EMPTY: Cell = Cell {
        ch: ' ',
        fg: None,
        bg: None,
    };
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<Option<(Rgb, f64)>>,
    dots: Vec<u8>,
    dot_col: Vec<(Rgb, f64)>,
    glyph: Vec<Option<(char, Rgb, f64)>>,
    label: Vec<Option<(char, Rgb)>>,
    haze: Vec<f64>,
    /// Labels stay left of this column (the info panel covers the rest).
    pub label_limit: usize,
    /// Cells covered by an HD image, which labels must avoid.
    reserved: Vec<bool>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        let n = w * h;
        Canvas {
            w,
            h,
            px: vec![None; n * 4],
            dots: vec![0; n],
            dot_col: vec![(Rgb::BLACK, 0.0); n],
            glyph: vec![None; n],
            label: vec![None; n],
            haze: vec![0.0; n],
            label_limit: w,
            reserved: vec![false; n],
        }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        if w != self.w || h != self.h {
            *self = Canvas::new(w, h);
        }
    }

    fn clear(&mut self) {
        self.px.fill(None);
        self.dots.fill(0);
        self.dot_col.fill((Rgb::BLACK, 0.0));
        self.glyph.fill(None);
        self.label.fill(None);
        self.haze.fill(0.0);
        self.reserved.fill(false);
    }

    /// Mark the cells under an ellipse (centre and radii in cells) as taken.
    fn reserve(&mut self, cx: f64, cy: f64, rx: f64, ry: f64) {
        for y in (cy - ry).floor() as i64..=(cy + ry).ceil() as i64 {
            for x in (cx - rx).floor() as i64..=(cx + rx).ceil() as i64 {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                if dx * dx + dy * dy <= 1.2
                    && x >= 0
                    && y >= 0
                    && (x as usize) < self.w
                    && (y as usize) < self.h
                {
                    self.reserved[y as usize * self.w + x as usize] = true;
                }
            }
        }
    }

    /// Set a block pixel (x in 0..2w, y in 0..2h) if it is nearer than what is there.
    fn pixel(&mut self, x: i64, y: i64, col: Rgb, depth: f64) {
        if x < 0 || y < 0 || x >= 2 * self.w as i64 || y >= 2 * self.h as i64 {
            return;
        }
        let i = y as usize * 2 * self.w + x as usize;
        if self.px[i].is_none_or(|(_, d)| depth < d) {
            self.px[i] = Some((col, depth));
        }
    }

    /// Light one braille dot (x in 0..2w, y in 0..4h); the brightest dot sets the cell colour.
    fn dot(&mut self, x: i64, y: i64, col: Rgb, strength: f64) {
        if x < 0 || y < 0 || x >= 2 * self.w as i64 || y >= 4 * self.h as i64 || strength <= 0.02 {
            return;
        }
        const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let (cx, cy) = (x as usize / 2, y as usize / 4);
        let i = cy * self.w + cx;
        self.dots[i] |= BITS[x as usize % 2][y as usize % 4];
        if strength > self.dot_col[i].1 {
            self.dot_col[i] = (col, strength);
        }
    }

    fn line(&mut self, a: (f64, f64), b: (f64, f64), col: Rgb, strength: f64) {
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as i64).clamp(1, 4000);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            self.dot(
                (a.0 + (b.0 - a.0) * t) as i64,
                (a.1 + (b.1 - a.1) * t) as i64,
                col,
                strength,
            );
        }
    }

    fn glyph(&mut self, x: i64, y: i64, ch: char, col: Rgb, strength: f64) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        if self.glyph[i].is_none_or(|(_, _, s)| strength > s) {
            self.glyph[i] = Some((ch, col, strength));
        }
    }

    /// Place a label unless it would overlap another label or run off the canvas.
    fn text(&mut self, x: i64, y: i64, s: &str, col: Rgb) -> bool {
        let n = s.chars().count() as i64;
        if y < 0 || y >= self.h as i64 || x < 0 || x + n > self.label_limit.min(self.w) as i64 {
            return false;
        }
        let row = y as usize * self.w;
        let blocked = |cx: i64| {
            self.label[row + cx as usize].is_some()
                || self.reserved[row + cx as usize]
                || self.quad(cx as usize, y as usize).0 != 0
        };
        if (x..x + n).any(blocked) {
            return false;
        }
        for (k, ch) in s.chars().enumerate() {
            self.label[row + x as usize + k] = Some((ch, col));
        }
        true
    }

    /// The four block pixels of a cell (top-left, top-right, bottom-left, bottom-right)
    /// and a bit mask of which are set.
    fn quad(&self, x: usize, y: usize) -> (u8, [Option<Rgb>; 4]) {
        let top = 2 * y * 2 * self.w + 2 * x;
        let bot = top + 2 * self.w;
        let q = [
            self.px[top],
            self.px[top + 1],
            self.px[bot],
            self.px[bot + 1],
        ]
        .map(|p| p.map(|(c, _)| c));
        let mask = q
            .iter()
            .enumerate()
            .fold(0u8, |m, (k, p)| m | ((p.is_some() as u8) << k));
        (mask, q)
    }

    pub fn cell(&self, x: usize, y: usize) -> Cell {
        let i = y * self.w + x;
        let (mask, q) = self.quad(x, y);
        if mask == 15 {
            // fully covered: split the cell along whichever axis keeps more detail
            let [tl, tr, bl, br] = q.map(|c| c.unwrap_or_default());
            let (top, bot) = (tl.mix(tr, 0.5), bl.mix(br, 0.5));
            let (left, right) = (tl.mix(bl, 0.5), tr.mix(br, 0.5));
            let diff = |a: Rgb, b: Rgb| (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs();
            return if diff(left, right) > diff(top, bot) * 1.3 {
                Cell {
                    ch: '▌',
                    fg: Some(left),
                    bg: Some(right),
                }
            } else {
                Cell {
                    ch: '▀',
                    fg: Some(top),
                    bg: Some(bot),
                }
            };
        }
        if mask != 0 {
            // an edge: quadrant glyphs trace the outline at twice the horizontal resolution,
            // over the terminal's own background
            const QUADS: [char; 16] = [
                ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
            ];
            let set: Vec<Rgb> = q.iter().flatten().copied().collect();
            let k = 1.0 / set.len() as f64;
            let fg = set.iter().fold(Rgb::BLACK, |a, c| a.add(c.scale(k)));
            return Cell {
                ch: QUADS[mask as usize],
                fg: Some(fg),
                bg: None,
            };
        }
        if let Some((ch, col)) = self.label[i] {
            return Cell {
                ch,
                fg: Some(col),
                bg: None,
            };
        }
        if self.dots[i] != 0 {
            let ch = char::from_u32(0x2800 + self.dots[i] as u32).unwrap_or(' ');
            return Cell {
                ch,
                fg: Some(self.dot_col[i].0),
                bg: None,
            };
        }
        if let Some((ch, col, _)) = self.glyph[i] {
            return Cell {
                ch,
                fg: Some(col),
                bg: None,
            };
        }
        Cell::EMPTY
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub target: V3,
    pub dist: f64,
    pub az: f64,
    pub el: f64,
}

/// Perspective view in pixel space: x across 0..w, y down 0..2h (pixels are about square).
pub struct View {
    eye: V3,
    right: V3,
    up: V3,
    fwd: V3,
    f: f64,
    cx: f64,
    cy: f64,
}

const FOV_DEG: f64 = 50.0;

impl View {
    pub fn new(cam: &Camera, h_px: f64, cx: f64) -> View {
        let (se, ce) = cam.el.sin_cos();
        let (sa, ca) = cam.az.sin_cos();
        let eye = cam.target + V3::new(ce * ca, ce * sa, se) * cam.dist;
        let fwd = (cam.target - eye).norm();
        let right = fwd.cross(V3::Z).norm();
        let up = right.cross(fwd);
        let f = (h_px * 0.5) / (FOV_DEG.to_radians() * 0.5).tan();
        View {
            eye,
            right,
            up,
            fwd,
            f,
            cx,
            cy: h_px * 0.5,
        }
    }

    /// Pixel position and depth of a world point, if in front of the camera.
    pub fn project(&self, p: V3) -> Option<(f64, f64, f64)> {
        let d = p - self.eye;
        let z = d.dot(self.fwd);
        if z < 0.05 {
            return None;
        }
        Some((
            self.cx + d.dot(self.right) / z * self.f,
            self.cy - d.dot(self.up) / z * self.f,
            z,
        ))
    }

    /// Pixel position of a direction at infinity (stars).
    fn project_dir(&self, d: V3) -> Option<(f64, f64)> {
        let z = d.dot(self.fwd);
        if z < 0.15 {
            return None;
        }
        Some((
            self.cx + d.dot(self.right) / z * self.f,
            self.cy - d.dot(self.up) / z * self.f,
        ))
    }
}

/// Distances are compressed logarithmically so Mercury and Neptune fit on one screen.
pub fn display_r(au: f64) -> f64 {
    (1.0 + au / 0.25).ln()
}

pub fn to_display(p: V3) -> V3 {
    let r = p.len();
    if r < 1e-9 {
        p
    } else {
        p * (display_r(r) / r)
    }
}

/// Display radius of a body; sizes are exaggerated and compressed so all of them are visible.
pub fn body_radius(index: usize, bodies: &[Body]) -> f64 {
    if index == 0 {
        0.30
    } else {
        0.035 + 0.045 * (bodies[index].radius_km / 1000.0).log10()
    }
}

pub fn body_position(index: usize, jd: f64) -> V3 {
    if index == 0 {
        V3::ZERO
    } else {
        to_display(ephemeris::position(index - 1, jd))
    }
}

/// Body-fixed axes in the ecliptic frame: z along the pole, x towards the pole's ascending node
/// on the ecliptic (for Earth that is the vernal equinox, which sidereal time is measured from).
fn body_axes(pole: V3) -> (V3, V3, V3) {
    let x = pole.cross(V3::Z);
    let x = if x.len() < 1e-6 {
        V3::new(1.0, 0.0, 0.0)
    } else {
        x.norm()
    };
    (x, pole.cross(x), pole)
}

pub fn has_rings(id: &str) -> bool {
    matches!(id, "saturn" | "uranus")
}

/// Ring structure in planet radii: (normal optical depth, albedo, colour).
fn ring_at(id: &str, r: f64) -> Option<(f64, f64, Rgb)> {
    match id {
        "saturn" => {
            // Cassini-based ring profile: face-on opacity turned into optical depth
            let (col, alpha) = textures::saturn_ring(r)?;
            Some((-(1.0 - alpha.min(0.97)).ln(), 1.0, col))
        }
        "uranus" => {
            const RINGLETS: [f64; 4] = [1.67, 1.75, 1.84, 2.00];
            RINGLETS.iter().any(|c| (r - c).abs() < 0.012).then_some((
                0.5,
                0.35,
                Rgb::hex(0x8fa6b0),
            ))
        }
        _ => None,
    }
}

fn lon_lat(n: V3) -> (f64, f64) {
    (n.y.atan2(n.x), n.z.clamp(-1.0, 1.0).asin())
}

/// Fraction of Earth's sky covered by cloud at a point; the layer drifts slowly eastwards.
fn earth_clouds(lon: f64, lat: f64, t: f64) -> f64 {
    // the map is very cloudy; thin it so the continents show through
    textures::earth_clouds()
        .sample(lon - t * 0.004, lat)
        .r
        .powf(1.7)
        * 0.85
}

/// Surface colour at body-fixed unit vector `n` (z = north pole), from the real maps.
fn surface(id: &str, n: V3, t: f64) -> Rgb {
    let (lon, lat) = lon_lat(n);
    match id {
        "earth" => textures::earth_day()
            .sample(lon, lat)
            .mix(Rgb::hex(0xf4f6fb), earth_clouds(lon, lat, t)),
        // Venus' cloud tops circle the planet in about four days: let them drift
        "venus" => textures::venus().sample(lon + t * 0.02, lat),
        _ => textures::for_body(id).map_or(Rgb::WHITE, |m| m.sample(lon, lat)),
    }
}

/// Greenwich mean sidereal time in radians, so Earth's real geography faces the right way.
fn gmst(jd: f64) -> f64 {
    (280.460_618_37 + 360.985_647_366_29 * (jd - J2000)).to_radians()
}

struct Star {
    dir: V3,
    bright: f64,
    col: Rgb,
    phase: f64,
    speed: f64,
}

#[derive(Clone, Copy)]
struct Particle {
    a: f64,
    e: f64,
    incl: f64,
    node: f64,
    peri: f64,
    m0: f64,
    shade: f64,
}

impl Particle {
    fn position(&self, jd: f64) -> V3 {
        let n = TAU / (365.256 * self.a.powf(1.5));
        let m = self.m0 + n * (jd - J2000);
        let ea = m + self.e * m.sin();
        let p = V3::new(
            self.a * (ea.cos() - self.e),
            self.a * (1.0 - self.e * self.e).sqrt() * ea.sin(),
            0.0,
        );
        p.rot_z(self.peri).rot_x(self.incl).rot_z(self.node)
    }
}

/// Where a body was drawn, for mouse picking (cell coordinates).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub body: usize,
    pub x: f64,
    pub y: f64,
    pub r: f64,
}

pub struct Frame<'a> {
    pub bodies: &'a [Body],
    pub jd: f64,
    /// Date driving planet spin; it never runs faster than an hour per second.
    pub spin_jd: f64,
    pub t: f64,
    pub cam: Camera,
    pub selected: usize,
    pub focused: Option<usize>,
    /// Horizontal centre of the view in cells (shifted left when the info panel is open).
    pub center_x: f64,
    /// Cell size in pixels when bodies are drawn as images (HD mode).
    pub hd: Option<(u32, u32)>,
    /// What each image slot last showed, so slow-changing images need not be redrawn every frame.
    pub sent: Option<&'a std::collections::HashMap<u32, Sent>>,
}

/// Size of an image slot's last picture and when it was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub w: u32,
    pub h: u32,
    pub t: f64,
}

pub struct Scene {
    stars: Vec<Star>,
    milky: Vec<(V3, f64)>,
    belt: Vec<Particle>,
    kuiper: Vec<Particle>,
}

/// Galactic north pole and centre in ecliptic coordinates.
const GAL_POLE: V3 = V3::new(-0.8676, 0.0, 0.4972);
const GAL_CENTER: V3 = V3::new(-0.0548, -0.9940, -0.0958);

fn particles(
    seed: i32,
    count: i32,
    a: (f64, f64),
    e_max: f64,
    incl_max: f64,
    shade: (f64, f64),
) -> Vec<Particle> {
    (0..count)
        .map(|k| Particle {
            a: a.0 + (a.1 - a.0) * hashf(k, 1, seed),
            e: e_max * hashf(k, 2, seed),
            incl: incl_max * hashf(k, 3, seed).powi(2),
            node: TAU * hashf(k, 4, seed),
            peri: TAU * hashf(k, 5, seed),
            m0: TAU * hashf(k, 6, seed),
            shade: shade.0 + (shade.1 - shade.0) * hashf(k, 7, seed),
        })
        .collect()
}

impl Scene {
    pub fn new() -> Scene {
        let palette = [
            (0.10, Rgb::hex(0xaac4ff)),
            (0.45, Rgb::hex(0xf2f4ff)),
            (0.75, Rgb::hex(0xfff0d0)),
            (0.90, Rgb::hex(0xffd2a0)),
            (1.01, Rgb::hex(0xffb090)),
        ];
        let stars = (0..2600)
            .map(|k| {
                let z = hashf(k, 1, 11) * 2.0 - 1.0;
                let a = hashf(k, 2, 11) * TAU;
                let r = (1.0 - z * z).sqrt();
                let pick = hashf(k, 3, 11);
                Star {
                    dir: V3::new(r * a.cos(), r * a.sin(), z),
                    bright: hashf(k, 4, 11).powi(4),
                    col: palette
                        .iter()
                        .find(|(p, _)| pick < *p)
                        .map(|x| x.1)
                        .unwrap_or(Rgb::WHITE),
                    phase: hashf(k, 5, 11) * TAU,
                    speed: 0.6 + 2.4 * hashf(k, 6, 11),
                }
            })
            .collect();
        // Milky Way: points scattered around the galactic plane, denser towards the centre
        let e1 = GAL_CENTER;
        let e2 = GAL_POLE.cross(e1);
        let milky = (0..9000)
            .map(|k| {
                let lon = hashf(k, 1, 21) * TAU;
                let g = (hashf(k, 2, 21) + hashf(k, 3, 21) + hashf(k, 4, 21) - 1.5) * 0.16;
                let dir =
                    (e1 * (lon.cos() * g.cos()) + e2 * (lon.sin() * g.cos()) + GAL_POLE * g.sin())
                        .norm();
                let toward_center = 0.35 + 0.65 * (0.5 + 0.5 * lon.cos()).powi(2);
                let dust = smoothstep(0.35, 0.65, fbm(dir * 6.0, 3));
                (dir, toward_center * (0.4 + 0.6 * dust))
            })
            .collect();
        Scene {
            stars,
            milky,
            belt: particles(31, 1100, (2.1, 3.3), 0.12, 0.2, (0.45, 1.0)),
            kuiper: particles(41, 650, (30.0, 50.0), 0.1, 0.25, (0.3, 0.7)),
        }
    }

    pub fn render(&self, cv: &mut Canvas, fr: &Frame) -> (Vec<Hit>, Vec<Sprite>) {
        cv.clear();
        let h = cv.h as f64;
        let view = View::new(&fr.cam, 2.0 * h, fr.center_x);
        self.sky(cv, &view, fr.t);

        let sun_proj = view.project(V3::ZERO).filter(|_| fr.hd.is_none());
        if let Some((sx, sy, sd)) = sun_proj {
            sun_glow(
                cv,
                sx,
                sy,
                (body_radius(0, fr.bodies) / sd * view.f).max(4.0),
                fr.t,
            );
        }
        self.orbits(cv, &view, fr);
        for (list, col, strength) in [
            (&self.belt, Rgb::hex(0xa89a86), 0.45),
            (&self.kuiper, Rgb::hex(0x6f7d9c), 0.28),
        ] {
            for p in list {
                if let Some((x, y, d)) = view.project(to_display(p.position(fr.jd))) {
                    let s = strength * p.shade * depth_fade(d, fr.cam.dist);
                    cv.dot(
                        (x * 2.0) as i64,
                        (y * 2.0) as i64,
                        col.scale(0.6 + 0.4 * p.shade),
                        s,
                    );
                }
            }
        }

        let mut hits = Vec::new();
        let mut order: Vec<(usize, V3, f64)> = (0..fr.bodies.len())
            .filter_map(|i| {
                let p = body_position(i, fr.jd);
                view.project(p).map(|(_, _, d)| (i, p, d))
            })
            .collect();
        order.sort_by(|a, b| b.2.total_cmp(&a.2));
        // while flying to a body, the others would loom far too large at this scale:
        // hide the ones between us and it, and shrink the rest
        let focus_depth = fr
            .focused
            .and_then(|f| order.iter().find(|o| o.0 == f).map(|o| o.2));
        if let (Some(f), Some(fd)) = (fr.focused, focus_depth) {
            order.retain(|o| o.0 == f || o.0 == 0 || o.2 > fd * 1.05);
        }
        let mut sprites = Vec::new();
        // `order` runs from far to near, so the running count is the stacking order: each body
        // sits above everything farther away (one image per body, so occlusion is per body)
        for (rank, &(i, pos, _)) in order.iter().enumerate() {
            let Some(g) = body_geom(&view, fr, i, pos) else {
                continue;
            };
            match fr.hd {
                Some(cell) => {
                    cv.reserve(g.cx, g.cy * 0.5, g.sr, g.sr * 0.5);
                    sprites.extend(body_sprite(&g, i as u32, cell, fr.sent).map(|s| Sprite {
                        z: 1 + rank as i32,
                        ..s
                    }));
                }
                None => draw_body(cv, &g),
            }
            hits.push(Hit {
                body: i,
                x: g.cx,
                y: g.cy * 0.5,
                r: g.sr * if g.has_ring { 1.6 } else { 1.0 },
            });
        }
        for hit in &hits {
            let body = &fr.bodies[hit.body];
            let sel = hit.body == fr.selected;
            if sel && fr.focused != Some(hit.body) {
                // pulsing selection ring
                let pulse = 0.55 + 0.45 * (fr.t * 3.0).sin();
                let rr = (hit.r * 2.0 + 3.0) * 1.0;
                let n = (rr * 8.0).max(24.0) as i64;
                for k in 0..n {
                    let a = k as f64 / n as f64 * TAU;
                    cv.dot(
                        (hit.x * 2.0 + a.cos() * rr) as i64,
                        (hit.y * 4.0 + a.sin() * rr) as i64,
                        body.rgb(),
                        pulse,
                    );
                }
            }
            if fr.focused == Some(hit.body) {
                continue;
            }
            let (col, text) = if sel {
                (body.rgb().mix(Rgb::WHITE, 0.3), format!("▸ {}", body.name))
            } else {
                (Rgb::hex(0x8088a8), body.name.clone())
            };
            let lx = (hit.x + hit.r + if sel { 2.5 } else { 1.5 }).round() as i64;
            let ly = hit.y.floor() as i64;
            if !cv.text(lx, ly, &text, col) {
                let n = text.chars().count() as i64;
                cv.text((hit.x - hit.r).round() as i64 - n - 1, ly, &text, col);
            }
        }
        (hits, sprites)
    }

    fn sky(&self, cv: &mut Canvas, view: &View, t: f64) {
        for &(dir, wgt) in &self.milky {
            if let Some((x, y)) = view.project_dir(dir) {
                let (cx, cy) = (x.floor() as i64, (y * 0.5).floor() as i64);
                if cx >= 0 && cy >= 0 && (cx as usize) < cv.w && (cy as usize) < cv.h {
                    cv.haze[cy as usize * cv.w + cx as usize] += wgt;
                }
            }
        }
        for i in 0..cv.haze.len() {
            let v = cv.haze[i];
            if v > 0.55 {
                let k = smoothstep(0.55, 3.5, v);
                let ch = [' ', '.', '·', ':'][(1.0 + k * 2.99) as usize];
                let col = Rgb::hex(0x2a2f4c).mix(Rgb::hex(0x6a6f9c), k);
                cv.glyph((i % cv.w) as i64, (i / cv.w) as i64, ch, col, 0.1 + k * 0.2);
            }
        }
        for s in &self.stars {
            if let Some((x, y)) = view.project_dir(s.dir) {
                let tw = 0.65 + 0.35 * (t * s.speed + s.phase).sin();
                let b = s.bright * tw;
                if b < 0.04 {
                    continue;
                }
                let ch = if b > 0.7 {
                    '*'
                } else if b > 0.42 {
                    '+'
                } else if b > 0.16 {
                    '·'
                } else {
                    '.'
                };
                cv.glyph(
                    x.floor() as i64,
                    (y * 0.5).floor() as i64,
                    ch,
                    s.col.scale(0.35 + 0.65 * b),
                    0.3 + b,
                );
            }
        }
    }

    fn orbits(&self, cv: &mut Canvas, view: &View, fr: &Frame) {
        for i in 1..fr.bodies.len() {
            let orbit = ephemeris::orbit(i - 1, fr.jd);
            let sel = i == fr.selected;
            let base = Rgb::hex(0x39406a).mix(fr.bodies[i].rgb(), if sel { 0.85 } else { 0.25 });
            let strength = if sel { 1.0 } else { 0.55 };
            let n = 360;
            let mut prev: Option<(f64, f64)> = None;
            for k in 0..=n {
                let p = to_display(orbit.position_at(k as f64 / n as f64 * TAU));
                match view.project(p) {
                    Some((x, y, d)) => {
                        let pt = (x * 2.0, y * 2.0);
                        let fade = depth_fade(d, fr.cam.dist);
                        if let Some(a) = prev {
                            cv.line(a, pt, base.scale(0.45 + 0.55 * fade), strength * fade);
                        }
                        prev = Some(pt);
                    }
                    None => prev = None,
                }
            }
        }
    }
}

/// Things far behind the focus point fade a little, giving the orbits depth.
fn depth_fade(depth: f64, cam_dist: f64) -> f64 {
    (1.25 - 0.45 * (depth / cam_dist)).clamp(0.35, 1.0)
}

/// 4x4 Bayer thresholds for dithering soft light into braille dots.
const BAYER: [[f64; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];

/// Corona: a flickering halo dithered into braille dots, so it stays light and transparent.
fn sun_glow(cv: &mut Canvas, sx: f64, sy: f64, sr: f64, t: f64) {
    // the halo grows with the Sun but never floods the screen when flying close
    let reach = sr + (sr * 2.0).min(26.0);
    let (bx, by) = (sx * 2.0, sy * 2.0);
    let (r_in, r_out) = (sr * 2.0, reach * 2.0);
    for y in (by - r_out).floor() as i64..=(by + r_out).ceil() as i64 {
        for x in (bx - r_out).floor() as i64..=(bx + r_out).ceil() as i64 {
            let (dx, dy) = (x as f64 + 0.5 - bx, y as f64 + 0.5 - by);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r_in || d >= r_out {
                continue;
            }
            let ang = dy.atan2(dx);
            let rays =
                0.65 + 0.35 * vnoise(ang.cos() * 4.0 + 7.0, ang.sin() * 4.0, t * 0.45 - d * 0.02);
            let g = ((r_out - d) / (r_out - r_in)).powf(2.4) * rays;
            let threshold =
                (BAYER[(y.rem_euclid(4)) as usize][(x.rem_euclid(4)) as usize] + 0.5) / 16.0;
            if g > threshold * 0.9 {
                let col = Rgb::hex(0xff7a2a).mix(Rgb::hex(0xffe2a0), g);
                cv.dot(x, y, col.scale(0.45 + 0.55 * g), 0.2 + g);
            }
        }
    }
}

/// Everything needed to shade one body for the current view. Both back ends use it, so the
/// text renderer and the HD (pixel) renderer show exactly the same planet.
pub struct BodyGeom<'a> {
    pub i: usize,
    id: &'a str,
    body: &'a Body,
    /// Centre in view units (x in cells, y in half rows) and on-screen radius in the same units.
    pub cx: f64,
    pub cy: f64,
    pub sr: f64,
    depth: f64,
    radius: f64,
    ambient: f64,
    /// 0 keeps only the broad colours (small text-mode discs), 1 keeps all surface detail.
    detail: f64,
    has_ring: bool,
    /// Pixel back end: photographic lighting instead of the text renderer's readable one.
    hd: bool,
    axis: V3,
    axes: (V3, V3, V3),
    spin: f64,
    sun_dir: V3,
    ax_c: V3,
    l_dot_a: f64,
    lit_face: bool,
    right: V3,
    up: V3,
    fwd: V3,
    t: f64,
}

fn body_geom<'a>(view: &View, fr: &Frame<'a>, i: usize, pos: V3) -> Option<BodyGeom<'a>> {
    let body = &fr.bodies[i];
    let (cx, cy, depth) = view.project(pos)?;
    let shrink = if fr.focused.is_some_and(|f| f != i) && i != 0 {
        0.3
    } else {
        1.0
    };
    let radius = body_radius(i, fr.bodies) * shrink;
    let proj_r = radius / depth * view.f;
    // keep tiny planets visible as at least a small disc, scaled to the screen size
    let screen = (view.cy / 55.0).clamp(0.6, 1.3);
    let sr = proj_r.max(screen * if i == 0 { 5.0 } else { 1.8 + 15.0 * radius });
    // Small discs are lit almost fully so their colour reads at a glance. Big ones show the
    // real phase, but their night side keeps a trace of colour instead of going black, which
    // would look like a hole on terminals with a coloured background.
    let ambient = 0.10 + 0.5 * (1.0 - smoothstep(3.5, 10.0, sr));
    let id = body.id.as_str();
    // in text mode, rings only once they are big enough to read; tiny ones are dark specks
    let min_ring = match (id, fr.hd.is_some()) {
        ("uranus", false) => 10.0,
        ("uranus", true) => 5.0,
        (_, false) => 2.5,
        (_, true) => 1.2,
    };
    let has_ring = has_rings(id) && sr >= min_ring;
    let axis = body.pole();
    let sun_dir = if i == 0 { V3::Z } else { (-pos).norm() };
    // the spin phase; Earth uses real sidereal time so its geography is correct
    let spin = if id == "earth" {
        gmst(fr.spin_jd)
    } else if i == 0 {
        // the real 25-day rotation is far too slow to see, so the Sun also turns once
        // every 70 seconds of animation
        TAU * (fr.t / 70.0 + fr.spin_jd * 24.0 / body.rotation_hours).rem_euclid(1.0)
    } else {
        TAU * (fr.spin_jd * 24.0 / body.rotation_hours).rem_euclid(1.0)
    };
    let ax_c = V3::new(axis.dot(view.right), axis.dot(view.up), -axis.dot(view.fwd));
    let l_dot_a = sun_dir.dot(axis);
    let detail = if fr.hd.is_some() {
        1.0
    } else {
        smoothstep(4.0, 12.0, sr)
    };
    Some(BodyGeom {
        i,
        id,
        body,
        cx,
        cy,
        sr,
        depth,
        radius,
        ambient,
        detail,
        has_ring,
        hd: fr.hd.is_some(),
        axis,
        axes: body_axes(axis),
        spin,
        sun_dir,
        ax_c,
        l_dot_a,
        lit_face: (l_dot_a > 0.0) == (ax_c.z > 0.0),
        right: view.right,
        up: view.up,
        fwd: view.fwd,
        t: fr.t,
    })
}

impl BodyGeom<'_> {
    /// How far from the centre anything is drawn, in planet radii.
    fn reach(&self) -> f64 {
        if self.i == 0 {
            1.0 + (1.6f64).min(26.0 / self.sr)
        } else if self.has_ring {
            2.3
        } else {
            1.08
        }
    }

    fn to_body(&self, n: V3) -> V3 {
        let (x, y, z) = self.axes;
        V3::new(n.dot(x), n.dot(y), n.dot(z)).rot_z(-self.spin)
    }

    fn atmosphere(&self) -> Option<Rgb> {
        match self.id {
            "earth" => Some(Rgb::hex(0x7ab0ff)),
            "venus" => Some(Rgb::hex(0xffe8b0)),
            "uranus" | "neptune" => Some(Rgb::hex(0xa0e0ff)),
            _ => None,
        }
    }

    /// Surface colour at disc coordinates (u, v) with u² + v² <= 1; also returns the depth
    /// of the surface towards the viewer (1 at the centre, 0 at the rim).
    fn planet(&self, u: f64, v: f64) -> (Rgb, f64) {
        let wp = (1.0 - u * u - v * v).max(0.0).sqrt();
        let n = self.right * u + self.up * v - self.fwd * wp;
        let nb = self.to_body(n);
        if self.i == 0 {
            return (self.sun_surface(nb, wp), wp);
        }
        let (lon, lat) = lon_lat(nb);
        let cloud = if self.id == "earth" {
            earth_clouds(lon, lat, self.t)
        } else {
            0.0
        };
        let mut tex = surface(self.id, nb, self.t);
        if self.id == "earth" && !self.hd {
            // the Blue Marble oceans are very dark; lift them so Earth reads as blue in text
            let water = smoothstep(0.05, 0.12, tex.b - tex.r.max(tex.g) * 0.8) * (1.0 - cloud);
            tex = Rgb::new(tex.r.powf(0.8), tex.g.powf(0.8), tex.b.powf(0.8))
                .mix(Rgb::hex(0x2a5fc8), water * 0.55);
        }
        let tex = tex.mix(self.body.rgb(), 0.75 * (1.0 - self.detail));
        let ndl = n.dot(self.sun_dir);
        let mut shade = 1.0;
        if self.has_ring && self.l_dot_a.abs() > 1e-3 {
            let s = -n.dot(self.axis) / self.l_dot_a;
            if s > 0.0 {
                if let Some((tau, _, _)) = ring_at(self.id, (n + self.sun_dir * s).len()) {
                    shade = 0.25 + 0.75 * (-tau / self.l_dot_a.abs()).exp();
                }
            }
        }
        if !self.hd {
            let lit = (ndl * 1.15 + 0.05).clamp(0.0, 1.0).powf(0.85) * shade;
            let mut c = tex.scale(self.ambient + (1.0 - self.ambient) * lit);
            if let Some(a) = self.atmosphere() {
                c = c.add(a.scale((1.0 - wp).powf(2.5) * (ndl + 0.3).clamp(0.0, 1.0) * 0.5));
            }
            return (c, wp);
        }
        // HD: photographic lighting, a soft terminator and a nearly black night side
        let lit = ((ndl + 0.04) / 1.04).clamp(0.0, 1.0).powf(0.85) * shade;
        let limb = match self.id {
            "jupiter" | "saturn" | "uranus" | "neptune" => 0.55 + 0.45 * wp.powf(0.5),
            "venus" => 0.7 + 0.3 * wp.powf(0.5),
            _ => 1.0,
        };
        // a little light on the night side of small discs so they keep their shape
        let ambient = 0.04 + 0.24 * (1.0 - smoothstep(4.0, 14.0, self.sr));
        let mut c = tex.scale(ambient + (1.0 - ambient) * lit * limb);
        if self.id == "earth" {
            let ground = textures::earth_day().sample(lon, lat);
            let water =
                smoothstep(0.02, 0.08, ground.b - ground.r.max(ground.g) * 0.9) * (1.0 - cloud);
            // sun glint on the oceans
            let half = (self.sun_dir - self.fwd).norm();
            let glint = n.dot(half).max(0.0).powf(70.0) * water * smoothstep(0.0, 0.2, ndl);
            c = c.add(Rgb::hex(0xfff2dc).scale(glint * 0.8));
            // city lights on the night side, dimmed by cloud
            let night = 1.0 - smoothstep(-0.2, 0.04, ndl);
            if night > 0.0 {
                let lights = textures::earth_night().sample(lon, lat);
                c = c.add(
                    Rgb::new(lights.r, lights.g * 0.85, lights.b * 0.6)
                        .scale(night * (1.0 - 0.8 * cloud) * 1.3),
                );
            }
        }
        if let Some(a) = self.atmosphere() {
            c = c.add(a.scale((1.0 - wp).powf(4.0) * smoothstep(-0.25, 0.45, ndl) * 0.75));
        }
        (c, wp)
    }

    /// The photosphere: the real map turning, boiling granulation and a dark, red limb.
    fn sun_surface(&self, nb: V3, wp: f64) -> Rgb {
        let (lon, lat) = lon_lat(nb);
        let base = textures::sun().sample(lon, lat);
        let boil = vnoise(nb.x * 38.0, nb.y * 38.0, nb.z * 38.0 + self.t * 0.7);
        let limb = 0.32 + 0.68 * wp.powf(0.55);
        // white-hot in the middle, warming to orange at the limb, as the eye sees it
        let core = base.mix(Rgb::hex(0xfff4dc), 0.72 * wp.powf(0.7));
        core.scale(1.15)
            .mix(Rgb::hex(0xff8a2c), (1.0 - wp).powi(2) * 0.5)
            .scale(limb * (0.9 + 0.18 * boil))
    }

    /// Ring colour and opacity at (u, v), and whether that bit of ring is in front of the
    /// planet's surface depth `wp` (pass None where the planet is not drawn).
    fn ring(&self, u: f64, v: f64, wp: Option<f64>) -> Option<(Rgb, f64, bool)> {
        if !self.has_ring || self.ax_c.z.abs() <= 0.01 {
            return None;
        }
        let wr = -(self.ax_c.x * u + self.ax_c.y * v) / self.ax_c.z;
        let r = (u * u + v * v + wr * wr).sqrt();
        let (tau, alb, rc) = ring_at(self.id, r)?;
        let alpha = 1.0 - (-tau / self.ax_c.z.abs().max(0.03)).exp();
        let p = self.right * u + self.up * v - self.fwd * wr;
        let qs = p.dot(self.sun_dir);
        let shadow = if qs < 0.0 && p.dot(p) - qs * qs < 1.0 {
            0.08
        } else {
            1.0
        };
        let light = if self.lit_face {
            0.42 + 0.58 * self.l_dot_a.abs().sqrt()
        } else {
            0.12 + 1.4 * tau * (-2.5 * tau).exp()
        };
        // at icon size the ring is lit generously so it reads as Saturn's ring,
        // not as two dark specks beside the planet
        let light = light.max(0.95 * (1.0 - smoothstep(5.0, 9.0, self.sr)));
        Some((
            rc.scale(alb * light * shadow),
            alpha,
            wp.is_none_or(|w| wr > w),
        ))
    }

    /// Sun corona at distance `d` (in solar radii) outside the disc: a bright thin rim
    /// fading into faint, slowly shifting streamers.
    fn corona(&self, u: f64, v: f64, d: f64) -> (Rgb, f64) {
        let x = (d - 1.0).max(0.0);
        let ang = v.atan2(u);
        let streamers = 0.55 + 0.45 * vnoise(ang.cos() * 3.0 + 7.0, ang.sin() * 3.0, self.t * 0.12);
        let inner = (-x * 10.0).exp() * 0.8;
        let outer = (-x * 2.4).exp() * 0.3 * streamers;
        let edge = 1.0 - smoothstep(0.75, 1.0, x / (self.reach() - 1.0));
        let col = Rgb::hex(0xfff3da).mix(Rgb::hex(0xffc27a), smoothstep(0.0, 0.7, x));
        (col, ((inner + outer) * edge).min(1.0))
    }
}

/// Text back end: write the body into the canvas' block pixels.
fn draw_body(cv: &mut Canvas, g: &BodyGeom) {
    let reach = g.sr * if g.has_ring { 2.3 } else { 1.0 } + 1.0;
    // pixels are half a cell wide: x counts in half cells, y in half rows
    let (x0, x1) = (
        (2.0 * (g.cx - reach)).floor() as i64,
        (2.0 * (g.cx + reach)).ceil() as i64,
    );
    let (y0, y1) = ((g.cy - reach).floor() as i64, (g.cy + reach).ceil() as i64);
    let sr = g.sr;
    let (inner, outer) = ((1.0 - 1.2 / sr).max(0.0).powi(2), (1.0 + 1.2 / sr).powi(2));
    for py in y0..=y1 {
        for px in x0..=x1 {
            let mut u = ((px as f64 + 0.5) * 0.5 - g.cx) / sr;
            let mut v = -(py as f64 + 0.5 - g.cy) / sr;
            let c2 = u * u + v * v;
            // 4x4 coverage test near the rim so small discs come out round
            let on_planet = if c2 < inner {
                true
            } else if c2 > outer {
                false
            } else {
                let mut inside = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let su = ((px as f64 + (sx as f64 + 0.5) / 4.0) * 0.5 - g.cx) / sr;
                        let sv = -(py as f64 + (sy as f64 + 0.5) / 4.0 - g.cy) / sr;
                        inside += (su * su + sv * sv <= 1.0) as u32;
                    }
                }
                inside >= 8
            };
            if on_planet && u * u + v * v > 0.97 {
                // an edge pixel whose centre falls just outside: shade it at the rim
                let k = (0.97 / (u * u + v * v)).sqrt();
                u *= k;
                v *= k;
            }
            let planet = on_planet.then(|| g.planet(u, v));
            let mut col = planet.map(|p| p.0);
            if let Some((rc, alpha, front)) = g.ring(u, v, planet.map(|p| p.1)) {
                if front {
                    col = match col {
                        Some(c) => Some(c.mix(rc, alpha)),
                        // over empty space, skip faint or very dark ring pixels: they read
                        // as dirt on terminals that are not pure black
                        None if alpha > 0.12 && rc.scale(alpha.sqrt()).luma() > 0.07 => {
                            Some(rc.scale(alpha.sqrt()))
                        }
                        None => None,
                    };
                }
            }
            if let Some(c) = col {
                let wp = planet.map_or(0.0, |p| p.1);
                cv.pixel(px, py, c, g.depth - wp * g.radius);
            }
        }
    }
}

/// An RGBA image drawn over the text by the graphics back end, positioned to the pixel.
pub struct Sprite {
    /// Identifies what it shows across frames (body index, or a panel slot).
    pub slot: u32,
    /// Top-left corner in screen pixels, relative to the canvas.
    pub x: f64,
    pub y: f64,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// The picture has not changed enough to redraw: just move the one on screen.
    pub keep: bool,
    /// Stacking order among images: nearer bodies get higher values.
    pub z: i32,
}

/// Images bigger than this are shaded at half resolution and smoothly upscaled, which keeps
/// very close fly-bys fast; at that size the difference is hard to see.
const HALF_RES_PIXELS: u64 = 600_000;

/// HD back end: render the body at true pixel resolution with anti-aliased edges, a soft
/// corona for the Sun and a thin atmosphere halo for planets that have one.
fn body_sprite(
    g: &BodyGeom,
    slot: u32,
    cell: (u32, u32),
    sent: Option<&std::collections::HashMap<u32, Sent>>,
) -> Option<Sprite> {
    let (cw, ch) = (cell.0 as f64, cell.1 as f64);
    // centre in screen pixels; the radius uses the horizontal metric so the disc stays round
    let (x, y) = (g.cx * cw, g.cy * ch * 0.5);
    let r_px = g.sr * cw;
    let ext = r_px * g.reach() + 1.0;
    let (x0, y0) = ((x - ext).floor(), (y - ext).floor());
    let size = (2.0 * ext).ceil() as u32 + 1;
    let px = size as u64 * size as u64;
    // Motion comes from moving the image every frame, which costs almost nothing. What it
    // shows (spin, phase, the corona's flicker) changes slowly, so it is redrawn less often,
    // and the bigger it is the less often.
    let interval = if slot == CARD_SLOT || px > 150_000 {
        1.0 / 8.0
    } else if g.i == 0 {
        1.0 / 12.0
    } else if px > 20_000 {
        1.0 / 10.0
    } else {
        1.0 / 15.0
    };
    if let Some(prev) = sent.and_then(|m| m.get(&slot)) {
        if prev.w.abs_diff(size) <= 1 && g.t - prev.t < interval {
            return Some(Sprite {
                slot,
                x: x0,
                y: y0,
                w: prev.w,
                h: prev.h,
                rgba: Vec::new(),
                keep: true,
                z: 1,
            });
        }
    }
    let half = px > HALF_RES_PIXELS;
    let (step, rs) = if half {
        (2.0, size.div_ceil(2))
    } else {
        (1.0, size)
    };
    let atmo = g.atmosphere();
    let mut rgba = vec![0u8; (rs * rs * 4) as usize];
    par_rows(&mut rgba, rs as usize, |iy, line| {
        for ix in 0..rs as usize {
            let px = x0 + (ix as f64 + 0.5) * step;
            let py = y0 + (iy as f64 + 0.5) * step;
            let (u, v) = ((px - x) / r_px, -(py - y) / r_px);
            let d = (u * u + v * v).sqrt();
            // planet coverage, supersampled near the rim for a clean edge
            let edge = 1.5 * step / r_px;
            let cov = if d < 1.0 - edge {
                1.0
            } else if d > 1.0 + edge {
                0.0
            } else {
                let mut n = 0;
                for k in 0..16 {
                    let (a, b) = ((k % 4) as f64 + 0.5, (k / 4) as f64 + 0.5);
                    let su = (x0 + (ix as f64 + a / 4.0) * step - x) / r_px;
                    let sv = -(y0 + (iy as f64 + b / 4.0) * step - y) / r_px;
                    n += (su * su + sv * sv <= 1.0) as u32;
                }
                n as f64 / 16.0
            };
            let mut out = (Rgb::BLACK, 0.0);
            // behind the planet: corona, halo, or the far side of the rings
            if d >= 1.0 - edge {
                if g.i == 0 {
                    out = g.corona(u, v, d.max(1.0));
                } else if let Some(a) = atmo {
                    let k = ((d - 1.0) / 0.06).clamp(0.0, 1.0);
                    let rim = V3::new(u, v, 0.0) * (1.0 / d.max(1e-9));
                    let lit =
                        smoothstep(-0.3, 0.5, (g.right * rim.x + g.up * rim.y).dot(g.sun_dir));
                    out = (a, (1.0 - k).powi(2) * 0.5 * lit);
                }
            }
            let (uc, vc) = if d > 0.995 {
                (u * 0.995 / d, v * 0.995 / d)
            } else {
                (u, v)
            };
            let planet = (cov > 0.0).then(|| g.planet(uc, vc));
            let ring = g.ring(u, v, planet.map(|p| p.1));
            if let Some((rc, ra, false)) = ring {
                out = over((rc, ra), out);
            }
            if let Some((pc, _)) = planet {
                out = over((pc, cov), out);
            }
            if let Some((rc, ra, true)) = ring {
                out = over((rc, ra), out);
            }
            let (r, gg, b) = out.0.to_u8();
            let k = ix * 4;
            line[k..k + 4].copy_from_slice(&[
                r,
                gg,
                b,
                (out.1.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ]);
        }
    });
    if half {
        rgba = upscale2(&rgba, rs as usize, size as usize);
    }
    Some(Sprite {
        slot,
        x: x0,
        y: y0,
        w: size,
        h: size,
        rgba,
        keep: false,
        z: 1,
    })
}

/// Bilinear 2x upscale of a square RGBA image from `n` to `m` pixels a side.
fn upscale2(src: &[u8], n: usize, m: usize) -> Vec<u8> {
    let mut out = vec![0u8; m * m * 4];
    let at = |x: usize, y: usize, c: usize| src[(y.min(n - 1) * n + x.min(n - 1)) * 4 + c] as f64;
    for y in 0..m {
        let fy = ((y as f64 + 0.5) / 2.0 - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        for x in 0..m {
            let fx = ((x as f64 + 0.5) / 2.0 - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            for c in 0..4 {
                let top = at(x0, y0, c) + (at(x0 + 1, y0, c) - at(x0, y0, c)) * tx;
                let bot = at(x0, y0 + 1, c) + (at(x0 + 1, y0 + 1, c) - at(x0, y0 + 1, c)) * tx;
                out[(y * m + x) * 4 + c] = (top + (bot - top) * ty + 0.5) as u8;
            }
        }
    }
    out
}

/// Porter-Duff "over" for straight (non-premultiplied) colour and alpha.
fn over(top: (Rgb, f64), below: (Rgb, f64)) -> (Rgb, f64) {
    let a = top.1 + below.1 * (1.0 - top.1);
    if a <= 1e-6 {
        return (Rgb::BLACK, 0.0);
    }
    let c = top
        .0
        .scale(top.1)
        .add(below.0.scale(below.1 * (1.0 - top.1)))
        .scale(1.0 / a);
    (c, a)
}

/// Fill an RGBA buffer row by row on all CPU cores.
fn par_rows(buf: &mut [u8], w: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    let stride = w * 4;
    if stride == 0 {
        return;
    }
    let rows = buf.len() / stride;
    // threads only pay off for big images; small ones are cheaper on this thread
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(buf.len() / (4 * 40_000))
        .clamp(1, 8);
    if threads == 1 {
        for (j, line) in buf.chunks_mut(stride).enumerate() {
            f(j, line);
        }
        return;
    }
    let per = rows.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (k, chunk) in buf.chunks_mut(per * stride).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, line) in chunk.chunks_mut(stride).enumerate() {
                    f(k * per + j, line);
                }
            });
        }
    });
}

/// A close-up of one body, lit from its true Sun direction, for the info panel.
pub fn render_card(cv: &mut Canvas, scene: &Scene, fr: &Frame, i: usize) -> Option<Sprite> {
    cv.clear();
    let pos = body_position(i, fr.jd);
    let radius = body_radius(i, fr.bodies);
    let sun_dir = if i == 0 {
        V3::new(1.0, 0.0, 0.0)
    } else {
        (-pos).norm()
    };
    // view from 55 degrees off the Sun direction for a gibbous phase, drifting slowly
    let v = (sun_dir.rot_z(0.95 + 0.25 * (fr.t * 0.05).sin()) + V3::Z * 0.35).norm();
    let h_px = 2.0 * cv.h as f64;
    let f = (h_px * 0.5) / (FOV_DEG.to_radians() * 0.5).tan();
    let ringed = has_rings(&fr.bodies[i].id) && fr.bodies[i].id == "saturn";
    let want = h_px
        * if ringed {
            0.19
        } else if i == 0 {
            0.30
        } else {
            0.40
        };
    let cam = Camera {
        target: pos,
        dist: radius * f / want,
        az: v.y.atan2(v.x),
        el: v.z.asin(),
    };
    let view = View::new(&cam, h_px, cv.w as f64 * 0.5);
    let card = Frame {
        cam,
        focused: None,
        ..*fr
    };
    scene.sky(cv, &view, fr.t);
    let g = body_geom(&view, &card, i, pos)?;
    match fr.hd {
        Some(cell) => body_sprite(&g, CARD_SLOT, cell, fr.sent),
        None => {
            if i == 0 {
                sun_glow(cv, view.cx, view.cy, want, fr.t);
            }
            draw_body(cv, &g);
            None
        }
    }
}

/// Sprite slot used by the close-up in the info panel.
pub const CARD_SLOT: u32 = 64;

/// Top-down map of the orbits with the bodies, highlighting `i`; shown while flying to a body.
pub fn render_minimap(cv: &mut Canvas, fr: &Frame, i: usize) {
    cv.clear();
    let (bw, bh) = (2.0 * cv.w as f64, 4.0 * cv.h as f64);
    let scale = (bw.min(bh) * 0.5 - 2.0) / display_r(31.0);
    let (cx, cy) = (bw * 0.5, bh * 0.5);
    let map = |p: V3| (cx + p.x * scale, cy - p.y * scale);
    for k in 1..fr.bodies.len() {
        let orbit = ephemeris::orbit(k - 1, fr.jd);
        let col = Rgb::hex(0x39406a).mix(fr.bodies[k].rgb(), if k == i { 0.9 } else { 0.2 });
        let mut prev = None;
        for s in 0..=180 {
            let q = map(to_display(orbit.position_at(s as f64 / 180.0 * TAU)));
            if let Some(a) = prev {
                cv.line(a, q, col, if k == i { 1.0 } else { 0.5 });
            }
            prev = Some(q);
        }
    }
    for k in 0..fr.bodies.len() {
        let (x, y) = map(body_position(k, fr.jd));
        let col = fr.bodies[k].rgb();
        let r = if k == 0 {
            2.0
        } else if k == i {
            1.5
        } else {
            0.8
        };
        for dy in -2i64..=2 {
            for dx in -2i64..=2 {
                if ((dx * dx + dy * dy) as f64).sqrt() <= r {
                    cv.dot(x as i64 + dx, y as i64 + dy, col, 2.0);
                }
            }
        }
    }
    let (x, y) = map(body_position(i, fr.jd));
    let name = &fr.bodies[i].name;
    let lx = (x / 2.0) as i64 + 2;
    let ly = (y / 4.0) as i64;
    if !cv.text(lx, ly, name, fr.bodies[i].rgb()) {
        cv.text(
            (x / 2.0) as i64 - name.chars().count() as i64 - 1,
            ly,
            name,
            fr.bodies[i].rgb(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Latitude and longitude (degrees) of the point on Earth with the Sun overhead.
    fn subsolar(jd: f64) -> (f64, f64) {
        let earth = &crate::data::load()[3];
        let (x, y, z) = body_axes(earth.pole());
        let s = (-ephemeris::position(2, jd)).norm();
        let nb = V3::new(s.dot(x), s.dot(y), s.dot(z)).rot_z(-gmst(jd));
        (nb.z.asin().to_degrees(), nb.y.atan2(nb.x).to_degrees())
    }

    #[test]
    fn earth_faces_the_sun_correctly() {
        // June solstice 2026, 12:00 UTC: Sun overhead near 23.4 N, close to Greenwich
        let (lat, lon) = subsolar(crate::time::parse_date("2026-06-21T12:00").unwrap());
        assert!((lat - 23.44).abs() < 0.3, "lat {lat}");
        assert!(lon.abs() < 2.5, "lon {lon}");
        // 00:00 UTC: the Sun is over the Pacific near the date line
        let (_, lon) = subsolar(crate::time::parse_date("2026-06-21T00:00").unwrap());
        assert!((lon.abs() - 180.0).abs() < 2.5, "lon {lon}");
    }

    #[test]
    fn nearer_bodies_are_stacked_on_top_in_hd() {
        let bodies = crate::data::load();
        let cam = Camera {
            target: V3::ZERO,
            dist: 8.8,
            az: -1.2,
            el: 0.5,
        };
        let fr = Frame {
            bodies: &bodies,
            jd: crate::time::parse_date("2026-10-22").unwrap(),
            spin_jd: 0.0,
            t: 0.0,
            cam,
            selected: 3,
            focused: None,
            center_x: 80.0,
            hd: Some((10, 20)),
            sent: None,
        };
        let mut cv = Canvas::new(160, 50);
        let (_, sprites) = Scene::new().render(&mut cv, &fr);
        let view = View::new(&cam, 100.0, 80.0);
        let depth = |slot: u32| view.project(body_position(slot as usize, fr.jd)).unwrap().2;
        assert!(sprites.len() >= 5);
        for a in &sprites {
            for b in &sprites {
                if depth(a.slot) < depth(b.slot) {
                    assert!(
                        a.z > b.z,
                        "body {} is nearer than {} but stacked below it",
                        a.slot,
                        b.slot
                    );
                }
            }
        }
    }

    #[test]
    fn saturn_pole_is_tilted_about_27_degrees() {
        let saturn = &crate::data::load()[6];
        let tilt = saturn.pole().dot(V3::Z).acos().to_degrees();
        assert!((tilt - 28.0).abs() < 1.5, "{tilt}");
    }
}
