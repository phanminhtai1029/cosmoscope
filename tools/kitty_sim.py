#!/usr/bin/env python3
"""Composite a `cosmoscope --graphics kitty --snapshot` dump the way a kitty-protocol terminal
would: text grid first, then every placed image (zlib RGBA, chunked base64) on top.

    cosmoscope --graphics kitty --cell 10x20 --snapshot 160x46 --at 6 > hd.ans
    python3 tools/kitty_sim.py hd.ans hd.png [--cell 10x20] [--bg 0b0c14]
"""
import argparse, base64, os, re, sys, tempfile, zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from PIL import Image  # noqa: E402
import ansi2png  # noqa: E402

CMD = re.compile(rb"\x1b\[(\d+);(\d+)H|\x1b_G([^;\x1b]*)(?:;([^\x1b]*))?\x1b\\")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--cell", default="10x20")
    ap.add_argument("--bg", default="0b0c14")
    a = ap.parse_args()
    cw, ch = map(int, a.cell.split("x"))
    bg = tuple(int(a.bg[i:i + 2], 16) for i in (0, 2, 4))
    raw = open(a.src, "rb").read()
    cut = raw.rfind(b"\n") + 1
    text, cmds = raw[:cut], raw[cut:]
    ansi2png.CW, ansi2png.CH, ansi2png.FONT_SIZE = cw, ch, int(ch * 0.8)
    with tempfile.NamedTemporaryFile(suffix=".ans", delete=False) as t:
        t.write(text)
    ansi2png.main(t.name, a.dst, bg)
    os.unlink(t.name)
    img = Image.open(a.dst).convert("RGBA")
    cursor, pending, keys, n, images = (1, 1), [], {}, 0, {}
    for m in CMD.finditer(cmds):
        if m.group(1):
            cursor = (int(m.group(1)), int(m.group(2)))
            continue
        k = dict(kv.split(b"=", 1) for kv in m.group(3).split(b",") if b"=" in kv)
        act = k.get(b"a", b"")
        if act == b"d":
            images.pop(int(k[b"i"]), None)
            continue
        if act == b"p":
            pic = images[int(k[b"i"])]
            sx, sy = int(k.get(b"x", 0)), int(k.get(b"y", 0))
            sw, sh = int(k.get(b"w", pic.width)), int(k.get(b"h", pic.height))
            pic = pic.crop((sx, sy, sx + sw, sy + sh))
            pos = ((cursor[1] - 1) * cw + int(k.get(b"X", 0)), (cursor[0] - 1) * ch + int(k.get(b"Y", 0)))
            img.alpha_composite(pic, pos)
            n += 1
            continue
        if act:
            keys, pending = k, []
        pending.append(m.group(4) or b"")
        if k.get(b"m", b"0") == b"0":
            data = base64.b64decode(b"".join(pending))
            if keys.get(b"o") == b"z":
                data = zlib.decompress(data)
            w, h = int(keys[b"s"]), int(keys[b"v"])
            images[int(keys[b"i"])] = Image.frombytes("RGBA", (w, h), data)
    img.convert("RGB").save(a.dst)
    print(f"composited {n} placements", file=sys.stderr)


if __name__ == "__main__":
    main()
