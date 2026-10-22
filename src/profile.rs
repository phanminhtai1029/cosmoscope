//! Optional frame profiler: set COSMOSCOPE_PROFILE=/path/to/file to log, per frame, how
//! long each phase took (milliseconds). Costs nothing when off.

use std::cell::RefCell;
use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

static FILE: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();

thread_local! {
    static PHASES: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
}

fn file() -> Option<&'static std::sync::Mutex<std::fs::File>> {
    FILE.get_or_init(|| {
        let path = std::env::var("COSMOSCOPE_PROFILE").ok()?;
        std::fs::File::create(path).ok().map(std::sync::Mutex::new)
    })
    .as_ref()
}

/// Run `f`, recording its duration under `name` when profiling is on.
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if file().is_none() {
        return f();
    }
    let t = Instant::now();
    let out = f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    PHASES.with(|p| p.borrow_mut().push((name, ms)));
    out
}

/// Write this frame's phases as one line.
pub fn end_frame() {
    let Some(f) = file() else { return };
    let line: String = PHASES.with(|p| {
        p.borrow_mut()
            .drain(..)
            .map(|(n, ms)| format!("{n}={ms:.3} "))
            .collect()
    });
    if let Ok(mut f) = f.lock() {
        let _ = writeln!(f, "{line}");
    }
}

/// Record a duration measured by hand (for code that is awkward to wrap in a closure).
pub fn since(name: &'static str, t: Instant) {
    if file().is_some() {
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        PHASES.with(|p| p.borrow_mut().push((name, ms)));
    }
}
