//! Terminal output and timing details that differ on Windows.
//!
//! A frame is collected in memory and handed to the terminal in one write. The Windows
//! console paints whenever it likes, so a frame written in small pieces (the standard
//! library writes at most 4 KiB per call) shows up half drawn and flickers.
//!
//! Windows also rounds every wait up to the system timer, 15.6 ms by default, so a 33 ms
//! frame wait lasts 47 ms and the animation stutters at about 21 fps. [`FineTimer`] asks for
//! 1 ms steps while cosmoscope runs.
//!
//! The classic console window (conhost, the default on Windows 10) draws with fonts such as
//! Consolas that lack some block glyphs; [`classic_windows_console`] tells it apart from
//! terminals like Windows Terminal that host the console themselves.
//!
//! Windows Terminal draws sixel images, which it says in its answer to a device attributes
//! query ([`ask_terminal`]); the classic console does not, so from there cosmoscope
//! reopens itself in Windows Terminal when it can ([`reopen_in_windows_terminal`]).
//!
//! Keys are read from the console, so terminals that talk to programs through pipes (Git
//! Bash's own MinTTY window) never deliver them; [`check_console`] says so up front. Ctrl+Break still
//! signals in raw mode, so [`catch_break`] turns it into an ordinary quit.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// A `Write` that keeps everything until `flush`, then writes it to stdout at once.
#[derive(Default)]
pub struct FrameOut {
    buf: Vec<u8>,
}

impl Write for FrameOut {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // after a panic the terminal has already been restored, and a half-drawn frame
        // would only spill onto the shell's screen
        let result = if self.buf.is_empty() || std::thread::panicking() {
            Ok(())
        } else {
            write_stdout(&self.buf)
        };
        self.buf.clear();
        result
    }
}

impl Drop for FrameOut {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(not(windows))]
fn write_stdout(data: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(data)?;
    out.flush()
}

#[cfg(windows)]
fn write_stdout(data: &[u8]) -> io::Result<()> {
    let Some(console) = win::stdout_console() else {
        // redirected, or a terminal that talks through a pipe (mintty)
        let mut out = io::stdout().lock();
        out.write_all(data)?;
        return out.flush();
    };
    let text: Vec<u16> = String::from_utf8_lossy(data).encode_utf16().collect();
    let mut rest = &text[..];
    while !rest.is_empty() {
        let n = win::write_console(console, rest)?;
        rest = &rest[n..];
    }
    Ok(())
}

/// Is stdout shown in the classic Windows console window? Under Windows Terminal, VS Code,
/// WezTerm and other ConPTY terminals the console window is a hidden stand-in instead.
pub fn classic_windows_console() -> bool {
    #[cfg(windows)]
    return win::stdout_console().is_some() && win::console_window_is_classic();
    #[cfg(not(windows))]
    false
}

/// Fails when neither stdin nor stdout is a console, as under MinTTY: keys would never arrive.
pub fn check_console() -> Result<(), String> {
    #[cfg(windows)]
    if win::stdout_console().is_none() && !win::stdin_is_console() {
        return Err(
            "this terminal talks through pipes, so no keys would reach cosmoscope. Run it in \
             Windows Terminal (its Git Bash profile works too), PowerShell or cmd; \
             `winpty cosmoscope` also works, but only in 16 colours"
                .into(),
        );
    }
    Ok(())
}

/// What the terminal said about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Answers {
    /// It draws sixel images.
    pub sixel: bool,
    /// Its background colour, when it told.
    pub background: Option<[u8; 3]>,
}

/// Ask the terminal (Windows only) for its background colour (OSC 11) and its primary device
/// attributes (ESC [ c), whose answer lists 4 for sixel. Terminals answer in order and the
/// console host answers the second itself, at once, so its answer ends the wait; a terminal
/// that says nothing within a moment counts as plain text.
pub fn ask_terminal() -> Answers {
    #[cfg(windows)]
    return win::ask("\x1b]11;?\x1b\\\x1b[c").map_or_else(Answers::default, |r| parse_answers(&r));
    #[cfg(not(windows))]
    Answers::default()
}

#[cfg(any(windows, test))]
fn parse_answers(reply: &str) -> Answers {
    let sixel = reply
        .rsplit_once("\x1b[?")
        .and_then(|(_, rest)| rest.strip_suffix('c'))
        .is_some_and(|params| params.split(';').any(|p| p == "4"));
    // ESC ] 11 ; rgb:RRRR/GGGG/BBBB, each channel 1 to 4 hex digits
    let background = reply.split_once("\x1b]11;rgb:").and_then(|(_, rest)| {
        let end = rest.find(['\x1b', '\x07'])?;
        let mut channels = rest[..end].split('/').map(|h| {
            let max = 16u32.checked_pow(h.len() as u32)? - 1;
            let v = u32::from_str_radix(h, 16).ok()?;
            Some((v * 255 / max.max(1)) as u8)
        });
        Some([channels.next()??, channels.next()??, channels.next()??])
    });
    Answers { sixel, background }
}

/// From the classic console, start the same command in Windows Terminal, where planets are
/// drawn as images. Returns false when Windows Terminal is not there (or the user opted out
/// with COSMOSCOPE_NO_WT), so cosmoscope carries on here.
pub fn reopen_in_windows_terminal() -> bool {
    if !classic_windows_console() || std::env::var_os("COSMOSCOPE_NO_WT").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // wt reads ';' as "open another tab"
    let args = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().replace(';', "\\;"));
    std::process::Command::new("wt.exe")
        .args(["new-tab", "--title", "cosmoscope", "--"])
        .arg(exe)
        .args(args)
        .spawn()
        .is_ok()
}

static BREAK: AtomicBool = AtomicBool::new(false);

/// Turn Ctrl+Break into a request to quit (see [`break_pressed`]) instead of killing the
/// process with the terminal still in raw mode on the alternate screen.
pub fn catch_break() {
    #[cfg(windows)]
    win::catch_break();
}

pub fn break_pressed() -> bool {
    BREAK.load(Ordering::Relaxed)
}

/// Finer timer steps for as long as it lives (Windows only; elsewhere it does nothing).
pub struct FineTimer;

impl FineTimer {
    pub fn new() -> FineTimer {
        #[cfg(windows)]
        win::fine_timer(true);
        FineTimer
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        #[cfg(windows)]
        win::fine_timer(false);
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::sync::atomic::Ordering;

    type Handle = *mut c_void;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const CTRL_BREAK_EVENT: u32 = 1;
    const KEY_EVENT: u16 = 1;
    /// ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT
    const COOKED: u32 = 0x1 | 0x2 | 0x4;

    /// INPUT_RECORD, read as a key event (the largest member of its union is 16 bytes).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct InputRecord {
        kind: u16,
        _pad: u16,
        down: i32,
        repeat: u16,
        vk: u16,
        scan: u16,
        ch: u16,
        state: u32,
    }
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> Handle;
        fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
        fn GetConsoleWindow() -> Handle;
        fn SetConsoleMode(console: Handle, mode: u32) -> i32;
        fn ReadConsoleInputW(
            console: Handle,
            records: *mut InputRecord,
            len: u32,
            read: *mut u32,
        ) -> i32;
        fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
        fn WriteConsoleW(
            console: Handle,
            text: *const u16,
            len: u32,
            written: *mut u32,
            reserved: *mut c_void,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn IsWindowVisible(window: Handle) -> i32;
        fn GetClassNameW(window: Handle, name: *mut u16, len: i32) -> i32;
    }

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(ms: u32) -> u32;
        fn timeEndPeriod(ms: u32) -> u32;
    }

    fn std_console(which: u32) -> Option<Handle> {
        let mut mode = 0;
        // SAFETY: plain Win32 calls; GetConsoleMode only writes the u32 we pass
        unsafe {
            let h = GetStdHandle(which);
            (!h.is_null() && h != INVALID_HANDLE_VALUE && GetConsoleMode(h, &mut mode) != 0)
                .then_some(h)
        }
    }

    /// The console behind stdout, if stdout is one.
    pub fn stdout_console() -> Option<Handle> {
        std_console(STD_OUTPUT_HANDLE)
    }

    pub fn stdin_is_console() -> bool {
        std_console(STD_INPUT_HANDLE).is_some()
    }

    /// Send queries ending with ESC [ c and read what the terminal types back, without echo,
    /// up to the end of the device attributes answer.
    pub fn ask(query: &str) -> Option<String> {
        let (input, output) = (std_console(STD_INPUT_HANDLE)?, stdout_console()?);
        let mut mode = 0;
        let mut reply = String::new();
        // SAFETY: plain console calls on our own handles; ReadConsoleInputW writes at most
        // `records.len()` records into `records`
        unsafe {
            GetConsoleMode(input, &mut mode);
            SetConsoleMode(input, mode & !COOKED);
            let query: Vec<u16> = query.encode_utf16().collect();
            let asked = write_console(output, &query).is_ok();
            let start = std::time::Instant::now();
            let mut records = [InputRecord::default(); 32];
            let done = |r: &str| r.contains("\x1b[?") && r.ends_with('c');
            while asked && !done(&reply) && start.elapsed().as_millis() < 300 {
                if WaitForSingleObject(input, 20) != 0 {
                    continue;
                }
                let mut n = 0;
                if ReadConsoleInputW(input, records.as_mut_ptr(), records.len() as u32, &mut n) == 0
                {
                    break;
                }
                for r in &records[..n as usize] {
                    if r.kind == KEY_EVENT && r.down != 0 && r.ch != 0 {
                        reply.extend(char::decode_utf16([r.ch]).flatten());
                    }
                }
            }
            SetConsoleMode(input, mode);
        }
        Some(reply)
    }

    unsafe extern "system" fn on_ctrl(event: u32) -> i32 {
        if event == CTRL_BREAK_EVENT {
            super::BREAK.store(true, Ordering::Relaxed);
            return 1;
        }
        0
    }

    pub fn catch_break() {
        // SAFETY: registers a handler that only stores to an atomic
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }

    pub fn console_window_is_classic() -> bool {
        let mut name = [0u16; 64];
        // SAFETY: GetClassNameW writes at most `name.len()` units into `name`
        let len = unsafe {
            let w = GetConsoleWindow();
            if w.is_null() || IsWindowVisible(w) == 0 {
                return false;
            }
            GetClassNameW(w, name.as_mut_ptr(), name.len() as i32)
        };
        String::from_utf16_lossy(&name[..len.max(0) as usize]) == "ConsoleWindowClass"
    }

    /// Write UTF-16 text to the console; returns how many units were taken.
    pub fn write_console(console: Handle, text: &[u16]) -> io::Result<usize> {
        let len = text.len().min(u32::MAX as usize) as u32;
        let mut written = 0;
        // SAFETY: `text` is valid for `len` units and `written` outlives the call
        let ok = unsafe {
            WriteConsoleW(
                console,
                text.as_ptr(),
                len,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        match (ok, written) {
            (0, _) => Err(io::Error::last_os_error()),
            (_, 0) => Err(io::ErrorKind::WriteZero.into()),
            _ => Ok(written as usize),
        }
    }

    /// Ask for 1 ms timer steps, or give the request back. Since Windows 10 2004 this only
    /// affects our own process.
    pub fn fine_timer(on: bool) {
        // SAFETY: no pointers involved; each begin is paired with an end
        unsafe {
            if on {
                timeBeginPeriod(1);
            } else {
                timeEndPeriod(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_terminal_answers() {
        // Windows Terminal 1.24 and the classic console on Windows 11, as they answer
        let wt = "\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c";
        assert_eq!(
            parse_answers(wt),
            Answers {
                sixel: true,
                background: Some([12, 12, 12])
            }
        );
        let conhost = parse_answers("\x1b[?61;1;6;7;21;22;23;24;28;32;42;52c");
        assert_eq!(conhost, Answers::default());
        assert_eq!(
            parse_answers("\x1b]11;rgb:ff/80/0\x07\x1b[?61;14c"),
            Answers {
                sixel: false,
                background: Some([255, 128, 0])
            }
        );
        assert_eq!(parse_answers(""), Answers::default());
    }
}
