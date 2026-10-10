"""Renders the VELA brand assets procedurally (no source artwork needed).

  build/icon.ico        Windows exe / installer icon (16-256 px)
  build/icon.png        1024 px master (macOS .icns and Linux packages)
  build/icon-64.rgba    raw 64x64 RGBA window icon embedded on macOS/Linux
  build/wordmark.png    VELA wordmark (README / store listings)

The same geometry is drawn live in the app by src/ui.rs (logo_mark/wordmark).
usage: python scripts/brand.py
"""
import math
import os

from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'build')
SS = 4  # supersampling


def tapered_arc(draw, pts, max_w, color, alpha_in=0.35):
    """Draw a polyline whose width swells from 0 to max_w and back to 0, fading in."""
    n = len(pts)
    for i in range(n - 1):
        t = i / (n - 1)
        w = max_w * math.sin(math.pi * min(1.0, t * 1.15)) ** 0.8
        a = int(255 * min(1.0, t / alpha_in))
        (x0, y0), (x1, y1) = pts[i], pts[i + 1]
        dx, dy = x1 - x0, y1 - y0
        L = math.hypot(dx, dy) or 1
        nx, ny = -dy / L * w / 2, dx / L * w / 2
        draw.polygon([(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)], fill=color + (a,))


def bezier(p0, p1, p2, p3, n=400):
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append((u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                    u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1]))
    return out


# Shared geometry (unit square). Keep in sync with ui::logo_mark.
TRI = [(0.385, 0.315), (0.385, 0.685), (0.700, 0.500)]
SWOOSH = [(0.14, 0.80), (0.40, 0.92), (0.80, 0.80), (0.90, 0.22)]


def icon(size):
    S = size * SS
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    # tile with vertical gradient
    tile = Image.new('RGBA', (S, S))
    td = ImageDraw.Draw(tile)
    for y in range(S):
        k = y / S
        c = (int(5 + 7 * k), int(7 + 9 * k), int(11 + 14 * k), 255)
        td.line([(0, y), (S, y)], fill=c)
    # earth-limb glow, lower right
    glow = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse([S * 0.35, S * 0.62, S * 1.45, S * 1.55], fill=(40, 110, 220, 95))
    glow = glow.filter(ImageFilter.GaussianBlur(S * 0.09))
    tile = Image.alpha_composite(tile, glow)
    mask = Image.new('L', (S, S), 0)
    m = S * 0.04
    ImageDraw.Draw(mask).rounded_rectangle([m, m, S - m, S - m], radius=S * 0.225, fill=255)
    img.paste(tile, (0, 0), mask)

    # hairline rim for definition on dark taskbars
    rim = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(rim).rounded_rectangle([m, m, S - m, S - m], radius=S * 0.225, outline=(255, 255, 255, 34), width=max(1, int(S * 0.006)))
    img = Image.alpha_composite(img, rim)

    # trajectory swoosh (behind the triangle)
    sw = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    pts = [(x * S, y * S) for x, y in bezier(*SWOOSH)]
    tapered_arc(ImageDraw.Draw(sw), pts, S * 0.045, (235, 242, 255))
    # soft glow on the swoosh
    sw_glow = sw.filter(ImageFilter.GaussianBlur(S * 0.012))
    img = Image.alpha_composite(img, sw_glow)
    img = Image.alpha_composite(img, sw)

    # knock-out gap around the triangle, then the triangle itself
    tri = [(x * S, y * S) for x, y in TRI]
    gap = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    gd = ImageDraw.Draw(gap)
    cx = sum(p[0] for p in tri) / 3
    cy = sum(p[1] for p in tri) / 3
    grow = 1.16
    gd.polygon([(cx + (x - cx) * grow, cy + (y - cy) * grow) for x, y in tri], fill=(7, 10, 16, 255))
    img = Image.alpha_composite(img, gap)
    ImageDraw.Draw(img).polygon(tri, fill=(255, 255, 255, 255))
    img = Image.composite(img, Image.new('RGBA', (S, S), (0, 0, 0, 0)), mask)
    return img.resize((size, size), Image.LANCZOS)


# ---- wordmark ------------------------------------------------------------
def wordmark(h=220, color=(255, 255, 255)):
    """Extended geometric 'VELΛ' with the trajectory swoosh off the Λ."""
    t = h * 0.15          # stroke weight
    w = h * 1.12          # extended letter width
    gap = h * 0.42
    W = int(4 * w + 3 * gap + h * 2.3)
    H = int(h * 2.0)
    S = SS
    img = Image.new('RGBA', (W * S, H * S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    oy = h * 0.6
    c = color + (255,)

    def P(x, y):
        return (x * S, (y + oy) * S)

    x = h * 0.2
    # V
    d.polygon([P(x, 0), P(x + t * 1.15, 0), P(x + w / 2 + t * 0.58, h), P(x + w / 2 - t * 0.58, h)], fill=c)
    d.polygon([P(x + w - t * 1.15, 0), P(x + w, 0), P(x + w / 2 + t * 0.58, h), P(x + w / 2 - t * 0.58, h)], fill=c)
    x += w + gap
    # E
    d.rectangle([P(x, 0), P(x + t, h)], fill=c)
    d.rectangle([P(x, 0), P(x + w * 0.92, t)], fill=c)
    d.rectangle([P(x, h / 2 - t / 2), P(x + w * 0.78, h / 2 + t / 2)], fill=c)
    d.rectangle([P(x, h - t), P(x + w * 0.92, h)], fill=c)
    x += w * 0.92 + gap
    # L
    d.rectangle([P(x, 0), P(x + t, h)], fill=c)
    d.rectangle([P(x, h - t), P(x + w * 0.88, h)], fill=c)
    x += w * 0.88 + gap
    # Λ (no crossbar)
    d.polygon([P(x, h), P(x + t * 1.15, h), P(x + w / 2 + t * 0.58, 0), P(x + w / 2 - t * 0.58, 0)], fill=c)
    d.polygon([P(x + w - t * 1.15, h), P(x + w, h), P(x + w / 2 + t * 0.58, 0), P(x + w / 2 - t * 0.58, 0)], fill=c)
    # swoosh: leaves the Λ's right foot, climbs and arcs back over the word
    foot = (x + w - t * 0.3, h)
    pts = bezier(foot, (x + w + h * 0.95, h * 0.2), (x + w + h * 0.4, -h * 0.75), (h * 0.9, -h * 0.42), 500)
    tapered_arc(d, [P(px, py) for px, py in reversed(pts)], t * 0.62 * S, color, alpha_in=0.6)
    return img.resize((W, H), Image.LANCZOS)


def main():
    os.makedirs(OUT, exist_ok=True)
    master = icon(1024)
    master.save(os.path.join(OUT, 'icon.png'))
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    master.save(os.path.join(OUT, 'icon.ico'), sizes=[(s, s) for s in sizes])
    small = icon(64)
    with open(os.path.join(OUT, 'icon-64.rgba'), 'wb') as f:
        f.write(small.tobytes())
    wm = wordmark()
    wm.save(os.path.join(OUT, 'wordmark.png'))
    # preview sheet: icon at several sizes + wordmark on black
    sheet = Image.new("RGBA", (1700, 760), (0, 0, 0, 255))
    sheet.alpha_composite(icon(512), (40, 40))
    x = 600
    for s in (256, 128, 64, 32, 16):
        sheet.alpha_composite(icon(s), (x, 40))
        x += s + 24
    sheet.alpha_composite(wm.resize((int(wm.width * 0.62), int(wm.height * 0.62)), Image.LANCZOS), (600, 380))
    sheet.save(os.path.join(OUT, 'brand-preview.png'))
    print('brand assets written to build/')


if __name__ == '__main__':
    main()
