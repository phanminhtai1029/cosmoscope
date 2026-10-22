#!/usr/bin/env python3
"""Rasterise cosmoscope --snapshot output (ANSI truecolor text) into a PNG.

Half blocks and braille are drawn as pixels so screenshots stay crisp.
    cosmoscope --snapshot 160x48 --at 6 > frame.ans && python3 tools/ansi2png.py frame.ans frame.png
"""
import re
import sys
from PIL import Image, ImageDraw, ImageFont

CW, CH = 8, 16
FONT_SIZE = 13
BG = (11, 12, 20)
FONTS = ["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/System/Library/Fonts/Menlo.ttc",
         "C:/Windows/Fonts/consola.ttf"]
TOKEN = re.compile(r"\x1b\[([0-9;]*)m|(.)")
QUADS = {c: m for m, c in enumerate(" ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█") if m}
QUADS.pop("▀"); QUADS.pop("▄")
BRAILLE = [(0, 0, 0x01), (0, 1, 0x02), (0, 2, 0x04), (1, 0, 0x08), (1, 1, 0x10), (1, 2, 0x20), (0, 3, 0x40), (1, 3, 0x80)]


def font():
    for f in FONTS:
        try:
            return ImageFont.truetype(f, FONT_SIZE)
        except OSError:
            pass
    return ImageFont.load_default()


def fallback_font():
    """DejaVu Sans Mono covers the arrows, bullets and symbols other mono fonts often lack."""
    try:
        return ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", FONT_SIZE)
    except OSError:
        return font()


def xterm256(n):
    if n >= 232:
        v = 8 + (n - 232) * 10
        return (v, v, v)
    if n >= 16:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return (lv[n // 36], lv[(n // 6) % 6], lv[n % 6])
    return (128, 128, 128)


def sgr(params, fg, bg):
    p = [int(x) if x else 0 for x in params.split(";")]
    i = 0
    while i < len(p):
        if p[i] == 0:
            fg, bg = None, None
        elif p[i] in (38, 48) and i + 4 < len(p) + 1 and p[i + 1] == 2:
            c = tuple(p[i + 2:i + 5])
            fg, bg = (c, bg) if p[i] == 38 else (fg, c)
            i += 4
        elif p[i] in (38, 48) and i + 2 < len(p) + 1 and p[i + 1] == 5:
            c = xterm256(p[i + 2])
            fg, bg = (c, bg) if p[i] == 38 else (fg, c)
            i += 2
        elif p[i] == 39:
            fg = None
        elif p[i] == 49:
            bg = None
        i += 1
    return fg, bg


def main(src, dst, bg=BG):
    lines = open(src, encoding="utf-8").read().split("\n")
    lines = [l for l in lines if l]
    cols = max(len(TOKEN.sub(lambda m: m.group(2) or "", l)) for l in lines)
    img = Image.new("RGB", (cols * CW, len(lines) * CH), bg)
    d = ImageDraw.Draw(img)
    f, fb = font(), fallback_font()
    for y, line in enumerate(lines):
        x, fg, bg = 0, None, None
        for m in TOKEN.finditer(line):
            if m.group(1) is not None:
                fg, bg = sgr(m.group(1), fg, bg)
                continue
            ch = m.group(2)
            X, Y = x * CW, y * CH
            if bg:
                d.rectangle([X, Y, X + CW - 1, Y + CH - 1], fill=bg)
            col = fg or (200, 200, 200)
            o = ord(ch)
            if ch == "▌" and bg:
                d.rectangle([X, Y, X + CW // 2 - 1, Y + CH - 1], fill=col)
            elif ch == "▀":
                d.rectangle([X, Y, X + CW - 1, Y + CH // 2 - 1], fill=col)
            elif ch == "▄":
                d.rectangle([X, Y + CH // 2, X + CW - 1, Y + CH - 1], fill=col)
            elif ch in QUADS:
                for k in range(4):
                    if QUADS[ch] & (1 << k):
                        qx, qy = X + (k % 2) * CW // 2, Y + (k // 2) * CH // 2
                        d.rectangle([qx, qy, qx + CW // 2 - 1, qy + CH // 2 - 1], fill=col)
            elif 0x2800 <= o <= 0x28FF:
                bits = o - 0x2800
                for cx, cy, b in BRAILLE:
                    if bits & b:
                        px, py = X + 2 + cx * 4, Y + 2 + cy * 4
                        d.rectangle([px, py, px + 1, py + 1], fill=col)
            elif ch != " ":
                d.text((X, Y), ch, font=f if ord(ch) < 128 else fb, fill=col)
            x += 1
    img.save(dst)


if __name__ == "__main__":
    # optional third argument: terminal background as rrggbb, e.g. 300a24 for Ubuntu's default
    bg = tuple(int(sys.argv[3][i:i + 2], 16) for i in (0, 2, 4)) if len(sys.argv) > 3 else BG
    main(sys.argv[1], sys.argv[2], bg)
