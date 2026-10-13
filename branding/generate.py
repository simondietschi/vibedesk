#!/usr/bin/env python3
"""Erzeugt alle Icons und Logos des Clients aus branding/icon.svg und branding/mark.svg.

Benötigt: rsvg-convert, ImageMagick (convert) und Pillow.
Aufruf aus dem Repo-Root: python3 branding/generate.py
"""
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BRANDING = os.path.join(ROOT, "branding")
CLIENT = os.path.join(ROOT, "client")
TMP = tempfile.mkdtemp(prefix="vibedesk-branding-")


def read(name):
    with open(os.path.join(BRANDING, name)) as f:
        return f.read()


def variant(name, svg):
    path = os.path.join(TMP, name)
    with open(path, "w") as f:
        f.write(svg)
    return path


ICON = os.path.join(BRANDING, "icon.svg")
# iOS und Android Legacy maskieren selbst, deshalb ohne abgerundete Ecken.
ICON_SQUARE = variant("icon-square.svg", read("icon.svg").replace('rx="228"', 'rx="0"'))
MARK_BLACK = os.path.join(BRANDING, "mark.svg")
MARK_WHITE = variant("mark-white.svg", read("mark.svg").replace("#000000", "#FFFFFF"))


def render(svg, size, out=None):
    out = out or os.path.join(TMP, f"{os.path.basename(svg)}-{size}.png")
    subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), svg, "-o", out], check=True)
    return out


def dest(*parts):
    path = os.path.join(CLIENT, *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    return path


def padded(svg, size, content):
    """Rendert svg mit Kantenlänge content zentriert auf eine transparente Fläche size."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    inner = Image.open(render(svg, content))
    off = (size - content) // 2
    img.paste(inner, (off, off), inner)
    return img


def circle(svg, size):
    img = Image.open(render(svg, size)).convert("RGBA")
    mask = Image.new("L", (size * 4, size * 4), 0)
    from PIL import ImageDraw

    ImageDraw.Draw(mask).ellipse((0, 0, size * 4 - 1, size * 4 - 1), fill=255)
    img.putalpha(mask.resize((size, size), Image.LANCZOS))
    return img


def ico(svg, sizes, out):
    pngs = [render(svg, s) for s in sizes]
    subprocess.run(["convert", *pngs, out], check=True)


def desktop():
    render(ICON, 1024, dest("res", "icon.png"))
    padded(ICON, 1024, 824).save(dest("res", "mac-icon.png"))
    render(ICON, 32, dest("res", "32x32.png"))
    render(ICON, 64, dest("res", "64x64.png"))
    render(ICON, 128, dest("res", "128x128.png"))
    render(ICON, 256, dest("res", "128x128@2x.png"))
    ico(ICON, [16, 24, 32, 48, 64, 128, 256], dest("res", "icon.ico"))
    ico(ICON, [16, 24, 32], dest("res", "tray-icon.ico"))
    padded(MARK_BLACK, 60, 48).save(dest("res", "mac-tray-dark-x2.png"))
    padded(MARK_WHITE, 48, 40).save(dest("res", "mac-tray-light-x2.png"))
    with open(dest("res", "scalable.svg"), "w") as f:
        f.write(read("icon.svg"))
    with open(dest("flutter", "assets", "icon.svg"), "w") as f:
        f.write(read("icon.svg"))
    ico(ICON, [16, 24, 32, 48, 64, 128, 256], dest("flutter", "windows", "runner", "resources", "app_icon.ico"))


def macos():
    img = padded(ICON, 1024, 824)
    img.save(dest("flutter", "macos", "Runner", "AppIcon.icns"), sizes=[(16, 16), (32, 32), (128, 128), (256, 256), (512, 512)])


def ios():
    base = dest("flutter", "ios", "Runner", "Assets.xcassets", "AppIcon.appiconset", "x")
    folder = os.path.dirname(base)
    for name in os.listdir(folder):
        m = re.fullmatch(r"Icon-App-([\d.]+)x[\d.]+@(\d)x\.png", name)
        if not m:
            continue
        px = round(float(m.group(1)) * int(m.group(2)))
        Image.open(render(ICON_SQUARE, px)).convert("RGB").save(os.path.join(folder, name))


def android():
    res = dest("flutter", "android", "app", "src", "main", "res", "x")
    res = os.path.dirname(res)
    densities = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}
    for name, f in densities.items():
        d = os.path.join(res, f"mipmap-{name}")
        os.makedirs(d, exist_ok=True)
        launcher = round(48 * f)
        Image.open(render(ICON_SQUARE, launcher)).save(os.path.join(d, "ic_launcher.png"))
        circle(ICON_SQUARE, launcher).save(os.path.join(d, "ic_launcher_round.png"))
        # Adaptive Icons: 108dp Fläche, sichtbarer Bereich 72dp, Symbol in der 66dp-Schutzzone.
        fg = round(108 * f)
        padded(MARK_WHITE, fg, round(56 * f)).save(os.path.join(d, "ic_launcher_foreground.png"))
        stat = round(24 * f)
        padded(MARK_WHITE, stat, round(22 * f)).save(os.path.join(d, "ic_stat_logo.png"))
    fastlane = os.path.join(CLIENT, "fastlane", "metadata", "android", "en-US", "images", "icon.png")
    if os.path.isdir(os.path.dirname(fastlane)):
        render(ICON, 256, fastlane)


def logo_header():
    svg = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 287" width="1000" height="287">
  <g transform="translate(0,15.5) scale(0.25)">
{icon}
  </g>
  <text x="300" y="185" font-family="Inter, 'Segoe UI', Helvetica, Arial, sans-serif" font-size="150" font-weight="700" fill="#7C3AED">Vibe<tspan fill="#0891B2">Desk</tspan></text>
</svg>
"""
    inner = read("icon.svg")
    inner = inner[inner.index(">", inner.index("<svg")) + 1 : inner.rindex("</svg>")]
    with open(dest("res", "logo-header.svg"), "w") as f:
        f.write(svg.format(icon=inner))


def main():
    for tool in ("rsvg-convert", "convert"):
        if subprocess.run(["which", tool], capture_output=True).returncode != 0:
            sys.exit(f"{tool} fehlt")
    desktop()
    macos()
    ios()
    android()
    logo_header()
    print("Icons erzeugt in", CLIENT)


if __name__ == "__main__":
    main()
