//! Kitty graphics protocol output (kitty, Ghostty, WezTerm, Konsole): bodies are sent as RGBA
//! images placed over the text grid, so they are drawn with real pixels and true transparency.
//! See https://sw.kovidgoyal.net/kitty/graphics-protocol/

use std::collections::HashMap;
use std::io::{self, Write};

use flate2::write::ZlibEncoder;
use flate2::Compression;

/// An image ready to place: top-left cell (absolute), size in cells and pixels.
pub struct Placed {
    pub slot: u32,
    pub col: u16,
    pub row: u16,
    pub cols: u32,
    pub rows: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Leave the image already on screen for this slot untouched.
    pub keep: bool,
}

/// Tracks which image id is on screen for each slot. Every frame a slot's new picture goes
/// to its other id and only then is the old one deleted, so nothing flickers.
#[derive(Default)]
pub struct Kitty {
    live: HashMap<u32, u32>,
}

const BASE_ID: u32 = 7100;

fn id_for(slot: u32, parity: u32) -> u32 {
    BASE_ID + slot * 2 + parity
}

impl Kitty {
    pub fn frame(&mut self, out: &mut impl Write, images: &[Placed]) -> io::Result<()> {
        let mut seen = Vec::with_capacity(images.len());
        for img in images {
            if img.keep && self.live.contains_key(&img.slot) {
                seen.push(img.slot);
                continue;
            }
            if img.w == 0 || img.h == 0 {
                continue;
            }
            let old = self.live.get(&img.slot).copied();
            let id = id_for(img.slot, old.map_or(0, |o| 1 - (o - BASE_ID) % 2));
            transmit(out, id, img)?;
            if let Some(o) = old {
                delete(out, o)?;
            }
            self.live.insert(img.slot, id);
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

fn transmit(out: &mut impl Write, id: u32, img: &Placed) -> io::Result<()> {
    let mut z = ZlibEncoder::new(Vec::with_capacity(img.rgba.len() / 3), Compression::fast());
    z.write_all(&img.rgba)?;
    let data = base64(&z.finish()?);
    // place at the cell without moving the cursor; stretch to the cell box when the image
    // was rendered at reduced resolution
    write!(out, "\x1b[{};{}H", img.row as u32 + 1, img.col as u32 + 1)?;
    let mut first = format!(
        "a=T,q=2,f=32,o=z,s={},v={},i={id},p=1,C=1,z=1",
        img.w, img.h
    );
    first.push_str(&format!(",c={},r={}", img.cols, img.rows));
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        let more = (k + 1 < chunks.len()) as u8;
        if k == 0 {
            write!(out, "\x1b_G{first},m={more};")?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
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

    #[test]
    fn double_buffers_and_cleans_up() {
        let img = |slot| Placed {
            slot,
            col: 0,
            row: 0,
            cols: 1,
            rows: 1,
            w: 1,
            h: 1,
            rgba: vec![255; 4],
            keep: false,
        };
        let mut k = Kitty::default();
        let mut out = Vec::new();
        k.frame(&mut out, &[img(3)]).unwrap();
        let first = k.live[&3];
        k.frame(&mut out, &[img(3)]).unwrap();
        assert_ne!(k.live[&3], first, "a new frame goes to the other id");
        let second = k.live[&3];
        let kept = Placed {
            keep: true,
            rgba: vec![],
            w: 0,
            h: 0,
            ..img(3)
        };
        out.clear();
        k.frame(&mut out, &[kept]).unwrap();
        assert!(out.is_empty(), "a kept image sends nothing");
        assert_eq!(k.live[&3], second);
        k.frame(&mut out, &[]).unwrap();
        assert!(k.live.is_empty());
        assert!(String::from_utf8_lossy(&out).contains("a=d,d=I"));
    }
}
