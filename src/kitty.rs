//! Kitty graphics protocol output (kitty, Ghostty, WezTerm, Konsole): bodies are sent as RGBA
//! images placed over the text grid, so they are drawn with real pixels and true transparency.
//! See https://sw.kovidgoyal.net/kitty/graphics-protocol/
//!
//! Every frame each image is only *moved* (a tiny placement command, to the pixel), which is
//! what makes motion smooth. New pixels are sent only when the picture itself changes, and on
//! a local terminal they go through a temporary file instead of the escape stream.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::write::ZlibEncoder;
use flate2::Compression;

/// An image placement for this frame, in absolute screen terms.
pub struct Placed {
    pub slot: u32,
    /// Cell holding the top-left corner, and the pixel offset inside that cell.
    pub col: u16,
    pub row: u16,
    pub x_off: u32,
    pub y_off: u32,
    /// Visible part of the image (x, y, w, h), in image pixels.
    pub src: (u32, u32, u32, u32),
    /// Full image size and pixels (empty when `keep`).
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Reuse the picture already on screen for this slot; only move it.
    pub keep: bool,
    /// Stacking order: higher is drawn on top (all above the text).
    pub z: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// Pixels inside the escape sequence: zlib + base64. Works everywhere, including SSH.
    Direct,
    /// Pixels in a temporary file the terminal reads and deletes. Much cheaper, local only.
    TempFile,
}

#[derive(Default)]
pub struct Kitty {
    live: HashMap<u32, u32>,
}

const BASE_ID: u32 = 7100;

fn id_for(slot: u32, parity: u32) -> u32 {
    BASE_ID + slot * 2 + parity
}

impl Kitty {
    pub fn frame(
        &mut self,
        out: &mut impl Write,
        images: &[Placed],
        how: Transfer,
    ) -> io::Result<()> {
        let mut seen = Vec::with_capacity(images.len());
        for img in images {
            match (img.keep, self.live.get(&img.slot).copied()) {
                (true, Some(id)) => place(out, id, img)?,
                (true, None) => continue, // nothing to move yet; it is redrawn next time
                (false, old) => {
                    if img.w == 0 || img.h == 0 {
                        continue;
                    }
                    // new pixels go to the slot's other id, then the old picture is dropped,
                    // so the image never disappears for a frame
                    let id = id_for(img.slot, old.map_or(0, |o| 1 - (o - BASE_ID) % 2));
                    transmit(out, id, img.w, img.h, &img.rgba, how)?;
                    place(out, id, img)?;
                    if let Some(o) = old {
                        delete(out, o)?;
                    }
                    self.live.insert(img.slot, id);
                }
            }
            seen.push(img.slot);
        }
        let stale: Vec<u32> = self
            .live
            .keys()
            .copied()
            .filter(|s| !seen.contains(s))
            .collect();
        for slot in stale {
            if let Some(id) = self.live.remove(&slot) {
                delete(out, id)?;
            }
        }
        Ok(())
    }

    /// Remove everything we put on screen (on exit).
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        for (_, id) in self.live.drain() {
            delete(out, id)?;
        }
        out.flush()
    }
}

fn delete(out: &mut impl Write, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Show (or move) image `id` at the placement's cell and pixel offset, cropped to `src`.
fn place(out: &mut impl Write, id: u32, p: &Placed) -> io::Result<()> {
    let (sx, sy, sw, sh) = p.src;
    write!(
        out,
        "\x1b[{};{}H\x1b_Ga=p,i={id},p=1,x={sx},y={sy},w={sw},h={sh},X={},Y={},C=1,z={},q=2\x1b\\",
        p.row as u32 + 1,
        p.col as u32 + 1,
        p.x_off,
        p.y_off,
        p.z.max(1)
    )
}

static FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Temporary files must live in the temp dir and carry this marker, or the terminal
/// refuses (and does not delete) them.
fn temp_path() -> PathBuf {
    let n = FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "cosmoscope-tty-graphics-protocol-{}-{n}.rgba",
        std::process::id()
    ))
}

fn transmit(
    out: &mut impl Write,
    id: u32,
    w: u32,
    h: u32,
    rgba: &[u8],
    how: Transfer,
) -> io::Result<()> {
    if how == Transfer::TempFile {
        let path = temp_path();
        if std::fs::write(&path, rgba).is_ok() {
            let name = base64(path.to_string_lossy().as_bytes());
            return write!(
                out,
                "\x1b_Ga=t,t=t,q=2,f=32,s={w},v={h},i={id};{name}\x1b\\"
            );
        }
    }
    let mut z = ZlibEncoder::new(Vec::with_capacity(rgba.len() / 3), Compression::fast());
    z.write_all(rgba)?;
    let data = base64(&z.finish()?);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        let more = (k + 1 < chunks.len()) as u8;
        if k == 0 {
            write!(out, "\x1b_Ga=t,q=2,f=32,o=z,s={w},v={h},i={id},m={more};")?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Finds out, without blocking start-up, whether the terminal reads images from temporary
/// files. Frames go out directly meanwhile; a terminal that supports it reads and deletes the
/// probe file within milliseconds, otherwise we tidy it up after a while and stay direct.
pub struct Probe {
    path: PathBuf,
    since: std::time::Instant,
}

const PROBE_ID: u32 = BASE_ID - 1;

impl Probe {
    /// Start probing, or return the transfer to use straight away when there is no choice.
    pub fn start(out: &mut impl Write) -> Result<Probe, Transfer> {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        match var("COSMOSCOPE_TRANSFER").as_str() {
            "direct" => return Err(Transfer::Direct),
            "file" => return Err(Transfer::TempFile),
            _ => {}
        }
        // over SSH the terminal cannot see our files
        if !var("SSH_CONNECTION").is_empty() || !var("SSH_TTY").is_empty() {
            return Err(Transfer::Direct);
        }
        let path = temp_path();
        std::fs::write(&path, [0u8; 4]).map_err(|_| Transfer::Direct)?;
        let name = base64(path.to_string_lossy().as_bytes());
        let _ = write!(
            out,
            "\x1b_Ga=t,t=t,q=2,f=32,s=1,v=1,i={PROBE_ID};{name}\x1b\\"
        );
        let _ = out.flush();
        Ok(Probe {
            path,
            since: std::time::Instant::now(),
        })
    }

    /// The verdict, once there is one.
    pub fn poll(&self, out: &mut impl Write) -> Option<Transfer> {
        if !self.path.exists() {
            let _ = delete(out, PROBE_ID);
            return Some(Transfer::TempFile);
        }
        if self.since.elapsed() > std::time::Duration::from_secs(2) {
            let _ = std::fs::remove_file(&self.path);
            return Some(Transfer::Direct);
        }
        None
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// Is this terminal known to speak the kitty graphics protocol?
pub fn supported_by_env() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if !var("TMUX").is_empty()
        || var("TERM").starts_with("screen")
        || var("TERM").starts_with("tmux")
    {
        return false; // multiplexers do not pass the images through
    }
    var("TERM").contains("kitty")
        || !var("KITTY_WINDOW_ID").is_empty()
        || !var("GHOSTTY_RESOURCES_DIR").is_empty()
        || matches!(var("TERM_PROGRAM").as_str(), "ghostty" | "WezTerm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    fn img(slot: u32, keep: bool) -> Placed {
        Placed {
            slot,
            col: 2,
            row: 3,
            x_off: 4,
            y_off: 5,
            src: (0, 0, 1, 1),
            w: 1,
            h: 1,
            rgba: if keep { vec![] } else { vec![255; 4] },
            keep,
            z: 3,
        }
    }

    #[test]
    fn redraws_double_buffer_and_moves_are_cheap() {
        let mut k = Kitty::default();
        let mut out = Vec::new();
        k.frame(&mut out, &[img(3, false)], Transfer::Direct)
            .unwrap();
        let first = k.live[&3];
        k.frame(&mut out, &[img(3, false)], Transfer::Direct)
            .unwrap();
        assert_ne!(k.live[&3], first, "new pixels go to the other id");
        out.clear();
        k.frame(&mut out, &[img(3, true)], Transfer::Direct)
            .unwrap();
        let text = String::from_utf8_lossy(&out).to_string();
        assert!(
            text.contains("a=p") && !text.contains("a=t"),
            "a kept image is only moved: {text:?}"
        );
        assert!(text.contains("X=4,Y=5") && text.starts_with("\x1b[4;3H"));
        out.clear();
        k.frame(&mut out, &[], Transfer::Direct).unwrap();
        assert!(k.live.is_empty());
        assert!(String::from_utf8_lossy(&out).contains("a=d,d=I"));
    }
}
