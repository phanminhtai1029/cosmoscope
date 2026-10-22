//! Software renderer. The scene is drawn into a cell grid with three layers:
//! block "pixels" (2x2 per cell, shown with half-block and quadrant glyphs) for the Sun and
//! planets, braille dots (2x4 per cell)
//! for orbits and small particles, and plain glyphs for the sky. Empty cells keep the
//! terminal's own background, so transparent terminals stay transparent.

use crate::data::{Body, EARTH_MAP, EARTH_MAP_H, EARTH_MAP_W};
use crate::ephemeris;
use crate::math::{fbm, hashf, smoothstep, vnoise, Rgb, TAU, V3};
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
            self.label[row + cx as usize].is_some() || self.quad(cx as usize, y as usize).0 != 0
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

/// Ring structure in planet radii: (optical depth, albedo, colour).
fn ring_at(id: &str, r: f64) -> Option<(f64, f64, Rgb)> {
    match id {
        "saturn" => {
            if !(1.24..=2.27).contains(&r) {
                return None;
            }
            let fine = vnoise(r * 18.0, 0.5, 0.5) * 0.6 + vnoise(r * 45.0, 1.5, 0.5) * 0.4;
            Some(if r < 1.53 {
                (0.10 * (0.5 + fine), 0.45, Rgb::hex(0x8a7d6c))
            } else if r < 1.95 {
                let inner = smoothstep(1.53, 1.65, r);
                (
                    (0.4 + 1.4 * inner) * (0.8 + 0.4 * fine),
                    (0.55 + 0.4 * inner) * (0.85 + 0.3 * fine),
                    Rgb::hex(0xe0d0ae).mix(Rgb::hex(0xf2e6c8), fine),
                )
            } else if r < 2.03 {
                (0.06, 0.35, Rgb::hex(0x5a5248))
            } else if (2.205..2.215).contains(&r) {
                (0.01, 0.3, Rgb::hex(0x5a5248))
            } else {
                (
                    0.55 * (0.75 + 0.5 * fine),
                    0.7 * (0.85 + 0.3 * fine),
                    Rgb::hex(0xc8b898),
                )
            })
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

fn earth_map(lon: f64, lat: f64) -> Rgb {
    let u = ((lon / TAU + 0.5).rem_euclid(1.0)) * EARTH_MAP_W as f64 - 0.5;
    let v = ((0.5 - lat / std::f64::consts::PI).clamp(0.0, 0.9999)) * EARTH_MAP_H as f64 - 0.5;
    let (x0, y0) = (u.floor(), v.floor().max(0.0));
    let (fx, fy) = (u - x0, (v - y0).clamp(0.0, 1.0));
    let at = |x: f64, y: f64| {
        let xi = (x as i64).rem_euclid(EARTH_MAP_W as i64) as usize;
        let yi = (y as usize).min(EARTH_MAP_H - 1);
        let k = (yi * EARTH_MAP_W + xi) * 3;
        Rgb::new(
            EARTH_MAP[k] as f64 / 255.0,
            EARTH_MAP[k + 1] as f64 / 255.0,
            EARTH_MAP[k + 2] as f64 / 255.0,
        )
    };
    let top = at(x0, y0).mix(at(x0 + 1.0, y0), fx);
    let bot = at(x0, y0 + 1.0).mix(at(x0 + 1.0, y0 + 1.0), fx);
    top.mix(bot, fy)
}

/// Surface colour at body-fixed unit vector `n` (z = north pole).
fn surface(id: &str, n: V3, t: f64) -> Rgb {
    let lat = n.z.clamp(-1.0, 1.0).asin();
    let lon = n.y.atan2(n.x);
    match id {
        "mercury" => {
            let craters = fbm(n * 7.0, 4);
            Rgb::hex(0x8f857c).mix(Rgb::hex(0xc4bbb2), craters * 1.4 - 0.3)
        }
        "venus" => {
            let swirl = fbm(V3::new(lon * 0.8 + lat * 2.0, lat * 5.0, t * 0.01), 3);
            Rgb::hex(0xd8b878).mix(Rgb::hex(0xf6e7c0), swirl)
        }
        "earth" => {
            // the Blue Marble oceans are very dark; lift them so Earth reads as blue at any size
            let g = earth_map(lon, lat);
            let water = smoothstep(0.05, 0.12, g.b - g.r.max(g.g) * 0.8);
            let ground = Rgb::new(g.r.powf(0.8), g.g.powf(0.8), g.b.powf(0.8))
                .mix(Rgb::hex(0x2a5fc8), water * 0.55);
            let clouds = fbm(V3::new(n.x * 3.0 + t * 0.004, n.y * 3.0, n.z * 5.0), 4);
            ground.mix(Rgb::hex(0xf4f6fb), smoothstep(0.55, 0.75, clouds) * 0.85)
        }
        "mars" => {
            let dark = fbm(n * 3.0, 4);
            let base = Rgb::hex(0xd0663e).mix(Rgb::hex(0x7a3a24), smoothstep(0.5, 0.7, dark));
            base.mix(Rgb::hex(0xf2eee8), smoothstep(0.88, 0.93, n.z.abs()))
        }
        "jupiter" => {
            let turb = fbm(V3::new(lon * 1.6, lat * 9.0, 1.0), 4);
            let band = (lat * 13.0 + turb * 1.6).sin();
            let mut c = Rgb::hex(0xb8875c).mix(Rgb::hex(0xefe2c4), band * 0.5 + 0.5);
            c = c.mix(Rgb::hex(0x8a5c3c), smoothstep(0.75, 0.95, -band) * 0.5);
            // Great Red Spot near 22 degrees south
            let (dl, dp) = (
                (lon - 1.0 + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0,
                lat + 0.38,
            );
            let spot = (dl / 0.32).powi(2) + (dp / 0.11).powi(2);
            c.mix(Rgb::hex(0xc85a40), 1.0 - smoothstep(0.6, 1.0, spot))
        }
        "saturn" => {
            let turb = fbm(V3::new(lon * 1.2, lat * 7.0, 3.0), 3);
            let band = (lat * 10.0 + turb).sin();
            Rgb::hex(0xd6bd86).mix(Rgb::hex(0xf2e3b8), band * 0.5 + 0.5)
        }
        "uranus" => {
            let band = (lat * 6.0).sin() * 0.5 + 0.5;
            Rgb::hex(0x9adfe4).mix(Rgb::hex(0xb8eef0), band * 0.4)
        }
        "neptune" => {
            let turb = fbm(V3::new(lon * 2.0, lat * 8.0, 5.0), 3);
            let mut c =
                Rgb::hex(0x3f5fd8).mix(Rgb::hex(0x6688f2), ((lat * 9.0 + turb).sin()) * 0.5 + 0.5);
            let (dl, dp) = (
                (lon + 0.6 + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0,
                lat + 0.35,
            );
            c = c.mix(
                Rgb::hex(0x22337a),
                1.0 - smoothstep(0.5, 1.0, (dl / 0.3).powi(2) + (dp / 0.12).powi(2)),
            );
            c.mix(Rgb::WHITE, smoothstep(0.72, 0.8, turb) * 0.7)
        }
        _ => Rgb::WHITE,
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

    pub fn render(&self, cv: &mut Canvas, fr: &Frame) -> Vec<Hit> {
        cv.clear();
        let h = cv.h as f64;
        let view = View::new(&fr.cam, 2.0 * h, fr.center_x);
        self.sky(cv, &view, fr.t);

        let sun_proj = view.project(V3::ZERO);
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
        for &(i, pos, _) in &order {
            if let Some(hit) = draw_body(cv, &view, fr, i, pos) {
                hits.push(hit);
            }
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
        hits
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

fn draw_body(cv: &mut Canvas, view: &View, fr: &Frame, i: usize, pos: V3) -> Option<Hit> {
    let body = &fr.bodies[i];
    let (pcx, pcy, depth) = view.project(pos)?;
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
    // rings only once they are big enough to read; tiny ones are just dark specks
    let has_ring = has_rings(id) && sr >= if id == "uranus" { 10.0 } else { 2.5 };
    let reach = if has_ring { sr * 2.3 } else { sr } + 1.0;
    let axis = body.pole();
    let (bx, by, bz) = body_axes(axis);
    let sun_dir = (-pos).norm();
    // the spin phase; Earth uses real sidereal time so its geography is correct
    let spin = if id == "earth" {
        gmst(fr.spin_jd)
    } else {
        TAU * (fr.spin_jd * 24.0 / body.rotation_hours).rem_euclid(1.0)
    };
    let to_body = |n: V3| V3::new(n.dot(bx), n.dot(by), n.dot(bz)).rot_z(-spin);
    let ax_c = V3::new(axis.dot(view.right), axis.dot(view.up), -axis.dot(view.fwd));
    let l_dot_a = sun_dir.dot(axis);
    let lit_face = (l_dot_a > 0.0) == (ax_c.z > 0.0);

    // pixels are half a cell wide: x counts in half cells, y in half rows
    let (x0, x1) = (
        (2.0 * (pcx - reach)).floor() as i64,
        (2.0 * (pcx + reach)).ceil() as i64,
    );
    let (y0, y1) = ((pcy - reach).floor() as i64, (pcy + reach).ceil() as i64);
    let (inner, outer) = ((1.0 - 1.2 / sr).max(0.0).powi(2), (1.0 + 1.2 / sr).powi(2));
    for py in y0..=y1 {
        for px in x0..=x1 {
            let mut u = ((px as f64 + 0.5) * 0.5 - pcx) / sr;
            let mut v = -(py as f64 + 0.5 - pcy) / sr;
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
                        let su = ((px as f64 + (sx as f64 + 0.5) / 4.0) * 0.5 - pcx) / sr;
                        let sv = -(py as f64 + (sy as f64 + 0.5) / 4.0 - pcy) / sr;
                        inside += (su * su + sv * sv <= 1.0) as u32;
                    }
                }
                inside >= 8
            };
            let mut d2 = u * u + v * v;
            if on_planet && d2 > 0.97 {
                // an edge pixel whose centre falls just outside: shade it at the rim
                let k = (0.97 / d2).sqrt();
                u *= k;
                v *= k;
                d2 = 0.97;
            }
            let wp = if on_planet { (1.0 - d2).sqrt() } else { -1e9 };
            let mut col: Option<Rgb> = None;
            if on_planet {
                let n = view.right * u + view.up * v - view.fwd * wp;
                col = Some(if i == 0 {
                    let nb = to_body(n);
                    let gran = fbm(V3::new(nb.x * 9.0, nb.y * 9.0, nb.z * 9.0 + fr.t * 0.12), 3);
                    Rgb::hex(0xff9a3c)
                        .mix(Rgb::hex(0xfff3d0), wp.powf(0.45))
                        .scale(0.86 + 0.22 * gran)
                } else {
                    // small discs keep only their broad colours; fine texture there reads as noise
                    let tex = surface(id, to_body(n), fr.t)
                        .mix(body.rgb(), 0.75 * (1.0 - smoothstep(4.0, 12.0, sr)));
                    let ndl = n.dot(sun_dir);
                    let mut lit = (ndl * 1.15 + 0.05).clamp(0.0, 1.0).powf(0.85);
                    if has_ring && l_dot_a.abs() > 1e-3 {
                        let s = -n.dot(axis) / l_dot_a;
                        if s > 0.0 {
                            if let Some((tau, _, _)) = ring_at(id, (n + sun_dir * s).len()) {
                                lit *= 0.25 + 0.75 * (-tau / l_dot_a.abs()).exp();
                            }
                        }
                    }
                    let atmo = match id {
                        "earth" => Some(Rgb::hex(0x7ab0ff)),
                        "venus" => Some(Rgb::hex(0xffe8b0)),
                        "uranus" | "neptune" => Some(Rgb::hex(0xa0e0ff)),
                        _ => None,
                    };
                    let mut c = tex.scale(ambient + (1.0 - ambient) * lit);
                    if let Some(a) = atmo {
                        c = c
                            .add(a.scale((1.0 - wp).powf(2.5) * (ndl + 0.3).clamp(0.0, 1.0) * 0.5));
                    }
                    c
                });
            }
            if has_ring && ax_c.z.abs() > 0.01 {
                let wr = -(ax_c.x * u + ax_c.y * v) / ax_c.z;
                if !on_planet || wr > wp {
                    let r = (d2 + wr * wr).sqrt();
                    if let Some((tau, alb, rc)) = ring_at(id, r) {
                        let alpha = 1.0 - (-tau / ax_c.z.abs().max(0.03)).exp();
                        let p = view.right * u + view.up * v - view.fwd * wr;
                        let qs = p.dot(sun_dir);
                        let shadow = if qs < 0.0 && p.dot(p) - qs * qs < 1.0 {
                            0.08
                        } else {
                            1.0
                        };
                        let light = if lit_face {
                            0.42 + 0.58 * l_dot_a.abs().sqrt()
                        } else {
                            0.12 + 1.4 * tau * (-2.5 * tau).exp()
                        };
                        // at icon size the ring is lit generously so it reads as Saturn's ring,
                        // not as two dark specks beside the planet
                        let light = light.max(0.95 * (1.0 - smoothstep(5.0, 9.0, sr)));
                        let ring_col = rc.scale(alb * light * shadow);
                        col = match col {
                            Some(c) => Some(c.mix(ring_col, alpha)),
                            // over empty space, skip faint or very dark ring pixels: they read
                            // as dirt on terminals that are not pure black
                            None if alpha > 0.12 && ring_col.scale(alpha.sqrt()).luma() > 0.07 => {
                                Some(ring_col.scale(alpha.sqrt()))
                            }
                            None => None,
                        };
                    }
                }
            }
            if let Some(c) = col {
                cv.pixel(px, py, c, depth - wp.max(0.0) * radius);
            }
        }
    }
    Some(Hit {
        body: i,
        x: pcx,
        y: pcy * 0.5,
        r: sr * if has_ring { 1.6 } else { 1.0 },
    })
}

/// A close-up of one body, lit from its true Sun direction, for the info panel.
pub fn render_card(cv: &mut Canvas, scene: &Scene, fr: &Frame, i: usize) {
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
    let card = Frame { cam, ..*fr };
    scene.sky(cv, &view, fr.t);
    if i == 0 {
        sun_glow(cv, view.cx, view.cy, want, fr.t);
    }
    draw_body(cv, &view, &card, i, pos);
}

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
    fn saturn_pole_is_tilted_about_27_degrees() {
        let saturn = &crate::data::load()[6];
        let tilt = saturn.pole().dot(V3::Z).acos().to_degrees();
        assert!((tilt - 28.0).abs() < 1.5, "{tilt}");
    }
}
