//! Julian dates and calendar conversion (Gregorian, after Meeus).

use std::time::{SystemTime, UNIX_EPOCH};

pub const J2000: f64 = 2_451_545.0;

pub fn jd_now() -> f64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    secs / 86_400.0 + 2_440_587.5
}

pub fn jd_from_calendar(year: i32, month: u32, day: f64) -> f64 {
    let (mut y, mut m) = (year as f64, month as f64);
    if m <= 2.0 {
        y -= 1.0;
        m += 12.0;
    }
    let a = (y / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y + 4716.0)).floor() + (30.6001 * (m + 1.0)).floor() + day + b - 1524.5
}

/// (year, month, day, hour, minute) for a Julian date.
pub fn calendar_from_jd(jd: f64) -> (i32, u32, u32, u32, u32) {
    let jd = jd + 0.5 / 1440.0; // round to the nearest minute
    let z = (jd + 0.5).floor();
    let f = jd + 0.5 - z;
    let a = if z < 2_299_161.0 {
        z
    } else {
        let alpha = ((z - 1_867_216.25) / 36_524.25).floor();
        z + 1.0 + alpha - (alpha / 4.0).floor()
    };
    let b = a + 1524.0;
    let c = ((b - 122.1) / 365.25).floor();
    let d = (365.25 * c).floor();
    let e = ((b - d) / 30.6001).floor();
    let day = b - d - (30.6001 * e).floor();
    let month = if e < 14.0 { e - 1.0 } else { e - 13.0 };
    let year = if month > 2.0 { c - 4716.0 } else { c - 4715.0 };
    let minutes = (f * 1440.0).floor() as u32;
    (
        year as i32,
        month as u32,
        day as u32,
        minutes / 60,
        minutes % 60,
    )
}

pub fn format_jd(jd: f64) -> String {
    let (y, mo, d, h, mi) = calendar_from_jd(jd);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

pub fn year_of(jd: f64) -> f64 {
    2000.0 + (jd - J2000) / 365.25
}

/// Parse "YYYY-MM-DD" or "YYYY-MM-DDTHH:MM" (UTC).
pub fn parse_date(s: &str) -> Result<f64, String> {
    let bad = || format!("invalid date '{s}', expected YYYY-MM-DD or YYYY-MM-DDTHH:MM");
    let (date, clock) = match s.split_once(['T', ' ']) {
        Some((d, c)) => (d, Some(c)),
        None => (s, None),
    };
    let neg = date.starts_with('-');
    let parts: Vec<&str> = date.trim_start_matches('-').split('-').collect();
    if parts.len() != 3 {
        return Err(bad());
    }
    let mut year: i32 = parts[0].parse().map_err(|_| bad())?;
    if neg {
        year = -year;
    }
    let month: u32 = parts[1].parse().map_err(|_| bad())?;
    let day: u32 = parts[2].parse().map_err(|_| bad())?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    let mut frac = 0.0;
    if let Some(c) = clock {
        let (h, m) = c.split_once(':').ok_or_else(bad)?;
        let (h, m): (u32, u32) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
        if h > 23 || m > 59 {
            return Err(bad());
        }
        frac = (h as f64 + m as f64 / 60.0) / 24.0;
    }
    Ok(jd_from_calendar(year, month, day as f64 + frac))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn j2000_round_trip() {
        assert_eq!(jd_from_calendar(2000, 1, 1.5), J2000);
        assert_eq!(calendar_from_jd(J2000), (2000, 1, 1, 12, 0));
        assert_eq!(calendar_from_jd(2_461_335.5), (2026, 10, 22, 0, 0));
    }

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("2026-10-22").unwrap(), 2_461_335.5);
        assert_eq!(parse_date("2000-01-01T12:00").unwrap(), J2000);
        assert!(parse_date("2026-13-01").is_err());
        assert!(parse_date("tomorrow").is_err());
    }
}
