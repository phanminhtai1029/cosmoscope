//! cosmoscope: a living, interactive Solar System in the terminal.

mod app;
mod console;
mod data;
mod ephemeris;
mod kitty;
mod math;
mod profile;
mod render;
mod sixel;
mod textures;
mod time;
mod ui;

use std::io::{self, Write};
use std::time::{Duration, Instant};

use clap::Parser;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::terminal::{self as term, BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::crossterm::{cursor, execute, queue};
use ratatui::style::Color;
use ratatui::Terminal;

use app::App;

type Term = Terminal<CrosstermBackend<console::FrameOut>>;

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
    /// Draw planets as real images: auto (with the kitty graphics protocol in kitty, Ghostty
    /// and WezTerm, or sixel in Windows Terminal), kitty, sixel (force one) or off
    #[arg(long, default_value = "auto")]
    graphics: String,
    /// Glyph set: auto, full, or basic (only glyphs that Consolas and Lucida Console have;
    /// auto picks it in the classic Windows console)
    #[arg(long, default_value = "auto")]
    glyphs: String,
    /// Cell size in pixels for --snapshot with graphics, e.g. 10x20
    #[arg(long, hide = true, default_value = "10x20")]
    cell: String,
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
    let glyphs = std::env::var("COSMOSCOPE_GLYPHS").unwrap_or(args.glyphs.clone());
    render::set_basic_glyphs(match glyphs.as_str() {
        "auto" => console::classic_windows_console(),
        "full" => false,
        "basic" => true,
        other => {
            return Err(format!(
                "unknown glyph set '{other}' (use auto, full or basic)"
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

    let graphics = std::env::var("COSMOSCOPE_GRAPHICS").unwrap_or(args.graphics.clone());
    if !matches!(graphics.as_str(), "auto" | "kitty" | "sixel" | "off") {
        return Err(format!(
            "unknown graphics mode '{graphics}' (use auto, kitty, sixel or off)"
        ));
    }

    if let Some(size) = &args.snapshot {
        let mode = match graphics.as_str() {
            "auto" if kitty::supported_by_env() => Graphics::Kitty,
            "kitty" => Graphics::Kitty,
            "sixel" => Graphics::Sixel,
            _ => Graphics::Off,
        };
        if mode != Graphics::Off {
            app.hd = Some(parse_size(&args.cell).ok_or("cell size must look like 10x20")?);
        }
        return snapshot(&mut app, size, args.at, mode);
    }

    console::check_console()?;
    if graphics != "off" && console::reopen_in_windows_terminal() {
        println!("cosmoscope opened in Windows Terminal.");
        return Ok(());
    }
    let answers = console::ask_terminal();
    let mode = match graphics.as_str() {
        "kitty" => Graphics::Kitty,
        "sixel" => Graphics::Sixel,
        "off" => Graphics::Off,
        // kitty images are placed to the pixel, and Windows cannot tell the cell size
        _ if kitty::supported_by_env() && !cfg!(windows) => Graphics::Kitty,
        _ if answers.sixel => Graphics::Sixel,
        _ => Graphics::Off,
    };
    let want_hd = mode != Graphics::Off;
    textures::warm_up();
    // a small pool renders the images just as fast as one thread per core, while idle
    // workers spinning on every core would burn far more CPU
    let workers = std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(2, 4);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build_global();
    let mut terminal = init_terminal().map_err(|e| {
        ratatui::restore();
        e.to_string()
    })?;
    let mut transfer = (kitty::Transfer::Direct, None);
    let mut sixel = None;
    if mode == Graphics::Sixel {
        // Windows Terminal draws sixel images as if a cell were 10x20 pixels, whatever the font
        app.hd = Some(cell_size().unwrap_or((10, 20)));
        sixel = Some(sixel::Sixel::new(
            answers.background.unwrap_or(sixel::DEFAULT_BACKGROUND),
        ));
    } else if want_hd {
        app.hd = cell_size();
        if app.hd.is_some() {
            kitty::sweep_temp_files(false);
            transfer = match kitty::Probe::start(&mut io::stdout()) {
                Ok(probe) => (kitty::Transfer::Direct, Some(probe)),
                Err(t) => (t, None),
            };
        }
    }
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    let mut gfx = kitty::Kitty::default();
    let timer = console::FineTimer::new();
    console::catch_break();
    let result = event_loop(
        &mut terminal,
        &mut app,
        &mut gfx,
        sixel.as_mut(),
        transfer.0,
        transfer.1,
        args.fps,
    );
    drop(timer);
    // an error can end the loop between the start and the end of a frame
    let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    let _ = gfx.clear(terminal.backend_mut());
    kitty::sweep_temp_files(true);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Graphics {
    Off,
    Kitty,
    Sixel,
}

/// Like `ratatui::init`, but frames are written through [`console::FrameOut`].
fn init_terminal() -> io::Result<Term> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // the frame buffer drops what it holds once panicking, cursor command included
        let _ = execute!(io::stdout(), EndSynchronizedUpdate, cursor::Show);
        ratatui::restore();
        hook(info);
    }));
    term::enable_raw_mode()?;
    execute!(io::stdout(), term::EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(console::FrameOut::default()))
}

/// Pixel size of one cell, when the terminal reports its window size in pixels.
fn cell_size() -> Option<(u32, u32)> {
    let ws = term::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    let cell = ((ws.width / ws.columns) as u32, (ws.height / ws.rows) as u32);
    Some(cell).filter(|&(w, h)| w >= 4 && h >= 8)
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn event_loop(
    terminal: &mut Term,
    app: &mut App,
    gfx: &mut kitty::Kitty,
    mut sixel: Option<&mut sixel::Sixel>,
    mut transfer: kitty::Transfer,
    mut probe: Option<kitty::Probe>,
    fps: u32,
) -> io::Result<()> {
    let frame = Duration::from_secs_f64(1.0 / fps as f64);
    let mut last = Instant::now();
    while !app.quit && !console::break_pressed() {
        let now = Instant::now();
        profile::time("update", || app.update((now - last).as_secs_f64().min(0.1)));
        last = now;
        // Each frame is written in one piece, and terminals that support synchronized
        // output show it only once it is complete, so nobody sees a half-drawn frame. Text
        // and images land together, so planets never lag behind their labels.
        queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        if sixel.is_some() && app.hd.is_some() {
            // sixel images cannot be moved: wipe last frame's, text and all
            terminal.clear()?;
        }
        profile::time("draw", || terminal.draw(|f| ui::draw(f, app)))?;
        if let (Some(sx), Some(cell)) = (sixel.as_deref_mut(), app.hd) {
            let out = terminal.backend_mut();
            profile::time("gfx", || sx.frame(out, &app.images, cell))?;
        } else if app.hd.is_some() {
            let out = terminal.backend_mut();
            if let Some(verdict) = probe.as_mut().and_then(|p| p.poll(out)) {
                transfer = verdict;
                probe = None;
            }
            profile::time("gfx", || gfx.frame(out, &app.images, transfer))?;
        }
        execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
        profile::end_frame();
        let deadline = now + frame;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // keys that are already waiting are read even when the frame is due, so a burst
            // from an input method is handled as one batch
            if !event::poll(left)? {
                break;
            }
            match event::read()? {
                // Windows also reports key releases; act on presses only
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Mouse(m) => app.mouse(m),
                Event::Resize(..) if app.hd.is_some() => app.hd = cell_size().or(app.hd),
                _ => {}
            }
            if app.quit || console::break_pressed() {
                break;
            }
        }
        app.end_input();
    }
    Ok(())
}

/// Simulate `at` seconds at 30 fps, render one frame off-screen and print it as ANSI text.
fn snapshot(app: &mut App, size: &str, at: f64, mode: Graphics) -> Result<(), String> {
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
    let mut bytes = out.into_bytes();
    // with graphics, the image commands follow the text, exactly as a terminal would get them
    let images = match (mode, app.hd) {
        (Graphics::Sixel, Some(cell)) => {
            sixel::Sixel::new(sixel::DEFAULT_BACKGROUND).frame(&mut bytes, &app.images, cell)
        }
        _ => kitty::Kitty::default().frame(&mut bytes, &app.images, kitty::Transfer::Direct),
    };
    images.map_err(|e| e.to_string())?;
    io::stdout().write_all(&bytes).map_err(|e| e.to_string())
}
