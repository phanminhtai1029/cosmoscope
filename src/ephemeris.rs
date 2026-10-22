//! Planet positions from the JPL "Keplerian Elements for Approximate Positions of the
//! Major Planets" (E. M. Standish, Table 1, valid 1800-2050 AD).
//! Positions are heliocentric, J2000 ecliptic frame, in AU.

use crate::math::V3;
use crate::time::J2000;

/// Element value at J2000 and its rate per Julian century:
/// a (AU), e, I (deg), mean longitude L (deg), longitude of perihelion (deg), longitude of node (deg).
struct Elements([[f64; 2]; 6]);

const PLANETS: [Elements; 8] = [
    // Mercury
    Elements([
        [0.387_099_27, 0.000_000_37],
        [0.205_635_93, 0.000_019_06],
        [7.004_979_02, -0.005_947_49],
        [252.250_323_50, 149_472.674_111_75],
        [77.457_796_28, 0.160_476_89],
        [48.330_765_93, -0.125_340_81],
    ]),
    // Venus
    Elements([
        [0.723_335_66, 0.000_003_90],
        [0.006_776_72, -0.000_041_07],
        [3.394_676_05, -0.000_788_90],
        [181.979_099_50, 58_517.815_387_29],
        [131.602_467_18, 0.002_683_29],
        [76.679_842_55, -0.277_694_18],
    ]),
    // Earth-Moon barycentre
    Elements([
        [1.000_002_61, 0.000_005_62],
        [0.016_711_23, -0.000_043_92],
        [-0.000_015_31, -0.012_946_68],
        [100.464_571_66, 35_999.372_449_81],
        [102.937_681_93, 0.323_273_64],
        [0.0, 0.0],
    ]),
    // Mars
    Elements([
        [1.523_710_34, 0.000_018_47],
        [0.093_394_10, 0.000_078_82],
        [1.849_691_42, -0.008_131_31],
        [-4.553_432_05, 19_140.302_684_99],
        [-23.943_629_59, 0.444_410_88],
        [49.559_538_91, -0.292_573_43],
    ]),
    // Jupiter
    Elements([
        [5.202_887_00, -0.000_116_07],
        [0.048_386_24, -0.000_132_53],
        [1.304_396_95, -0.001_837_14],
        [34.396_440_51, 3_034.746_127_75],
        [14.728_479_83, 0.212_526_68],
        [100.473_909_09, 0.204_691_06],
    ]),
    // Saturn
    Elements([
        [9.536_675_94, -0.001_250_60],
        [0.053_861_79, -0.000_509_91],
        [2.485_991_87, 0.001_936_09],
        [49.954_244_23, 1_222.493_622_01],
        [92.598_878_31, -0.418_972_16],
        [113.662_424_48, -0.288_677_94],
    ]),
    // Uranus
    Elements([
        [19.189_164_64, -0.001_961_76],
        [0.047_257_44, -0.000_043_97],
        [0.772_637_83, -0.002_429_39],
        [313.238_104_51, 428.482_027_85],
        [170.954_276_30, 0.408_052_81],
        [74.016_925_03, 0.042_405_89],
    ]),
    // Neptune
    Elements([
        [30.069_922_76, 0.000_262_91],
        [0.008_590_48, 0.000_051_05],
        [1.770_043_47, 0.000_353_72],
        [-55.120_029_69, 218.459_453_25],
        [44.964_762_27, -0.322_414_64],
        [131.784_225_74, -0.005_086_64],
    ]),
];

/// Osculating orbit of one planet at a given date.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub a: f64,
    pub e: f64,
    incl: f64,
    peri: f64, // argument of perihelion, rad
    node: f64, // longitude of ascending node, rad
    /// Mean anomaly in radians, 0..TAU.
    pub mean_anomaly: f64,
}

impl Orbit {
    /// Position for an arbitrary mean anomaly (radians), keeping the orbit's shape.
    pub fn position_at(&self, m: f64) -> V3 {
        let e = self.e;
        let mut ea = m + e * m.sin();
        for _ in 0..8 {
            ea -= (ea - e * ea.sin() - m) / (1.0 - e * ea.cos());
        }
        let xp = self.a * (ea.cos() - e);
        let yp = self.a * (1.0 - e * e).sqrt() * ea.sin();
        let (sw, cw) = self.peri.sin_cos();
        let (so, co) = self.node.sin_cos();
        let (si, ci) = self.incl.sin_cos();
        V3::new(
            (cw * co - sw * so * ci) * xp + (-sw * co - cw * so * ci) * yp,
            (cw * so + sw * co * ci) * xp + (-sw * so + cw * co * ci) * yp,
            (sw * si) * xp + (cw * si) * yp,
        )
    }

    pub fn position(&self) -> V3 {
        self.position_at(self.mean_anomaly)
    }

    #[cfg(test)]
    pub fn period_days(&self) -> f64 {
        365.256_363 * self.a.powf(1.5)
    }
}

/// Orbit of planet `index` (0 = Mercury .. 7 = Neptune) at Julian date `jd`.
pub fn orbit(index: usize, jd: f64) -> Orbit {
    let t = (jd - J2000) / 36_525.0;
    let el = &PLANETS[index].0;
    let v = |k: usize| el[k][0] + el[k][1] * t;
    let (lon, peri_lon, node) = (v(3), v(4), v(5));
    Orbit {
        a: v(0),
        e: v(1),
        incl: v(2).to_radians(),
        peri: (peri_lon - node).to_radians(),
        node: node.to_radians(),
        mean_anomaly: (lon - peri_lon)
            .to_radians()
            .rem_euclid(std::f64::consts::TAU),
    }
}

pub fn position(index: usize, jd: f64) -> V3 {
    orbit(index, jd).position()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Heliocentric ecliptic J2000 vectors (AU) from JPL Horizons (planet system barycentres,
    /// Earth-Moon barycentre for Earth) at JD 2451545.0, 2461335.5 and 2469807.5.
    const HORIZONS: [[[f64; 3]; 3]; 8] = [
        [
            [-0.130_093_6, -0.447_287_6, -0.024_598_3],
            [0.349_043_8, -0.164_337_4, -0.045_443_3],
            [-0.179_514_0, 0.267_811_8, 0.038_348_3],
        ],
        [
            [-0.718_302_3, -0.032_654_3, 0.041_014_2],
            [0.645_278_6, 0.327_890_5, -0.032_726_0],
            [0.141_782_2, -0.713_384_2, -0.018_025_7],
        ],
        [
            [-0.177_158_8, 0.967_219_4, -0.000_001_1],
            [0.876_961_4, 0.470_868_8, -0.000_034_1],
            [-0.171_583_0, 0.968_268_3, -0.000_108_1],
        ],
        [
            [1.390_715_9, -0.013_416_3, -0.034_467_7],
            [-0.155_067_0, 1.575_197_7, 0.036_812_6],
            [-1.543_231_7, -0.503_589_5, 0.027_201_9],
        ],
        [
            [4.001_177_2, 2.938_576_1, -0.101_785_3],
            [-3.610_212_4, 3.898_437_9, 0.064_579_6],
            [-2.391_046_3, 4.664_074_1, 0.033_965_6],
        ],
        [
            [6.406_408_9, 6.569_989_6, -0.369_076_4],
            [9.229_980_7, 1.881_344_2, -0.400_174_5],
            [4.766_225_4, -8.773_719_9, -0.037_876_8],
        ],
        [
            [14.431_856_6, -13.734_321_5, -0.238_141_7],
            [8.856_563_4, 17.302_429_8, -0.050_575_7],
            [-17.823_236_4, 4.071_483_0, 0.245_850_3],
        ],
        [
            [16.812_047_0, -24.991_762_9, 0.127_222_9],
            [29.833_900_0, 1.446_338_9, -0.717_299_0],
            [17.398_198_8, 24.197_784_0, -0.899_241_8],
        ],
    ];
    const DATES: [f64; 3] = [2_451_545.0, 2_461_335.5, 2_469_807.5];

    #[test]
    fn matches_jpl_horizons() {
        for (p, rows) in HORIZONS.iter().enumerate() {
            for (k, h) in rows.iter().enumerate() {
                let want = V3::new(h[0], h[1], h[2]);
                let got = position(p, DATES[k]);
                let angle = (got.dot(want) / (got.len() * want.len()))
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                let radial = (got.len() - want.len()).abs() / want.len();
                // Table 1 is good to well under a degree for 1800-2050; we stay inside 0.25 deg / 0.5%.
                assert!(
                    angle < 0.25,
                    "planet {p} at JD {}: off by {angle:.3} deg",
                    DATES[k]
                );
                assert!(
                    radial < 0.005,
                    "planet {p} at JD {}: distance off by {:.3}%",
                    DATES[k],
                    radial * 100.0
                );
            }
        }
    }

    #[test]
    fn periods_are_sensible() {
        let earth = orbit(2, J2000).period_days();
        assert!((earth - 365.26).abs() < 0.1, "{earth}");
        let neptune = orbit(7, J2000).period_days() / 365.25;
        assert!((neptune - 164.8).abs() < 1.0, "{neptune}");
    }
}
