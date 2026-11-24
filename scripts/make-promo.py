#!/usr/bin/env python3
"""Composes the promo video of VeloEdit.

The frames of the application are drawn by a test (a scripted tour of the real UI):

    ICED_TEST_BACKEND=wgpu cargo test --release promo_frames -- --ignored --nocapture

This script puts them on a stage and adds what a screen recording does not have: captions
(English and Turkish), key hints, a mouse pointer with click rings, a title card, a recap card
and a closing card. It needs Python 3 with Pillow, and ffmpeg with libx264.

    scripts/make-promo.py                      # both languages -> target/promo/veloedit-tour-{en,tr}.mp4
    scripts/make-promo.py --lang en            # one language
    scripts/make-promo.py --music track.mp3    # with music: cuts land on the track's hits (needs numpy)
    scripts/make-promo.py --stills 2,14.5,40   # PNGs of those moments (seconds) instead of a video
"""

import argparse
import functools
import json
import math
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONTS = Path("/usr/share/fonts/mozilla-fira")

CW, CH = 1920, 1080          # the video
FPS = 30
WIN = (28, 22, 1400, 1036)   # the application window on the stage: x, y, width, height
PANEL_X, PANEL_W = 1478, 410  # the caption column
ACCENT = (74, 163, 255)
WHITE = (240, 244, 250)
DIM = (140, 150, 168)
XFADE = 0.45                 # seconds of cross-fade between two parts

INTRO_S, RECAP_S, OUTRO_S = 3.4, 4.2, 5.2

TEXT = {
    "en": {
        "restart": "Quit and open again\u2026",
        "tagline": "A fast, GPU-accelerated text editor",
        "sub": "Written in Rust  ·  Linux and Windows",
        "recap_title": "And much more",
        "chips": [
            "Hot exit: nothing unsaved is lost", "One window for every file", "Git marks in the gutter, colors in the sidebar",
            "Safe on network mounts", "Drag tabs to reorder", "External change detection", "Preview tabs",
            "Light and dark themes", "50+ languages", "Trim trailing whitespace on save", "Hex editor", "Image viewer",
        ],
        "outro_title": "Try VeloEdit",
        "outro_free": "Free and open source  ·  MIT license",
        "outro_linux": "Linux",
        "outro_linux_what": ".deb  ·  .rpm  ·  .AppImage  ·  tarball",
        "outro_win": "Windows (preview)",
        "outro_win_what": ".zip, no installer",
    },
    "tr": {
        "restart": "Kapat ve yeniden aç\u2026",
        "tagline": "Hızlı, GPU hızlandırmalı bir metin editörü",
        "sub": "Rust ile yazıldı  ·  Linux ve Windows",
        "recap_title": "Ve daha fazlası",
        "chips": [
            "Sıcak çıkış: kaydedilmemiş bir şey kaybolmaz", "Her dosya için tek pencere", "Kenarda ve kenar çubuğunda git renkleri",
            "Ağ bağlantılı disklerde güvenli", "Sekmeleri sürükleyerek sırala", "Dış değişiklik algılama", "Önizleme sekmeleri",
            "Açık ve koyu temalar", "50+ dil", "Kaydederken satır sonu boşluklarını sil", "Hex editörü", "Resim görüntüleyici",
        ],
        "outro_title": "VeloEdit'i dene",
        "outro_free": "Ücretsiz ve açık kaynak  ·  MIT lisansı",
        "outro_linux": "Linux",
        "outro_linux_what": ".deb  ·  .rpm  ·  .AppImage  ·  tarball",
        "outro_win": "Windows (ön sürüm)",
        "outro_win_what": ".zip, kurulum gerektirmez",
    },
}


# ------------------------------------------------------------------ small helpers

@functools.lru_cache(maxsize=None)
def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / name), size)


def sans(size, weight="Medium"):
    return font(f"FiraSans-{weight}.otf", size)


def mono(size):
    return font("FiraMono-Medium.otf", size)


def ease(x: float) -> float:
    x = max(0.0, min(1.0, x))
    return x * x * (3 - 2 * x)


def clamp01(x: float) -> float:
    return max(0.0, min(1.0, x))


def wrap(draw: ImageDraw.ImageDraw, text: str, fnt, width: int) -> list[str]:
    lines, line = [], ""
    for word in text.split(" "):
        trial = f"{line} {word}".strip()
        if line and draw.textlength(trial, font=fnt) > width:
            lines.append(line)
            line = word
        else:
            line = trial
    if line:
        lines.append(line)
    return lines


def with_alpha(layer: Image.Image, factor: float) -> Image.Image:
    if factor >= 0.999:
        return layer
    out = layer.copy()
    alpha = out.getchannel("A").point(lambda v: int(v * factor))
    out.putalpha(alpha)
    return out


def rounded(size, radius):
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius=radius, fill=255)
    return mask


# ----------------------------------------------------------------------- the stage

@functools.lru_cache(maxsize=None)
def stage() -> Image.Image:
    """Backdrop with a soft glow and the window's shadow (the window itself is pasted per frame)."""
    top, bottom = (11, 13, 19), (24, 29, 41)
    ramp = Image.linear_gradient("L").resize((CW, CH))
    bg = Image.composite(Image.new("RGB", (CW, CH), bottom), Image.new("RGB", (CW, CH), top), ramp).convert("RGBA")
    glow = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gd.ellipse((-200, 300, 900, 1300), fill=(*ACCENT, 38))
    gd.ellipse((1100, -400, 2300, 500), fill=(120, 90, 255, 26))
    bg.alpha_composite(glow.filter(ImageFilter.GaussianBlur(170)))
    x, y, w, h = WIN
    shadow = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((x, y + 16, x + w, y + h + 16), radius=20, fill=(0, 0, 0, 200))
    bg.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(30)))
    # A hairline around the window.
    ImageDraw.Draw(bg).rounded_rectangle((x - 1, y - 1, x + w, y + h), radius=19, outline=(52, 60, 78, 255), width=2)
    # The brand at the top of the caption column, the address at its bottom.
    icon = Image.open(ROOT / "docs/icon-256.png").convert("RGBA").resize((64, 64), Image.LANCZOS)
    bg.alpha_composite(icon, (PANEL_X, 34))
    d = ImageDraw.Draw(bg)
    d.text((PANEL_X + 80, 36), "VeloEdit", font=sans(40, "SemiBold"), fill=WHITE)
    d.line((PANEL_X, 124, PANEL_X + PANEL_W, 124), fill=(48, 56, 72, 255), width=2)
    d.text((PANEL_X, CH - 62), "github.com/tkinali/veloedit", font=sans(24, "Regular"), fill=DIM)
    return bg


@functools.lru_cache(maxsize=12)
def window_image(path: str) -> Image.Image:
    x, y, w, h = WIN
    im = Image.open(path).convert("RGB").resize((w, h), Image.LANCZOS).convert("RGBA")
    im.putalpha(rounded((w, h), 18))
    return im


# ----------------------------------------------------------------------- mouse pointer

@functools.lru_cache(maxsize=None)
def pointer_sprite() -> Image.Image:
    s = 4
    pts = [(0, 0), (0, 29), (7.5, 22.5), (13, 35), (18.5, 32.5), (13, 21), (22.5, 21)]
    big = Image.new("RGBA", (34 * s, 42 * s), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    d.polygon([(x * s + 3 * s, y * s + 3 * s) for x, y in pts], fill=(255, 255, 255, 255), outline=(15, 17, 22, 255), width=3 * s // 2)
    shadow = big.filter(ImageFilter.GaussianBlur(2 * s))
    out = Image.new("RGBA", big.size, (0, 0, 0, 0))
    shade = Image.new("RGBA", big.size, (0, 0, 0, 110))
    out.paste(shade, (2 * s, 3 * s), shadow.getchannel("A"))
    out.alpha_composite(big)
    return out.resize((34 * 5 // 4 * 1, 42 * 5 // 4 * 1), Image.LANCZOS)


def window_point(x: float, y: float) -> tuple[float, float]:
    """Logical window coordinates (as the app saw them) -> stage pixels."""
    k = WIN[2] / (2.0 * LOGICAL[0])
    return WIN[0] + x * 2.0 * k, WIN[1] + y * 2.0 * k


def pointer_state(scene: dict, t: float):
    """`(x, y, visible)` of the pointer in logical coordinates at scene time `t` (ms)."""
    moves = scene["pointer"]
    if not moves:
        return None
    first = moves[0]
    prev = (first["x"] + 150, first["y"] + 95)
    if t < first["t0"]:
        return None
    pos = prev
    for mv in moves:
        target = (mv["x"], mv["y"])
        if t >= mv["t1"]:
            pos = target
            continue
        if t >= mv["t0"]:
            k = ease((t - mv["t0"]) / max(1.0, mv["t1"] - mv["t0"]))
            pos = (pos[0] + (target[0] - pos[0]) * k, pos[1] + (target[1] - pos[1]) * k)
        break
    appear = clamp01((t - first["t0"]) / 220.0)
    return pos[0], pos[1], appear


# ------------------------------------------------------------------ caption column

@functools.lru_cache(maxsize=None)
def caption_layer(text: str) -> Image.Image:
    layer = Image.new("RGBA", (PANEL_W, 520), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    fnt = sans(44, "SemiBold")
    lines = wrap(d, text, fnt, PANEL_W - 6)
    y = 0
    for line in lines:
        d.text((0, y), line, font=fnt, fill=WHITE)
        y += 58
    layer.info["height"] = y
    return layer


@functools.lru_cache(maxsize=None)
def progress_layer(index: int, total: int) -> Image.Image:
    layer = Image.new("RGBA", (PANEL_W, 24), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    step = min(26, (PANEL_W - 14) // max(1, total))
    for i in range(total):
        cx = 7 + i * step
        if i == index:
            d.rounded_rectangle((cx - 7, 5, cx + 17, 19), radius=7, fill=(*ACCENT, 255))
        else:
            d.ellipse((cx - 5, 7, cx + 5, 17), fill=(70, 80, 100, 255) if i > index else (110, 124, 150, 255))
    return layer


@functools.lru_cache(maxsize=None)
def keycap(label: str, pressed: bool) -> Image.Image:
    f = sans(34, "Medium")
    d = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
    w = int(d.textlength(label, font=f)) + 44
    h = 62
    img = Image.new("RGBA", (w, h + 10), (0, 0, 0, 0))
    g = ImageDraw.Draw(img)
    lift = 6 if not pressed else 2
    g.rounded_rectangle((0, lift, w - 1, lift + h - 6), radius=11, fill=(14, 17, 24, 255))
    top = (58, 68, 90, 255) if pressed else (42, 50, 68, 255)
    edge = (*ACCENT, 255) if pressed else (96, 108, 134, 255)
    g.rounded_rectangle((0, 0 if not pressed else 4, w - 1, h - 10 + (4 if pressed else 0)), radius=11, fill=top, outline=edge, width=2)
    tw = g.textlength(label, font=f)
    g.text(((w - tw) / 2, (0 if not pressed else 4) + 8), label, font=f, fill=WHITE)
    return img


def key_row(label: str, pressed: bool) -> Image.Image:
    parts = [p for p in label.split("+")] if "+" in label and len(label) > 1 else [label]
    caps = [keycap(p, pressed) for p in parts]
    plus = 30
    width = sum(c.width for c in caps) + plus * (len(caps) - 1)
    row = Image.new("RGBA", (width, 80), (0, 0, 0, 0))
    x = 0
    d = ImageDraw.Draw(row)
    for i, c in enumerate(caps):
        row.alpha_composite(c, (x, 0))
        x += c.width
        if i < len(caps) - 1:
            d.text((x + 8, 14), "+", font=sans(32, "Regular"), fill=DIM)
            x += plus
    return row


# ---------------------------------------------------------------------- the scenes

LOGICAL = (1000.0, 740.0)


def frame_at(scene: dict, t: float) -> str:
    acc = 0.0
    for name, ms in scene["frames"]:
        acc += ms
        if t < acc:
            return name
    return scene["frames"][-1][0]


def compose_scene(scene: dict, index: int, total: int, t: float, lang: str, frames: Path) -> Image.Image:
    img = stage().copy()
    x, y, _, _ = WIN
    img.alpha_composite(window_image(str(frames / frame_at(scene, t))), (x, y))

    # Caption: slides in, holds, fades out.
    length = scene["length"]
    fade_in = ease(t / 380.0)
    fade_out = ease((length - t) / 320.0)
    a = min(fade_in, fade_out)
    cap = caption_layer(scene[lang])
    height = cap.info.get("height", 200)
    cy = 214 + int((1 - fade_in) * 18)
    img.alpha_composite(with_alpha(cap, a), (PANEL_X, cy))

    # Key hints: the newest one that is active.
    active = [k for k in scene["keys"] if k["t"] <= t < k["t"] + k["dur"]]
    if active:
        k = max(active, key=lambda k: k["t"])
        age = t - k["t"]
        row = key_row(k["label"], pressed=age < 230)
        ka = min(ease(age / 160.0), ease((k["t"] + k["dur"] - t) / 220.0))
        img.alpha_composite(with_alpha(row, ka), (PANEL_X, cy + height + 44))

    img.alpha_composite(progress_layer(index, total), (PANEL_X, CH - 118))

    # The window is closed and opened again: it goes dark for a moment.
    for cut in scene.get("cuts", []):
        dark = ease((t - (cut["t0"] - 260)) / 260.0) * (1 - ease((t - cut["t1"]) / 380.0))
        if dark > 0.001:
            veil = Image.new("RGBA", (WIN[2], WIN[3]), (8, 10, 14, 255))
            veil.putalpha(rounded((WIN[2], WIN[3]), 18))
            d = ImageDraw.Draw(veil)
            label = TEXT[lang]["restart"]
            f = sans(54, "SemiBold")
            w = d.textlength(label, font=f)
            d.text(((WIN[2] - w) / 2, WIN[3] / 2 - 40), label, font=f, fill=WHITE)
            img.alpha_composite(with_alpha(veil, dark), (WIN[0], WIN[1]))

    # Pointer and click rings.
    state = pointer_state(scene, t)
    if state:
        px, py, appear = state
        sx, sy = window_point(px, py)
        for c in scene["clicks"]:
            age = t - c
            if 0 <= age < 520:
                k = age / 520.0
                r = 14 + 40 * ease(k)
                ring = Image.new("RGBA", (200, 200), (0, 0, 0, 0))
                ImageDraw.Draw(ring).ellipse((100 - r, 100 - r, 100 + r, 100 + r), outline=(*ACCENT, int(230 * (1 - k))), width=5)
                img.alpha_composite(ring, (int(sx) - 100, int(sy) - 100))
        sprite = pointer_sprite()
        down = any(0 <= t - c < 130 for c in scene["clicks"])
        if down:
            sprite = sprite.resize((int(sprite.width * 0.88), int(sprite.height * 0.88)), Image.LANCZOS)
        img.alpha_composite(with_alpha(sprite, appear), (int(sx) - 3, int(sy) - 3))
    return img


# ---------------------------------------------------------------------------- cards

def card_base() -> Image.Image:
    # Cards use the whole stage: no window, no column.
    top, bottom = (11, 13, 19), (24, 29, 41)
    ramp = Image.linear_gradient("L").resize((CW, CH))
    plain = Image.composite(Image.new("RGB", (CW, CH), bottom), Image.new("RGB", (CW, CH), top), ramp).convert("RGBA")
    glow = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse((560, 220, 1360, 860), fill=(*ACCENT, 46))
    plain.alpha_composite(glow.filter(ImageFilter.GaussianBlur(190)))
    return plain


def centered(d: ImageDraw.ImageDraw, y: int, text: str, fnt, fill, cx=CW // 2):
    w = d.textlength(text, font=fnt)
    d.text((cx - w / 2, y), text, font=fnt, fill=fill)


def intro_card(t: float, lang: str) -> Image.Image:
    img = card_base()
    k = ease(t / 0.9)
    icon = Image.open(ROOT / "docs/icon-256.png").convert("RGBA")
    size = int(230 + 20 * (1 - k))
    icon = icon.resize((size, size), Image.LANCZOS)
    layer = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    layer.alpha_composite(icon, (CW // 2 - size // 2, 190))
    d = ImageDraw.Draw(layer)
    centered(d, 470, "VeloEdit", sans(132, "SemiBold"), WHITE)
    centered(d, 640, TEXT[lang]["tagline"], sans(46, "Regular"), (200, 208, 224))
    centered(d, 724, TEXT[lang]["sub"], sans(32, "Regular"), DIM)
    img.alpha_composite(with_alpha(layer, k))
    return img


def recap_card(t: float, lang: str) -> Image.Image:
    img = card_base()
    layer = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    centered(d, 170, TEXT[lang]["recap_title"], sans(84, "SemiBold"), WHITE)
    chips = TEXT[lang]["chips"]
    f = sans(34, "Medium")
    rows, row, width = [], [], 0
    for c in chips:
        w = int(d.textlength(c, font=f)) + 56
        if width + w > 1560 and row:
            rows.append(row)
            row, width = [], 0
        row.append((c, w))
        width += w + 22
    rows.append(row)
    y = 360
    n = 0
    for row in rows:
        total = sum(w for _, w in row) + 22 * (len(row) - 1)
        x = CW // 2 - total // 2
        for text, w in row:
            appear = ease((t - 0.25 - n * 0.16) / 0.45)
            chip = Image.new("RGBA", (w, 76), (0, 0, 0, 0))
            cd = ImageDraw.Draw(chip)
            cd.rounded_rectangle((0, 0, w - 1, 75), radius=38, fill=(34, 42, 58, 255), outline=(*ACCENT, 255), width=2)
            cd.text((28, 15), text, font=f, fill=WHITE)
            dy = int((1 - appear) * 22)
            layer.alpha_composite(with_alpha(chip, appear), (x, y + dy))
            x += w + 22
            n += 1
        y += 120
    img.alpha_composite(layer)
    return img


def outro_card(t: float, lang: str) -> Image.Image:
    img = card_base()
    k = ease(t / 0.8)
    layer = Image.new("RGBA", (CW, CH), (0, 0, 0, 0))
    icon = Image.open(ROOT / "docs/icon-256.png").convert("RGBA").resize((150, 150), Image.LANCZOS)
    layer.alpha_composite(icon, (CW // 2 - 75, 110))
    d = ImageDraw.Draw(layer)
    centered(d, 290, TEXT[lang]["outro_title"], sans(100, "SemiBold"), WHITE)
    centered(d, 424, TEXT[lang]["outro_free"], sans(36, "Regular"), (200, 208, 224))
    box_w, box_h, gap = 560, 190, 50
    for i, (name, what) in enumerate([(TEXT[lang]["outro_linux"], TEXT[lang]["outro_linux_what"]), (TEXT[lang]["outro_win"], TEXT[lang]["outro_win_what"])]):
        bx = CW // 2 - box_w - gap // 2 + i * (box_w + gap)
        d.rounded_rectangle((bx, 500, bx + box_w, 500 + box_h), radius=24, fill=(30, 37, 52, 255), outline=(62, 72, 94, 255), width=2)
        centered(d, 530, name, sans(46, "SemiBold"), WHITE, cx=bx + box_w // 2)
        centered(d, 606, what, sans(30, "Regular"), (190, 200, 218), cx=bx + box_w // 2)
    centered(d, 770, "github.com/tkinali/veloedit", sans(64, "SemiBold"), ACCENT)
    centered(d, 860, "tkinali.github.io/veloedit", sans(36, "Regular"), DIM)
    img.alpha_composite(with_alpha(layer, k))
    return img


# ---------------------------------------------------------------------------- music

def analyze_music(path: str):
    """`(duration, onsets)`: the track's length and its strong low-band hits `[(seconds, strength)]`.

    A kick/bass onset detector (low-passed audio, rise of the log energy) is enough to find where
    the music "hits"; the cuts of the video are moved onto those moments."""
    import numpy as np  # only needed with --music

    sr, hop = 11025, 64
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-ac", "1", "-ar", str(sr), "-af", "lowpass=f=150", "-f", "f32le", "-"],
        capture_output=True, check=True,
    ).stdout
    x = np.frombuffer(raw, dtype=np.float32)
    n = len(x) // hop
    env = np.sqrt(np.mean(x[: n * hop].reshape(n, hop) ** 2, axis=1))
    le = np.log(env + 1e-4)
    on = np.maximum(0, np.diff(le, prepend=le[0]))
    fps = sr / hop
    out = []
    for i in np.argsort(on)[::-1]:
        if on[i] < 0.35 * on.max():
            break
        t = i / fps
        if all(abs(t - u) > 0.25 for u, _ in out):
            out.append((t, float(on[i])))
    duration = float(subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path], capture_output=True, text=True, check=True
    ).stdout)
    return duration, sorted(out)


def snap_cuts(parts_len, onsets, hold_min, window=0.7):
    """Moves each cut (the middle of a cross-fade) onto the nearest strong hit of the music, within
    `window` seconds, by lengthening or shortening the part before it. `hold_min[i]` is how much of
    the end of part `i` may be cut away. Returns the new lengths."""
    times = [t for t, _ in onsets]
    lens = list(parts_len)
    start = 0.0
    for i in range(len(lens) - 1):
        cut = start + lens[i] - XFADE / 2
        best = None
        for t in times:
            delta = t - cut
            if -min(window, hold_min[i]) <= delta <= window and (best is None or abs(delta) < abs(best)):
                best = delta
        if best is not None:
            lens[i] += best
        start += lens[i] - XFADE
    return lens


# -------------------------------------------------------------------------- timeline

def part_starts(lens):
    starts, at = [], 0.0
    for length in lens:
        starts.append(at)
        at += length - XFADE
    return starts, at + XFADE


def build_timeline(manifest: dict, lang: str, frames: Path, music=None):
    """`(parts, starts, total)`: `parts` is a list of `(length_seconds, renderer(t_seconds))`.

    With `music` (`analyze_music`'s result) the cuts between the parts are moved onto the strong
    hits of the track, and the closing card comes in on its last big hit."""
    scenes = [dict(sc) for sc in manifest["scenes"]]
    lens = [INTRO_S] + [sc["length"] / 1000.0 for sc in scenes] + [RECAP_S, OUTRO_S]
    if music:
        duration, onsets = music
        # How much of the end of each part can go without cutting into what happens in it.
        holds = [0.4] + [max(0.0, sc["frames"][-1][1] / 1000.0 - 0.25) for sc in scenes] + [0.8, 0.0]
        lens = snap_cuts(lens, onsets, holds)
        late = [(strength, t) for t, strength in onsets if duration - 14 <= t <= duration - 3]
        if late:
            hit = max(late)[1]
            starts, _ = part_starts(lens)
            r = len(lens) - 2
            wanted = hit - starts[r] + XFADE / 2
            if 3.0 <= wanted <= 7.0:
                lens[r] = wanted
            starts, _ = part_starts(lens)
            # The video ends a moment after the music's last hit (the rest of the track is silence).
            end = min(duration - 0.2, onsets[-1][0] + 2.6)
            lens[-1] = min(9.0, max(4.5, end - starts[-1]))
        for sc, length in zip(scenes, lens[1:-2]):
            sc["length"] = length * 1000.0
    parts = [(lens[0], lambda t: intro_card(t, lang))]
    for i, sc in enumerate(scenes):
        parts.append((lens[i + 1], lambda t, sc=sc, i=i: compose_scene(sc, i, len(scenes), t * 1000.0, lang, frames)))
    parts.append((lens[-2], lambda t: recap_card(t, lang)))
    parts.append((lens[-1], lambda t: outro_card(t, lang)))
    starts, total = part_starts(lens)
    return parts, starts, total


def render(parts, starts, T: float) -> Image.Image:
    live = [i for i, (length, _) in enumerate(parts) if starts[i] <= T < starts[i] + length + 1e-9]
    if not live:
        live = [len(parts) - 1]
    if len(live) == 1:
        i = live[0]
        return parts[i][1](min(T - starts[i], parts[i][0]))
    a, b = live[0], live[-1]
    k = ease((T - starts[b]) / XFADE)
    first = parts[a][1](min(T - starts[a], parts[a][0]))
    second = parts[b][1](max(0.0, T - starts[b]))
    return Image.blend(first, second, k)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--frames", default=str(ROOT / "target/promo/frames"))
    ap.add_argument("--out", default=str(ROOT / "target/promo"))
    ap.add_argument("--lang", default="both", choices=["en", "tr", "both"])
    ap.add_argument("--stills", help="comma separated seconds: write PNGs instead of a video")
    ap.add_argument("--crf", default="18")
    ap.add_argument("--music", help="an audio file for the background (the cuts follow its hits)")
    ap.add_argument("--music-gain", default="-6", help="dB applied to the music (a loud master needs about -6)")
    args = ap.parse_args()

    frames = Path(args.frames)
    manifest = json.loads((frames / "manifest.json").read_text())
    global LOGICAL
    LOGICAL = tuple(manifest["window"])
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    music = None
    if args.music:
        music = analyze_music(args.music)
        print(f"music: {music[0]:.1f} s, {len(music[1])} strong hits", flush=True)
    for lang in (["en", "tr"] if args.lang == "both" else [args.lang]):
        parts, starts, total = build_timeline(manifest, lang, frames, music)
        if args.stills:
            for s in args.stills.split(","):
                T = float(s)
                path = out / f"still-{lang}-{T:07.2f}.png"
                render(parts, starts, T).convert("RGB").save(path)
                print(path)
            continue
        target = out / f"veloedit-tour-{lang}{'-music' if music else ''}.mp4"
        if music and total > music[0] + 0.05:
            # (`-shortest` would end the video with the track and drop the closing cards.)
            sys.exit(f"the tour ({total:.1f} s) is longer than the music ({music[0]:.1f} s): shorten a scene")
        n = int(total * FPS)
        print(f"{lang}: {total:.1f} s, {n} frames -> {target}", flush=True)
        audio = ["-an"]
        music_in = []
        if music:
            fade = 1.4
            music_in = ["-i", args.music]
            audio = [
                "-map", "0:v", "-map", "1:a", "-map_chapters", "-1", "-dn", "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-shortest",
                "-af", f"volume={args.music_gain}dB,afade=t=in:st=0:d=0.25,afade=t=out:st={total - fade:.3f}:d={fade},alimiter=limit=0.89",
            ]
        ff = subprocess.Popen(
            [
                "ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{CW}x{CH}", "-r", str(FPS),
                "-i", "-", *music_in, *audio, "-c:v", "libx264", "-preset", "slow", "-crf", args.crf, "-pix_fmt", "yuv420p",
                "-movflags", "+faststart", str(target),
            ],
            stdin=subprocess.PIPE,
        )
        for k in range(n):
            ff.stdin.write(render(parts, starts, k / FPS).convert("RGB").tobytes())
            if k % 300 == 0:
                print(f"  {k}/{n}", flush=True)
        ff.stdin.close()
        if ff.wait() != 0:
            sys.exit("ffmpeg failed")
        print(f"  wrote {target} ({target.stat().st_size / 1e6:.1f} MB)")


if __name__ == "__main__":
    main()
