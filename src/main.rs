//! cosmoscope: a living, interactive Solar System in the terminal.

mod app;
mod data;
mod ephemeris;
mod math;
mod render;
mod time;
mod ui;

use std::io::{self, Write};
use std::time::{Duration, Instant};

use clap::Parser;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::style::Color;
use ratatui::{DefaultTerminal, Terminal};

use app::App;

#[derive(Parser)]
#[command(
    name = "cosmoscope",
    version,
    about = "A living, interactive Solar System in your terminal"
)]
struct Args {
    /// Body to fly to on start: sun, mercury, venus, earth, mars, jupiter, saturn, uranus, neptune
    body: Option<String>,
    /// Start at this date (UTC), e.g. 1969-07-20 or 2030-01-01T12:00
    #[arg(long)]
    date: Option<String>,
    /// Time speed: realtime, minute, hour, 6hours, day, week, month, year, decade
    #[arg(long, default_value = "week")]
    speed: String,
    /// Start with time paused
    #[arg(long)]
    paused: bool,
    /// Start with the info panel hidden
    #[arg(long)]
    no_panel: bool,
    /// Keep the camera still instead of slowly circling
    #[arg(long)]
    no_rotate: bool,
    /// Guided tour: fly from body to body on its own (any key takes over); great as a screensaver
    #[arg(long)]
    tour: bool,
    /// Colour mode: auto, truecolor or 256
    #[arg(long, default_value = "auto")]
    colors: String,
    /// Frames per second
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(5..=120))]
    fps: u32,
    /// Render one frame as ANSI text and exit (COLSxROWS); for screenshots and tests
    #[arg(long, hide = true)]
    snapshot: Option<String>,
    /// Seconds of animation to simulate before --snapshot
    #[arg(long, hide = true, default_value_t = 4.0)]
    at: f64,
}

fn detect_truecolor() -> bool {
    if let Ok(v) = std::env::var("COSMOSCOPE_COLORS") {
        return v != "256";
    }
    if std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false)
    {
        return true;
    }
    if cfg!(windows) {
        return true;
    }
    matches!(
        std::env::var("TERM_PROGRAM").as_deref(),
        Ok("iTerm.app") | Ok("WezTerm") | Ok("vscode") | Ok("ghostty")
    )
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("cosmoscope: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    ui::set_truecolor(match args.colors.as_str() {
        "auto" => detect_truecolor(),
        "truecolor" => true,
        "256" => false,
        other => {
            return Err(format!(
                "unknown colour mode '{other}' (use auto, truecolor or 256)"
            ))
        }
    });
    let jd = match &args.date {
        Some(d) => time::parse_date(d)?,
        None => time::jd_now(),
    };
    let mut app = App::new(jd);
    app.speed = app::speed_from_name(&args.speed).ok_or_else(|| {
        format!(
            "unknown speed '{}' (use {})",
            args.speed,
            app::speed_names()
        )
    })?;
    app.paused = args.paused;
    if args.no_panel {
        app.show_panel = Some(false);
    }
    app.auto_rotate = !args.no_rotate;
    if args.tour {
        app.start_tour();
    }
    if let Some(name) = &args.body {
        let i = app
            .body_index(name)
            .ok_or_else(|| format!("unknown body '{name}'"))?;
        app.focus(i);
    }

    if let Some(size) = &args.snapshot {
        return snapshot(&mut app, size, args.at);
    }

    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    let result = event_loop(&mut terminal, &mut app, args.fps);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, fps: u32) -> io::Result<()> {
    let frame = Duration::from_secs_f64(1.0 / fps as f64);
    let mut last = Instant::now();
    while !app.quit {
        let now = Instant::now();
        app.update((now - last).as_secs_f64().min(0.1));
        last = now;
        terminal.draw(|f| ui::draw(f, app))?;
        let deadline = now + frame;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !event::poll(left)? {
                break;
            }
            match event::read()? {
                // Windows also reports key releases; act on presses only
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
            if app.quit {
                break;
            }
        }
    }
    Ok(())
}

/// Simulate `at` seconds at 30 fps, render one frame off-screen and print it as ANSI text.
fn snapshot(app: &mut App, size: &str, at: f64) -> Result<(), String> {
    let (w, h) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
        .ok_or("snapshot size must look like 160x48")?;
    let steps = (at * 30.0).round() as usize;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).map_err(|e| e.to_string())?;
    for _ in 0..steps {
        app.update(1.0 / 30.0);
    }
    terminal
        .draw(|f| ui::draw(f, app))
        .map_err(|e| e.to_string())?;
    let buf = terminal.backend().buffer().clone();
    let sgr = |c: Color, base: u8| -> String {
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[{};2;{r};{g};{b}m", base),
            Color::Indexed(n) => format!("\x1b[{};5;{n}m", base),
            _ => format!("\x1b[{}m", base + 1),
        }
    };
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            let cell = &buf[(x, y)];
            out.push_str("\x1b[0m");
            out.push_str(&sgr(cell.fg, 38));
            out.push_str(&sgr(cell.bg, 48));
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    io::stdout()
        .write_all(out.as_bytes())
        .map_err(|e| e.to_string())
}
