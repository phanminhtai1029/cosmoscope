#!/usr/bin/env python3
"""Render the demo footage for the intro video, frame by frame, from cosmoscope --snapshot.

Every frame is a fresh, deterministic snapshot composited like a kitty-protocol terminal
would show it, so no screen recording is needed.

    cargo build --release && python3 tools/video/footage.py OUT_DIR
"""
import multiprocessing as mp
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import ansi2png  # noqa: E402

BINARY = os.path.join(os.path.dirname(os.path.dirname(HERE)), "target", "release", "cosmoscope")
CELL = (10, 20)
SIZE = "160x45"
FPS = 30
BG = (14, 16, 23)

# name: (cosmoscope arguments, first and last second of app time to film)
SEGMENTS = {
    "launch": (["--date", "2026-10-22"], 0.0, 6.0),
    "time": (["--date", "2026-10-22", "--speed", "year"], 3.0, 8.0),
    "earth": (["--tour", "--date", "2026-10-22"], 8.6, 16.6),
    "saturn": (["saturn", "--date", "2032-06-01"], 1.0, 7.0),
    "sun": (["sun", "--date", "2026-10-22"], 1.5, 6.5),
    "text": (["--date", "2026-10-22", "--graphics", "off"], 3.0, 7.0),
}


def render(job):
    name, args, t, out = job
    if os.path.exists(out):
        return out
    hd = "--graphics" not in args
    extra = ["--graphics", "kitty", "--cell", f"{CELL[0]}x{CELL[1]}"] if hd else []
    with tempfile.TemporaryDirectory() as tmp:
        ans = os.path.join(tmp, "f.ans")
        with open(ans, "wb") as f:
            subprocess.run([BINARY, *args, *extra, "--colors", "truecolor", "--snapshot", SIZE, "--at", f"{t:.4f}"],
                           stdout=f, check=True)
        if hd:
            subprocess.run([sys.executable, "-I", os.path.join(os.path.dirname(HERE), "kitty_sim.py"), ans, out,
                            "--cell", f"{CELL[0]}x{CELL[1]}", "--bg", "%02x%02x%02x" % BG],
                           check=True, stderr=subprocess.DEVNULL)
        else:
            ansi2png.CW, ansi2png.CH, ansi2png.FONT_SIZE = CELL[0], CELL[1], 16
            ansi2png.main(ans, out, BG)
    return out


def main():
    out_dir = sys.argv[1]
    ansi2png.FONTS.insert(0, "/usr/share/fonts/truetype/ubuntu/UbuntuSansMono[wght].ttf")
    jobs = []
    for name, (args, t0, t1) in SEGMENTS.items():
        os.makedirs(os.path.join(out_dir, name), exist_ok=True)
        n = int(round((t1 - t0) * FPS))
        for k in range(n):
            jobs.append((name, args, t0 + k / FPS, os.path.join(out_dir, name, f"{k:04d}.png")))
    with mp.Pool(max(1, os.cpu_count() - 2)) as pool:
        for i, _ in enumerate(pool.imap_unordered(render, jobs, chunksize=4)):
            if i % 100 == 0:
                print(f"{i}/{len(jobs)}", file=sys.stderr)
    print(f"done: {len(jobs)} frames", file=sys.stderr)


if __name__ == "__main__":
    main()
