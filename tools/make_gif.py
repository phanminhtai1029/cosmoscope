#!/usr/bin/env python3
"""Build assets/preview.gif from --tour snapshots (needs Pillow).

    cargo build --release && python3 tools/make_gif.py
"""
import argparse, os, subprocess, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from PIL import Image  # noqa: E402
import ansi2png  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default="target/release/cosmoscope")
    ap.add_argument("--size", default="120x34")
    ap.add_argument("--date", default="2026-10-22")
    ap.add_argument("--start", type=float, default=2.0)
    ap.add_argument("--end", type=float, default=36.0)
    ap.add_argument("--step", type=float, default=0.3, help="seconds of animation between frames")
    ap.add_argument("--fps", type=int, default=10)
    ap.add_argument("--scale", type=float, default=1.0)
    ap.add_argument("out", nargs="?", default="assets/preview.gif")
    a = ap.parse_args()
    frames, t = [], a.start
    with tempfile.TemporaryDirectory() as tmp:
        ans, png = os.path.join(tmp, "f.ans"), os.path.join(tmp, "f.png")
        while t <= a.end:
            with open(ans, "w") as f:
                subprocess.run([a.binary, "--tour", "--date", a.date, "--colors", "truecolor",
                                "--snapshot", a.size, "--at", f"{t:.3f}"], stdout=f, check=True)
            ansi2png.main(ans, png)
            im = Image.open(png).convert("RGB")
            if a.scale != 1.0:
                im = im.resize((int(im.width * a.scale), int(im.height * a.scale)), Image.LANCZOS)
            frames.append(im.quantize(colors=128, method=Image.Quantize.MEDIANCUT))
            print(f"\rt={t:5.1f}s  frames={len(frames)}", end="", file=sys.stderr)
            t += a.step
    print(file=sys.stderr)
    frames[0].save(a.out, save_all=True, append_images=frames[1:], duration=int(1000 / a.fps), loop=0, optimize=True)


if __name__ == "__main__":
    main()
