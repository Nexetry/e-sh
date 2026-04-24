#!/usr/bin/env python3
"""
Apply a macOS-style continuous-corner (superellipse) mask to icon-1024.png.

n=5 superellipse approximates the SwiftUI `RoundedRectangle(cornerRadius:_, style: .continuous)`
mask used for all macOS Big Sur+ app icons. Supersampled 4x for smooth anti-aliased edges.
"""

from PIL import Image
from pathlib import Path
import numpy as np

SRC = Path("assets/icon-1024.png")
DST = Path("assets/icon-1024.png")
SIZE = 1024
SUPERSAMPLE = 4
N = 5.0  # superellipse exponent; 4 = squircle, 5 ≈ Apple continuous corner

def superellipse_mask(size: int, n: float) -> Image.Image:
    big = size * SUPERSAMPLE
    half = big / 2.0
    coords = (np.arange(big) + 0.5 - half) / half
    nx = np.abs(coords) ** n
    # |x/a|^n + |y/a|^n <= 1
    field = nx[None, :] + nx[:, None]
    mask_arr = (field <= 1.0).astype(np.uint8) * 255
    return Image.fromarray(mask_arr, mode="L").resize((size, size), Image.LANCZOS)

def main():
    img = Image.open(SRC).convert("RGBA")
    if img.size != (SIZE, SIZE):
        img = img.resize((SIZE, SIZE), Image.LANCZOS)
    mask = superellipse_mask(SIZE, N)
    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(img, (0, 0), mask=mask)
    out.save(DST, "PNG", optimize=True)
    print(f"wrote {DST}")

if __name__ == "__main__":
    main()
