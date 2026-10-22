//! Small vector, colour and noise helpers shared by the renderer.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

pub const TAU: f64 = std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub const ZERO: V3 = V3::new(0.0, 0.0, 0.0);
    pub const Z: V3 = V3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f64, y: f64, z: f64) -> V3 {
        V3 { x, y, z }
    }
    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            self
        }
    }
    pub fn lerp(self, o: V3, t: f64) -> V3 {
        self + (o - self) * t
    }
    /// Rotate about the x axis by `a` radians.
    pub fn rot_x(self, a: f64) -> V3 {
        let (s, c) = a.sin_cos();
        V3::new(self.x, self.y * c - self.z * s, self.y * s + self.z * c)
    }
    /// Rotate about the z axis by `a` radians.
    pub fn rot_z(self, a: f64) -> V3 {
        let (s, c) = a.sin_cos();
        V3::new(self.x * c - self.y * s, self.x * s + self.y * c, self.z)
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl AddAssign for V3 {
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, k: f64) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

/// Linear RGB colour with components in 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);
    pub const WHITE: Rgb = Rgb::new(1.0, 1.0, 1.0);

    pub const fn new(r: f64, g: f64, b: f64) -> Rgb {
        Rgb { r, g, b }
    }
    pub const fn hex(h: u32) -> Rgb {
        Rgb::new(
            ((h >> 16) & 255) as f64 / 255.0,
            ((h >> 8) & 255) as f64 / 255.0,
            (h & 255) as f64 / 255.0,
        )
    }
    /// Parse "#rrggbb"; falls back to white.
    pub fn parse(s: &str) -> Rgb {
        u32::from_str_radix(s.trim_start_matches('#'), 16)
            .map(Rgb::hex)
            .unwrap_or(Rgb::WHITE)
    }
    pub fn mix(self, o: Rgb, t: f64) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb::new(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
        )
    }
    pub fn scale(self, k: f64) -> Rgb {
        Rgb::new(self.r * k, self.g * k, self.b * k)
    }
    pub fn add(self, o: Rgb) -> Rgb {
        Rgb::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
    pub fn to_u8(self) -> (u8, u8, u8) {
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        (q(self.r), q(self.g), q(self.b))
    }
}

pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn hash3(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((y as u32).wrapping_mul(668_265_263))
        .wrapping_add((z as u32).wrapping_mul(2_246_822_519));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

pub fn hashf(x: i32, y: i32, z: i32) -> f64 {
    (hash3(x, y, z) & 0xff_ffff) as f64 / 16_777_215.0
}

/// Smooth 3D value noise in 0..1.
pub fn vnoise(x: f64, y: f64, z: f64) -> f64 {
    let (ix, iy, iz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (fx, fy, fz) = (s(x - ix as f64), s(y - iy as f64), s(z - iz as f64));
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let mut c = [[0.0; 2]; 2];
    for (dz, row) in c.iter_mut().enumerate() {
        for (dy, v) in row.iter_mut().enumerate() {
            let (y1, z1) = (iy + dy as i32, iz + dz as i32);
            *v = lerp(hashf(ix, y1, z1), hashf(ix + 1, y1, z1), fx);
        }
    }
    lerp(lerp(c[0][0], c[0][1], fy), lerp(c[1][0], c[1][1], fy), fz)
}

/// Fractal noise: `oct` octaves of value noise, normalised to 0..1.
pub fn fbm(p: V3, oct: u32) -> f64 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 0.5, 0.0, 1.0);
    for _ in 0..oct {
        sum += vnoise(p.x * f, p.y * f, p.z * f) * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}
