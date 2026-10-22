#!/usr/bin/env python3
"""Compose the cosmoscope intro video: title, a terminal window with demo footage and
captions, an install card, and a soft synthesized ambient score.

    python3 tools/video/footage.py FOOTAGE_DIR
    python3 tools/video/make_video.py FOOTAGE_DIR OUT.mp4
"""
import math
import os
import subprocess
import sys
import tempfile
import wave

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H, FPS = 1920, 1080, 30
BG = (243, 241, 234)
INK = (25, 25, 23)
MUTED = (110, 106, 99)
CLAY = (204, 109, 76)
TERM_BG = (14, 16, 23)
BAR = (36, 38, 47)

SERIF = "/usr/share/fonts/truetype/noto/NotoSerifDisplay-Regular.ttf"
SANS = os.path.expanduser("~/.fonts/Inter.ttf")
MONO = "/usr/share/fonts/truetype/ubuntu/UbuntuSansMono[wght].ttf"

REPO = "github.com/phanminhtai1029/cosmoscope"
RAW = "https://raw.githubusercontent.com/phanminhtai1029/cosmoscope/main/install"


def font(path, size, weight=None):
    f = ImageFont.truetype(path, size)
    if weight:
        try:
            f.set_variation_by_name(weight)
        except Exception:
            pass
    return f


F_TITLE = font(SERIF, 150)
F_TAG = font(SANS, 40, "Light")
F_SMALL = font(SANS, 24, "Regular")
F_KICK = font(SANS, 21, "SemiBold")
F_CAP = font(SERIF, 54)
F_MONO = font(MONO, 30, "Regular")
F_MONO_S = font(MONO, 23, "Regular")
F_BAR = font(SANS, 17, "Medium")
F_CARD_H = font(SERIF, 72)
F_LABEL = font(SANS, 22, "Medium")


def ease_out(x):
    x = min(max(x, 0.0), 1.0)
    return 1 - (1 - x) ** 3


def ease_in_out(x):
    x = min(max(x, 0.0), 1.0)
    return 3 * x * x - 2 * x * x * x


def fade(f, start, length):
    return ease_out((f - start) / length)


# ---- the window ---------------------------------------------------------------------------
CW, CH = 1440, 810          # footage area
BAR_H = 40
WX = (W - CW) // 2
WY_FINAL = 196              # top of the title bar when in place
RADIUS = 18


def rounded_mask(w, h, r):
    m = Image.new("L", (w, h), 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, w - 1, h - 1], r, fill=255)
    return m


WIN_MASK = rounded_mask(CW, CH + BAR_H, RADIUS)
SHADOW = Image.new("RGBA", (CW + 160, CH + BAR_H + 160), (0, 0, 0, 0))
ImageDraw.Draw(SHADOW).rounded_rectangle([80, 95, 80 + CW, 95 + CH + BAR_H], RADIUS, fill=(40, 30, 20, 70))
SHADOW = SHADOW.filter(ImageFilter.GaussianBlur(34))


def window(content, title="cosmoscope"):
    """A dark terminal window around `content` (CW x CH), as RGBA."""
    win = Image.new("RGB", (CW, CH + BAR_H), BAR)
    d = ImageDraw.Draw(win)
    for k, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        cx = 26 + k * 24
        d.ellipse([cx - 7, BAR_H // 2 - 7, cx + 7, BAR_H // 2 + 7], fill=c)
    tw = d.textlength(title, font=F_BAR)
    d.text(((CW - tw) / 2, BAR_H / 2), title, font=F_BAR, fill=(150, 152, 165), anchor="lm")
    win.paste(content, (0, BAR_H))
    out = Image.new("RGBA", win.size)
    out.paste(win, (0, 0), WIN_MASK)
    return out


def place_window(canvas, win, y, alpha=1.0, scale=1.0):
    if scale != 1.0:
        win = win.resize((int(win.width * scale), int(win.height * scale)), Image.LANCZOS)
        shadow = SHADOW.resize((int(SHADOW.width * scale), int(SHADOW.height * scale)), Image.LANCZOS)
    else:
        shadow = SHADOW
    x = (W - win.width) // 2
    if alpha < 1.0:
        win = win.copy()
        win.putalpha(win.getchannel("A").point(lambda a: int(a * alpha)))
        shadow = shadow.copy()
        shadow.putalpha(shadow.getchannel("A").point(lambda a: int(a * alpha)))
    canvas.alpha_composite(shadow, (x - int(80 * scale), int(y) - int(80 * scale)))
    canvas.alpha_composite(win, (x, int(y)))


# ---- footage --------------------------------------------------------------------------------
class Footage:
    def __init__(self, root):
        self.root = root
        self.cache = {}

    def frame(self, seg, k):
        n = len(os.listdir(os.path.join(self.root, seg)))
        k = min(max(k, 0), n - 1)
        key = (seg, k)
        if key not in self.cache:
            if len(self.cache) > 40:
                self.cache.clear()
            im = Image.open(os.path.join(self.root, seg, f"{k:04d}.png")).convert("RGB")
            self.cache[key] = im.resize((CW, CH), Image.LANCZOS)
        return self.cache[key]

    def length(self, seg):
        return len(os.listdir(os.path.join(self.root, seg)))


# ---- timeline ---------------------------------------------------------------------------------
# (segment, start frame); each plays its full length, crossfading into the next
T_TITLE_END = 105
T_WIN_IN = 100
T_TYPE = 140
T_LAUNCH = 196
SEGMENTS = [("launch", T_LAUNCH), ("time", 376), ("earth", 526), ("saturn", 766), ("sun", 946), ("text", 1096)]
T_CARD = 1216
T_OUTRO = 1426
T_END = 1500
XFADE = 10

CAPTIONS = [
    (T_LAUNCH + 6, "COSMOSCOPE", "The Solar System, live in your terminal."),
    (376, "REAL POSITIONS", "Every planet where it really is. Then travel in time."),
    (526, "FLY IN", "Fly to any world, with live facts at your side."),
    (766, "REAL DATA", "Saturn's rings and shadows, from Cassini data."),
    (946, "ALWAYS MOVING", "A Sun that turns, boils and glows."),
    (1096, "ANY TERMINAL", "HD in modern terminals. Crisp text everywhere else."),
]


def caption_at(f):
    """(kicker, text, opacity, rise) for frame f."""
    current = None
    for i, (start, kick, text) in enumerate(CAPTIONS):
        if f >= start:
            current = (i, start, kick, text)
    if current is None or f >= T_CARD:
        out = 1.0 - fade(f, T_CARD - 2, 10) if current else 0.0
        if current is None:
            return None
        i, start, kick, text = current
        return kick, text, out, 0
    i, start, kick, text = current
    a = fade(f, start, 14)
    rise = (1 - a) * 12
    nxt = CAPTIONS[i + 1][0] if i + 1 < len(CAPTIONS) else T_CARD
    if f > nxt - 9:
        a *= max(0.0, (nxt - f) / 9)
    return kick, text, a, rise


def draw_caption(canvas, f):
    c = caption_at(f)
    if not c:
        return
    kick, text, a, rise = c
    if a <= 0:
        return
    layer = Image.new("RGBA", (W, 190), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    x = WX
    spaced = " ".join(kick)
    d.text((x, 70 - rise), spaced, font=F_KICK, fill=CLAY + (int(255 * a),))
    d.text((x, 102 - rise), text, font=F_CAP, fill=INK + (int(255 * a),))
    canvas.alpha_composite(layer, (0, 0))


def content_at(foot, f):
    """What the window shows at frame f."""
    if f < T_LAUNCH:
        return typing_screen(f)
    for i, (seg, start) in enumerate(SEGMENTS):
        end = start + foot.length(seg)
        nxt = SEGMENTS[i + 1][1] if i + 1 < len(SEGMENTS) else T_CARD
        if start <= f < nxt:
            img = foot.frame(seg, f - start)
            # crossfade into the next segment over its first frames
            if i + 1 < len(SEGMENTS) and f >= nxt - XFADE:
                nseg = SEGMENTS[i + 1][0]
                t = ease_in_out((f - (nxt - XFADE)) / XFADE)
                img = Image.blend(img, foot.frame(nseg, 0), t)
            return img
    seg, start = SEGMENTS[-1]
    return foot.frame(seg, f - start)


def typing_screen(f):
    img = Image.new("RGB", (CW, CH), TERM_BG)
    d = ImageDraw.Draw(img)
    cmd = "cosmoscope"
    typed = max(0, min(len(cmd), (f - T_TYPE) // 3)) if f >= T_TYPE else 0
    x, y = 36, 34
    d.text((x, y), "~", font=F_MONO, fill=(120, 170, 255))
    d.text((x + 34, y), "$", font=F_MONO, fill=CLAY)
    d.text((x + 68, y), cmd[:typed], font=F_MONO, fill=(228, 228, 236))
    cx = x + 68 + d.textlength(cmd[:typed], font=F_MONO)
    if (f // 15) % 2 == 0 or (T_TYPE <= f < T_TYPE + 3 * len(cmd)):
        d.rectangle([cx + 2, y + 4, cx + 16, y + 34], fill=(228, 228, 236))
    return img


# ---- title, card, outro -------------------------------------------------------------------------
def draw_title(canvas, f):
    d = ImageDraw.Draw(canvas)
    a = fade(f, 6, 26)
    out = 1.0 - fade(f, T_TITLE_END - 18, 16)
    alpha = a * out
    if alpha <= 0:
        return
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ld = ImageDraw.Draw(layer)
    rise = (1 - a) * 18
    ld.text((W / 2, 470 + rise), "cosmoscope", font=F_TITLE, fill=INK + (int(255 * alpha),), anchor="mm")
    a2 = fade(f, 26, 24) * out
    ld.text((W / 2, 590 + rise * 0.6), "The Solar System, live in your terminal.", font=F_TAG,
            fill=MUTED + (int(255 * a2),), anchor="mm")
    a3 = fade(f, 44, 24) * out
    ld.text((W / 2, 660), "Open source  ·  Rust  ·  Linux, macOS, Windows", font=F_SMALL,
            fill=MUTED + (int(200 * a3),), anchor="mm")
    # a small orbit mark above the title
    cx, cy = W / 2, 330
    ring = int(255 * a * out * 0.9)
    ld.ellipse([cx - 46, cy - 14, cx + 46, cy + 14], outline=CLAY + (ring,), width=3)
    ld.ellipse([cx - 13, cy - 13, cx + 13, cy + 13], fill=CLAY + (ring,))
    canvas.alpha_composite(layer)
    del d


INSTALL = [
    ("macOS · Linux", f"curl -fsSL {RAW}/install.sh | sh"),
    ("Windows", f"irm {RAW}/install.ps1 | iex"),
    ("Rust", f"cargo install --git https://{REPO}"),
]


def draw_card(canvas, f):
    a = fade(f, T_CARD + 8, 22) * (1.0 - fade(f, T_OUTRO - 14, 14))
    if a <= 0:
        return
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    rise = (1 - fade(f, T_CARD + 8, 22)) * 16
    x = 240
    d.text((x, 250 + rise), "O P E N   S O U R C E   ·   M I T", font=F_KICK, fill=CLAY + (int(255 * a),))
    d.text((x, 285 + rise), "Try it in one line.", font=F_CARD_H, fill=INK + (int(255 * a),))
    for k, (label, cmd) in enumerate(INSTALL):
        ak = fade(f, T_CARD + 24 + k * 8, 18) * (1.0 - fade(f, T_OUTRO - 14, 14))
        y = 440 + k * 118
        d.text((x, y), label, font=F_LABEL, fill=MUTED + (int(255 * ak),))
        box = [x, y + 36, W - 240, y + 36 + 58]
        d.rounded_rectangle(box, 12, fill=(31, 33, 41, int(255 * ak)))
        d.text((x + 24, y + 36 + 29), cmd, font=F_MONO_S, fill=(228, 228, 236, int(255 * ak)), anchor="lm")
    al = fade(f, T_CARD + 60, 18) * (1.0 - fade(f, T_OUTRO - 14, 14))
    d.text((x, 840), REPO, font=font(SANS, 30, "Medium"), fill=INK + (int(255 * al),))
    canvas.alpha_composite(layer)


def draw_outro(canvas, f):
    a = fade(f, T_OUTRO, 22)
    if a <= 0:
        return
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.text((W / 2, 500), "cosmoscope", font=F_TITLE, fill=INK + (int(255 * a),), anchor="mm")
    d.text((W / 2, 610), REPO, font=font(SANS, 32, "Regular"), fill=MUTED + (int(255 * a),), anchor="mm")
    canvas.alpha_composite(layer)


def frame(foot, f):
    canvas = Image.new("RGBA", (W, H), BG + (255,))
    if f < T_TITLE_END:
        draw_title(canvas, f)
    if T_WIN_IN <= f < T_CARD + 20:
        k = ease_out((f - T_WIN_IN) / 28)
        y = WY_FINAL + (1 - k) * 160
        alpha = k
        scale = 1.0
        if f >= T_CARD - 4:
            # the window steps back and fades to make room for the install card
            o = ease_in_out((f - (T_CARD - 4)) / 22)
            alpha *= 1 - o
            scale = 1 - 0.06 * o
            y += 20 * o
        if alpha > 0.01:
            place_window(canvas, window(content_at(foot, f)), y, alpha, scale)
        draw_caption(canvas, f)
    if T_CARD <= f < T_OUTRO:
        draw_card(canvas, f)
    if f >= T_OUTRO:
        draw_outro(canvas, f)
    # gentle fade to the background at the very end
    end = fade(f, T_END - 16, 16)
    out = canvas.convert("RGB")
    if end > 0:
        out = Image.blend(out, Image.new("RGB", (W, H), BG), end)
    return out


# ---- music ----------------------------------------------------------------------------------------
def score(path, seconds, rate=48000):
    """A soft ambient pad: slow chord changes, a light shimmer, long fades."""
    t = np.arange(int(seconds * rate)) / rate
    def hz(n):  # MIDI note to frequency
        return 440.0 * 2 ** ((n - 69) / 12)
    chords = [[45, 52, 55, 59, 60], [41, 48, 52, 55, 57], [48, 55, 59, 62, 64], [43, 50, 54, 57, 62]]
    seg = seconds / len(chords)
    left = np.zeros_like(t)
    right = np.zeros_like(t)
    for ci, chord in enumerate(chords):
        center = (ci + 0.5) * seg
        env = np.clip(1 - np.abs(t - center) / (seg * 0.75), 0, 1) ** 1.5
        for n in chord:
            f0 = hz(n)
            for h, amp in [(1, 1.0), (2, 0.25), (3, 0.08)]:
                det = 1.0015
                left += env * amp * np.sin(2 * np.pi * f0 * h * t / det) / len(chord)
                right += env * amp * np.sin(2 * np.pi * f0 * h * t * det + 0.7) / len(chord)
    # shimmer: soft bell notes from a pentatonic set
    rng = np.random.default_rng(7)
    bells = [69, 72, 74, 76, 79, 81, 84]
    for start in np.arange(2.5, seconds - 4, 1.7):
        n = rng.choice(bells)
        s = int(start * rate)
        tt = np.arange(int(2.5 * rate)) / rate
        tone = np.sin(2 * np.pi * hz(n) * tt) * np.exp(-tt * 2.2) * 0.12
        pan = rng.uniform(0.3, 0.7)
        e = min(len(t), s + len(tt))
        left[s:e] += tone[: e - s] * (1 - pan)
        right[s:e] += tone[: e - s] * pan
    trem = 0.92 + 0.08 * np.sin(2 * np.pi * 0.18 * t)
    master = np.clip(t / 2.5, 0, 1) * np.clip((seconds - t) / 3.0, 0, 1) * trem
    stereo = np.stack([left * master, right * master], axis=1)
    # one-pole low-pass to soften the edges
    alpha = 0.18
    for ch in range(2):
        x = stereo[:, ch]
        y = np.empty_like(x)
        acc = 0.0
        for i in range(0, len(x), 4800):
            block = x[i:i + 4800]
            out = np.empty_like(block)
            for j, v in enumerate(block):
                acc += alpha * (v - acc)
                out[j] = acc
            y[i:i + 4800] = out
        stereo[:, ch] = y
    stereo /= np.max(np.abs(stereo)) + 1e-9
    stereo *= 0.42
    pcm = (stereo * 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())


def main():
    footage, out = sys.argv[1], sys.argv[2]
    foot = Footage(footage)
    with tempfile.TemporaryDirectory() as tmp:
        wav = os.path.join(tmp, "score.wav")
        score(wav, T_END / FPS)
        ff = subprocess.Popen([
            "ffmpeg", "-loglevel", "error", "-y",
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
            "-i", wav,
            "-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "160k", "-shortest", "-movflags", "+faststart", out,
        ], stdin=subprocess.PIPE)
        for f in range(T_END):
            ff.stdin.write(frame(foot, f).tobytes())
            if f % 150 == 0:
                print(f"frame {f}/{T_END}", file=sys.stderr)
        ff.stdin.close()
        ff.wait()
    print(out, file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[3] == "--stills":
        foot = Footage(sys.argv[1])
        for f in [60, 150, 230, 300, 450, 640, 850, 1000, 1150, 1222, 1320, 1470]:
            frame(foot, f).save(os.path.join(sys.argv[2], f"still_{f:04d}.png"))
    else:
        main()
