//! Sixel output (Windows Terminal 1.22+, foot, WezTerm, mlterm, xterm -ti vt340): bodies are
//! drawn as real images on terminals without the kitty graphics protocol.
//!
//! Sixel has no way to move an image that is already on screen, so every frame the screen is
//! cleared and each image is sent again at its new place; the last picture of each slot is
//! kept here, already reduced to its palette, so that only its placement is recomputed.
//! Pixels are either drawn or left transparent, so soft edges and glows are blended over the
//! terminal's own background colour, and only the faintest pixels are left out.

use std::collections::HashMap;
use std::io::{self, Write};

use crate::kitty::Placed;

pub struct Sixel {
    pics: HashMap<u32, Pic>,
    buf: Vec<u8>,
    background: [u8; 3],
}

/// A picture as palette indices; `NONE` marks a transparent pixel.
struct Pic {
    w: u32,
    h: u32,
    idx: Vec<u8>,
    /// Colours in sixel percent (0..=100).
    palette: Vec<[u8; 3]>,
}

const NONE: u8 = 255;
/// Fainter pixels than this are left transparent.
const MIN_ALPHA: f32 = 0.025;
/// Windows Terminal's default background (Campbell), for terminals that do not tell theirs.
pub const DEFAULT_BACKGROUND: [u8; 3] = [12, 12, 12];

impl Sixel {
    /// Soft edges will be blended over `background`, the terminal's background colour.
    pub fn new(background: [u8; 3]) -> Sixel {
        Sixel {
            pics: HashMap::new(),
            buf: Vec::new(),
            background,
        }
    }

    pub fn frame(
        &mut self,
        out: &mut impl Write,
        images: &[Placed],
        cell: (u32, u32),
    ) -> io::Result<()> {
        let mut order: Vec<&Placed> = images.iter().collect();
        order.sort_by_key(|p| p.z);
        let mut seen = Vec::with_capacity(images.len());
        for p in order {
            if !p.keep && p.w > 0 && p.h > 0 {
                let pic = quantize(&p.rgba, p.w, p.h, self.background);
                self.pics.insert(p.slot, pic);
            }
            seen.push(p.slot);
            let Some(pic) = self.pics.get(&p.slot) else {
                continue;
            };
            self.buf.clear();
            encode(&mut self.buf, pic, p, cell);
            write!(out, "\x1b[{};{}H", p.row as u32 + 1, p.col as u32 + 1)?;
            out.write_all(&self.buf)?;
        }
        self.pics.retain(|s, _| seen.contains(s));
        Ok(())
    }
}

/// A 15-bit colour (5 bits per channel), the resolution palettes are built at.
fn bin(c: [u8; 3]) -> usize {
    (c[0] as usize >> 3) << 10 | (c[1] as usize >> 3) << 5 | c[2] as usize >> 3
}

fn quantize(rgba: &[u8], w: u32, h: u32, background: [u8; 3]) -> Pic {
    // the opaque colour of each pixel, or None where the pixel stays transparent
    let mut count = vec![0u32; 1 << 15];
    let mut sum = vec![[0u64; 3]; 1 << 15];
    let px: Vec<Option<[u8; 3]>> = rgba
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            (a >= MIN_ALPHA).then(|| {
                let c = [0, 1, 2]
                    .map(|k| (p[k] as f32 * a + background[k] as f32 * (1.0 - a) + 0.5) as u8);
                count[bin(c)] += 1;
                for (s, v) in sum[bin(c)].iter_mut().zip(c) {
                    *s += v as u64;
                }
                c
            })
        })
        .collect();
    let (palette, lookup) = median_cut(&count, &sum, NONE as usize);
    Pic {
        w,
        h,
        idx: px
            .iter()
            .map(|c| c.map_or(NONE, |c| lookup[bin(c)]))
            .collect(),
        palette: palette
            .iter()
            .map(|c| c.map(|v| ((v as u32 * 100 + 127) / 255) as u8))
            .collect(),
    }
}

/// Split the colours in use into at most `n` boxes, each time cutting the box with the most
/// pixels across its widest channel where half its pixels fall on each side, and give each
/// box the mean of the colours in it (`sum` holds each 15-bit colour's channel totals).
/// Returns the palette and, for every 15-bit colour, its palette index.
fn median_cut(count: &[u32], sum: &[[u64; 3]], n: usize) -> (Vec<[u8; 3]>, Vec<u8>) {
    let channel = |b: usize, ch: usize| (b >> (10 - 5 * ch)) & 31;
    let weight = |bx: &[usize]| bx.iter().map(|&b| count[b] as u64).sum::<u64>();
    let mut boxes: Vec<Vec<usize>> = vec![(0..count.len()).filter(|&b| count[b] > 0).collect()];
    while boxes.len() < n {
        let Some(k) = (0..boxes.len())
            .filter(|&k| boxes[k].len() > 1)
            .max_by_key(|&k| weight(&boxes[k]))
        else {
            break;
        };
        let bx = &mut boxes[k];
        let spread = |ch: usize| {
            let (lo, hi) = bx.iter().fold((31, 0), |(lo, hi), &b| {
                (lo.min(channel(b, ch)), hi.max(channel(b, ch)))
            });
            hi - lo
        };
        let ch = (0..3).max_by_key(|&ch| spread(ch)).unwrap_or(0);
        bx.sort_by_key(|&b| channel(b, ch));
        let half = weight(bx) / 2;
        let mut acc = 0;
        let at = bx
            .iter()
            .position(|&b| {
                acc += count[b] as u64;
                acc >= half
            })
            .map_or(1, |j| (j + 1).clamp(1, bx.len() - 1));
        let rest = bx.split_off(at);
        boxes.push(rest);
    }
    let mut lookup = vec![0u8; count.len()];
    let palette = boxes
        .iter()
        .enumerate()
        .map(|(k, bx)| {
            let (mut total, mut pixels) = ([0u64; 3], 0u64);
            for &b in bx {
                lookup[b] = k as u8;
                for (t, s) in total.iter_mut().zip(sum[b]) {
                    *t += s;
                }
                pixels += count[b] as u64;
            }
            total.map(|t| ((t + pixels / 2) / pixels.max(1)).min(255) as u8)
        })
        .collect();
    (palette, lookup)
}

/// One placement as a sixel image: the `src` crop of the picture, scaled to `cols` x `rows`
/// cells when asked, shifted right and down by the offset inside the first cell.
fn encode(out: &mut Vec<u8>, pic: &Pic, p: &Placed, cell: (u32, u32)) {
    let (sx, sy, sw, sh) = p.src;
    let (dw, dh) = if p.cols > 0 {
        (p.cols * cell.0, p.rows * cell.1)
    } else {
        (sw, sh)
    };
    let (ox, oy) = (p.x_off, p.y_off);
    let (w, h) = (dw + ox, dh + oy);
    let at = |x: u32, y: u32| -> u8 {
        if x < ox || y < oy {
            return NONE;
        }
        let u = sx + ((x - ox) as u64 * sw as u64 / dw as u64) as u32;
        let v = sy + ((y - oy) as u64 * sh as u64 / dh as u64) as u32;
        if u >= pic.w || v >= pic.h {
            return NONE;
        }
        pic.idx[(v * pic.w + u) as usize]
    };
    // 0;1 keeps the background transparent; the raster attributes give square pixels
    let _ = write!(out, "\x1bP0;1;0q\"1;1;{w};{h}");
    for (k, [r, g, b]) in pic.palette.iter().enumerate() {
        let _ = write!(out, "#{k};2;{r};{g};{b}");
    }
    let mut rows: Vec<Vec<u8>> = vec![Vec::new(); NONE as usize];
    let mut used: Vec<u8> = Vec::new();
    for band in (0..h).step_by(6) {
        for &c in &used {
            rows[c as usize].clear();
        }
        used.clear();
        for bit in 0..6.min(h - band) {
            for x in 0..w {
                let c = at(x, band + bit);
                if c == NONE {
                    continue;
                }
                let row = &mut rows[c as usize];
                if row.is_empty() {
                    row.resize(w as usize, 0);
                    used.push(c);
                }
                row[x as usize] |= 1 << bit;
            }
        }
        for (n, &c) in used.iter().enumerate() {
            if n > 0 {
                out.push(b'$');
            }
            let _ = write!(out, "#{c}");
            let row = &rows[c as usize];
            // trailing empty columns need not be sent
            let end = row.iter().rposition(|&b| b != 0).map_or(0, |e| e + 1);
            let mut x = 0;
            while x < end {
                let mut run = 1;
                while x + run < end && row[x + run] == row[x] {
                    run += 1;
                }
                let ch = 63 + row[x];
                if run > 3 {
                    let _ = write!(out, "!{run}{}", ch as char);
                } else {
                    out.extend(std::iter::repeat_n(ch, run));
                }
                x += run;
            }
        }
        out.push(b'-');
    }
    out.extend_from_slice(b"\x1b\\");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a sixel image into (width, height, pixels as RGB in percent, None = transparent).
    fn decode(data: &[u8]) -> (usize, usize, Vec<Option<[u8; 3]>>) {
        let s = std::str::from_utf8(data).unwrap();
        let body = s.split_once('q').unwrap().1.strip_suffix("\x1b\\").unwrap();
        let mut chars = body.chars().peekable();
        let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
            let mut n = 0usize;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n * 10 + d as usize;
                chars.next();
            }
            n
        };
        let (mut w, mut h) = (0, 0);
        let mut palette = HashMap::new();
        let mut px: HashMap<(usize, usize), usize> = HashMap::new();
        let (mut x, mut band, mut colour) = (0, 0, 0);
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let mut v = [0; 4];
                    for (k, slot) in v.iter_mut().enumerate() {
                        if k > 0 {
                            assert_eq!(chars.next(), Some(';'));
                        }
                        *slot = number(&mut chars);
                    }
                    (w, h) = (v[2], v[3]);
                }
                '#' => {
                    colour = number(&mut chars);
                    if chars.peek() == Some(&';') {
                        let mut v = [0; 4];
                        for slot in v.iter_mut() {
                            assert_eq!(chars.next(), Some(';'));
                            *slot = number(&mut chars);
                        }
                        assert_eq!(v[0], 2, "RGB colours");
                        palette.insert(colour, [v[1] as u8, v[2] as u8, v[3] as u8]);
                    }
                }
                '$' => x = 0,
                '-' => {
                    x = 0;
                    band += 6;
                }
                '!' | '?'..='~' => {
                    let run = if c == '!' { number(&mut chars) } else { 1 };
                    let bits = if c == '!' { chars.next().unwrap() } else { c } as u32 - 63;
                    for _ in 0..run {
                        for bit in 0..6 {
                            if bits & 1 << bit != 0 {
                                px.insert((x, band + bit), colour);
                            }
                        }
                        x += 1;
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(palette.len() <= 255, "{} colours", palette.len());
        let grid = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|k| px.get(&k).map(|c| palette[c]))
            .collect();
        assert!(
            px.keys().all(|&(x, y)| x < w && y < h),
            "pixels outside the image"
        );
        (w, h, grid)
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    /// A 4x3 picture: red and blue stripes with a transparent pixel and a faint one.
    fn picture() -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..3 {
            for x in 0..4 {
                let p = match (x, y) {
                    (0, 0) => CLEAR,
                    (3, 2) => [255, 255, 255, 3], // too faint to draw
                    (_, 1) => BLUE,
                    _ => RED,
                };
                rgba.extend_from_slice(&p);
            }
        }
        rgba
    }

    fn placed(keep: bool) -> Placed {
        Placed {
            slot: 1,
            col: 2,
            row: 5,
            x_off: 0,
            y_off: 0,
            src: (0, 0, 4, 3),
            cols: 0,
            rows: 0,
            w: 4,
            h: 3,
            rgba: if keep { Vec::new() } else { picture() },
            keep,
            z: 1,
        }
    }

    fn draw(sx: &mut Sixel, p: Placed) -> (usize, usize, Vec<Option<[u8; 3]>>) {
        let mut out = Vec::new();
        sx.frame(&mut out, &[p], (10, 20)).unwrap();
        assert!(
            out.starts_with(b"\x1b[6;3H\x1bP0;1;0q"),
            "placed at row 6, column 3"
        );
        decode(&out[b"\x1b[6;3H".len()..])
    }

    #[test]
    fn pictures_keep_their_pixels_and_transparency() {
        let (w, h, px) = draw(&mut Sixel::new(DEFAULT_BACKGROUND), placed(false));
        assert_eq!((w, h), (4, 3));
        let (red, blue) = (Some([100, 0, 0]), Some([0, 0, 100]));
        #[rustfmt::skip]
        let want = [
            None, red, red, red,
            blue, blue, blue, blue,
            red, red, red, None,
        ];
        assert_eq!(px, want);
    }

    #[test]
    fn kept_pictures_move_by_whole_pixels_and_scale_to_cells() {
        let mut sx = Sixel::new(DEFAULT_BACKGROUND);
        draw(&mut sx, placed(false));
        // moved 2 pixels right and 1 down inside its first cell, showing only the bottom rows
        let moved = Placed {
            x_off: 2,
            y_off: 1,
            src: (0, 1, 4, 2),
            ..placed(true)
        };
        let (w, h, px) = draw(&mut sx, moved);
        assert_eq!((w, h), (6, 3));
        assert!(
            px[..6].iter().all(Option::is_none),
            "the offset is transparent"
        );
        assert_eq!(px[6 + 2], Some([0, 0, 100]), "blue row first");
        assert_eq!(px[12 + 2], Some([100, 0, 0]));
        // stretched over 1x1 cell: 10x20 pixels
        let scaled = Placed {
            cols: 1,
            rows: 1,
            ..placed(true)
        };
        let (w, h, px) = draw(&mut sx, scaled);
        assert_eq!((w, h), (10, 20));
        assert_eq!(px[0], None);
        assert_eq!(px[19 * 10 + 9], None, "the faint corner stays clear");
        assert_eq!(px[10 * 10 + 5], Some([0, 0, 100]));
    }

    #[test]
    fn soft_edges_blend_over_the_background_and_slots_are_forgotten() {
        let mut sx = Sixel::new([255, 255, 255]);
        let half = Placed {
            rgba: [255, 0, 0, 128].repeat(12),
            ..placed(false)
        };
        let (_, _, px) = draw(&mut sx, half);
        // half red over white is pink, (255,128,128) give or take the 15-bit palette
        let [r, g, b] = px[0].unwrap();
        assert!(
            r == 100 && (48..=52).contains(&g) && (48..=52).contains(&b),
            "{r} {g} {b}"
        );
        let mut out = Vec::new();
        sx.frame(&mut out, &[], (10, 20)).unwrap();
        sx.frame(&mut out, &[placed(true)], (10, 20)).unwrap();
        assert!(
            out.is_empty(),
            "a slot that left the screen has nothing to show"
        );
    }

    #[test]
    fn many_colours_fit_in_the_palette() {
        let rgba: Vec<u8> = (0..64 * 64u32)
            .flat_map(|i| {
                [
                    (i % 64 * 4) as u8,
                    (i / 64 * 4) as u8,
                    (i % 7 * 36) as u8,
                    255,
                ]
            })
            .collect();
        let p = Placed {
            src: (0, 0, 64, 64),
            w: 64,
            h: 64,
            rgba,
            ..placed(false)
        };
        let (w, h, px) = draw(&mut Sixel::new(DEFAULT_BACKGROUND), p);
        assert_eq!((w, h), (64, 64));
        assert!(px.iter().all(Option::is_some));
    }
}
