//! Body data embedded from assets/bodies.toml.

use serde::Deserialize;

use crate::math::{Rgb, V3};

#[derive(Debug, Clone, Deserialize)]
pub struct Body {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub radius_km: f64,
    pub mass_earths: f64,
    pub gravity: f64,
    pub rotation_hours: f64,
    pub day_hours: f64,
    pub year_days: f64,
    pub mean_temp_c: f64,
    pub moons: u32,
    pub tilt_deg: f64,
    pub pole_ra: f64,
    pub pole_dec: f64,
    pub color: String,
    pub facts: Vec<String>,
}

impl Body {
    pub fn rgb(&self) -> Rgb {
        Rgb::parse(&self.color)
    }

    /// North pole as a unit vector in the J2000 ecliptic frame.
    pub fn pole(&self) -> V3 {
        let (ra, dec) = (self.pole_ra.to_radians(), self.pole_dec.to_radians());
        V3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin())
            .rot_x(-OBLIQUITY.to_radians())
    }
}

/// Obliquity of the ecliptic at J2000, degrees.
pub const OBLIQUITY: f64 = 23.439_291_1;

#[derive(Deserialize)]
struct BodyFile {
    body: Vec<Body>,
}

/// Sun first, then Mercury .. Neptune.
pub fn load() -> Vec<Body> {
    let file: BodyFile =
        toml::from_str(include_str!("../assets/bodies.toml")).expect("assets/bodies.toml is valid");
    file.body
}

#[cfg(test)]
mod tests {
    #[test]
    fn data_loads() {
        let b = super::load();
        assert_eq!(b.len(), 9);
        assert_eq!(b[0].id, "sun");
        assert_eq!(b[3].id, "earth");
        assert!(b.iter().all(|x| x.facts.len() >= 3));
    }
}
