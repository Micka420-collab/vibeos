#!/usr/bin/env python3
# generate-assets.py — génère les PNG du thème Plymouth VibeOS (dans vibeos/).
#
# Placé HORS du sous-dossier `vibeos/` exprès : le Containerfile copie
# `desktop/plymouth/vibeos` dans l'image, donc ce générateur (outil de build) ne
# doit PAS s'y trouver — seuls les assets du thème sont embarqués.
#
# Œuvre ORIGINALE VibeOS (Apache-2.0), dessinée par code — aucune source tierce.
# La provenance est ce script : les PNG commités dans vibeos/ sont reproductibles.
# Design : docs/DESIGN-SYSTEM.md §11.1 (Boot). La marque VibeOS est la SPIRALE
# galactique mauve→blue (le même motif que les wallpapers et le badge du sweat).
#
#   vibeos/ring.png     spirale galactique 2 bras, dégradé signature mauve
#                       #cba6f7 → blue #89b4fa, avec glow. Anneau qui TOURNE.
#   vibeos/mark.png     cœur galactique calme : point lumineux Text #cdd6f4 +
#                       halo doux. Il RESPIRE (opacité) pendant que la spirale tourne.
#   vibeos/wordmark.png « VibeOS », Text #cdd6f4 (posé à 55 % par le script).
#
# Regénérer : `python desktop/plymouth/generate-assets.py` (nécessite Pillow).
# Le rendu du wordmark dépend de la police trouvée ; le PNG commité fait foi.
import math
import os
from PIL import Image, ImageDraw, ImageFilter, ImageFont

# Sortie = le sous-dossier vibeos/ à côté de ce script.
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "vibeos")
SS = 4  # supersampling pour l'antialiasing

MAUVE = (203, 166, 247)  # #cba6f7
BLUE  = (137, 180, 250)  # #89b4fa
TEXT  = (205, 214, 244)  # #cdd6f4


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def spiral_points(cx, cy, r_min, r_max, turns, phase, n=900):
    pts = []
    for i in range(n):
        f = i / (n - 1)
        theta = phase + f * turns * 2 * math.pi
        r = r_min + (r_max - r_min) * (f ** 1.15)
        pts.append((cx + r * math.cos(theta), cy + r * math.sin(theta), f))
    return pts


def brush(draw, pts, color_fn, rad_fn):
    for (x, y, f) in pts:
        r = rad_fn(f)
        draw.ellipse([x - r, y - r, x + r, y + r], fill=color_fn(f))


def make_ring(size):
    S = size * SS
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d, dg = ImageDraw.Draw(img), ImageDraw.Draw(glow)
    cx = cy = S / 2
    r_min, r_max = 0.09 * S, 0.45 * S
    for a in range(2):  # 2 bras, symétrie galactique
        phase = a * math.pi
        pts = spiral_points(cx, cy, r_min, r_max, 1.12, phase)
        brush(dg, pts, lambda f: lerp(MAUVE, BLUE, f) + (95,),
              lambda f: (5.0 + 9.0 * f) * SS)
        brush(d, pts, lambda f: lerp(MAUVE, BLUE, f) + (255,),
              lambda f: (1.0 + 2.3 * f) * SS)
    glow = glow.filter(ImageFilter.GaussianBlur(radius=5 * SS))
    out = Image.alpha_composite(glow, img)
    return out.resize((size, size), Image.LANCZOS)


def make_mark(size):
    # Cœur galactique calme : point lumineux + halo doux (Text #cdd6f4). Il
    # respire au centre pendant que la spirale signature tourne autour.
    S = size * SS
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d, dg = ImageDraw.Draw(img), ImageDraw.Draw(glow)
    cx = cy = S / 2
    hr = 0.17 * S
    dg.ellipse([cx - hr, cy - hr, cx + hr, cy + hr], fill=TEXT + (110,))
    glow = glow.filter(ImageFilter.GaussianBlur(radius=0.09 * S))
    dr = 0.052 * S
    d.ellipse([cx - dr, cy - dr, cx + dr, cy + dr], fill=TEXT + (255,))
    out = Image.alpha_composite(glow, img)
    return out.resize((size, size), Image.LANCZOS)


def _load_font(size):
    # Police best-effort : libre d'abord (repro Linux/CI), Calibri en secours
    # sur Windows. Le PNG rendu est l'asset autoritatif commité (raster, pas la
    # police) ; ce fallback ne sert qu'à re-générer.
    for path in (
        "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "C:/Windows/Fonts/calibrib.ttf",
        "C:/Windows/Fonts/arialbd.ttf",
    ):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def make_wordmark(target_w):
    font = _load_font(220)
    tmp = Image.new("RGBA", (1400, 400), (0, 0, 0, 0))
    dt = ImageDraw.Draw(tmp)
    dt.text((20, 20), "VibeOS", font=font, fill=TEXT + (255,))
    bbox = tmp.getbbox()
    cropped = tmp.crop(bbox)
    ratio = target_w / cropped.width
    return cropped.resize((target_w, int(cropped.height * ratio)), Image.LANCZOS)


if __name__ == "__main__":
    for (name, fn, base) in [("ring", make_ring, 256),
                             ("mark", make_mark, 128),
                             ("wordmark", make_wordmark, 320)]:
        fn(base).save(os.path.join(OUT, name + ".png"))
        print(f"{name}: {base}px")
    print("OK")
