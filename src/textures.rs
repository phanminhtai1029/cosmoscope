//! Surface maps, embedded as JPEG and decoded once, the first time they are needed.
//!
//! Earth's day side is NASA's Blue Marble (public domain). The other maps, Earth's night
//! lights and clouds, and Saturn's ring profile are from Solar System Scope
//! (https://www.solarsystemscope.com/textures/), CC BY 4.0, based on NASA mission data.

use std::sync::OnceLock;

use crate::math::Rgb;

pub struct Map {
    w: usize,
    h: usize,
    channels: usize,
    data: Vec<u8>,
}

impl Map {
    fn decode(bytes: &[u8]) -> Map {
        let mut dec = jpeg_decoder::Decoder::new(bytes);
        let data = dec.decode().expect("embedded texture is a valid JPEG");
        let info = dec.info().expect("decoded JPEG has info");
        let channels = data.len() / (info.width as usize * info.height as usize);
        Map {
            w: info.width as usize,
            h: info.height as usize,
            channels,
            data,
        }
    }

    fn texel(&self, x: i64, y: i64) -> [f64; 3] {
        let xi = x.rem_euclid(self.w as i64) as usize;
        let yi = y.clamp(0, self.h as i64 - 1) as usize;
        let k = (yi * self.w + xi) * self.channels;
        if self.channels >= 3 {
            [
                self.data[k] as f64,
                self.data[k + 1] as f64,
                self.data[k + 2] as f64,
            ]
        } else {
            let v = self.data[k] as f64;
            [v, v, v]
        }
    }

    /// Bilinear sample at east longitude / latitude in radians (longitude 0 is the map centre).
    pub fn sample(&self, lon: f64, lat: f64) -> Rgb {
        let u = (lon / std::f64::consts::TAU + 0.5).rem_euclid(1.0) * self.w as f64 - 0.5;
        let v = (0.5 - lat / std::f64::consts::PI) * self.h as f64 - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (
            self.texel(x0, y0),
            self.texel(x0 + 1, y0),
            self.texel(x0, y0 + 1),
            self.texel(x0 + 1, y0 + 1),
        );
        let mix = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            (top + (bot - top) * fy) / 255.0
        };
        Rgb::new(mix(0), mix(1), mix(2))
    }
}

macro_rules! maps {
    ($($name:ident => $file:literal),* $(,)?) => {
        $(
            pub fn $name() -> &'static Map {
                static MAP: OnceLock<Map> = OnceLock::new();
                MAP.get_or_init(|| Map::decode(include_bytes!(concat!("../assets/textures/", $file))))
            }
        )*
        /// Decode every map on a background thread so the first frames do not stall.
        pub fn warm_up() {
            std::thread::spawn(|| {
                $( let _ = $name(); )*
            });
        }
    };
}

maps! {
    sun => "sun.jpg",
    mercury => "mercury.jpg",
    venus => "venus.jpg",
    earth_day => "earth_day.jpg",
    earth_night => "earth_night.jpg",
    earth_clouds => "earth_clouds.jpg",
    mars => "mars.jpg",
    jupiter => "jupiter.jpg",
    saturn => "saturn.jpg",
    uranus => "uranus.jpg",
    neptune => "neptune.jpg",
}

/// The map for a body's surface, if it has one of its own.
pub fn for_body(id: &str) -> Option<&'static Map> {
    Some(match id {
        "sun" => sun(),
        "mercury" => mercury(),
        "venus" => venus(),
        "mars" => mars(),
        "jupiter" => jupiter(),
        "saturn" => saturn(),
        "uranus" => uranus(),
        "neptune" => neptune(),
        _ => return None,
    })
}

const RING: &[u8] = include_bytes!("../assets/textures/saturn_ring.rgba");
/// The ring profile spans these radii (in Saturn radii), calibrated on the outer edges of the
/// B and A rings; the C ring, Encke gap and F ring then fall where they really are.
const RING_R0: f64 = 1.166;
const RING_DR: f64 = 0.002_288;

/// Colour and opacity (seen face-on) of Saturn's rings at radius `r`, in planet radii.
pub fn saturn_ring(r: f64) -> Option<(Rgb, f64)> {
    let n = RING.len() / 4;
    let x = (r - RING_R0) / RING_DR - 0.5;
    if x < 0.0 || x >= (n - 1) as f64 {
        return None;
    }
    let (i, f) = (x.floor() as usize, x - x.floor());
    let at = |k: usize, c: usize| RING[k * 4 + c] as f64 / 255.0;
    let lerp = |c: usize| at(i, c) + (at(i + 1, c) - at(i, c)) * f;
    let alpha = lerp(3);
    (alpha > 0.01).then(|| (Rgb::new(lerp(0), lerp(1), lerp(2)), alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_decode_and_sample() {
        for id in [
            "sun", "mercury", "venus", "mars", "jupiter", "saturn", "uranus", "neptune",
        ] {
            let c = for_body(id).unwrap().sample(0.3, 0.2);
            assert!(c.r + c.g + c.b > 0.05, "{id} sampled black");
        }
        // the Atlantic west of Africa is darker and bluer than the Sahara
        let ocean = earth_day().sample((-25f64).to_radians(), 15f64.to_radians());
        let desert = earth_day().sample(10f64.to_radians(), 23f64.to_radians());
        assert!(
            ocean.b > ocean.r && desert.r > desert.b,
            "{ocean:?} {desert:?}"
        );
    }

    #[test]
    fn ring_profile_has_the_cassini_division() {
        let b_ring = saturn_ring(1.80).unwrap().1;
        let cassini = saturn_ring(1.99).map_or(0.0, |x| x.1);
        let a_ring = saturn_ring(2.10).unwrap().1;
        assert!(
            b_ring > 0.8 && a_ring > 0.4 && cassini < 0.5,
            "{b_ring} {cassini} {a_ring}"
        );
        assert!(saturn_ring(1.0).is_none() && saturn_ring(2.5).is_none());
    }
}
