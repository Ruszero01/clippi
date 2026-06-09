#!/usr/bin/env python3
"""Patch iconfont.ttf with an 'm' glyph so it passes GPUI's font loading check.

GPUI's macOS and Linux text systems require every font to have a glyph
for the letter 'm' (used for text measurements). Icon-only fonts that
only have glyphs in the Private Use Area (U+E600-U+E9FF) are silently
dropped, causing all icons to render as tofu (□).

This script adds a cmap entry mapping U+006D ('m') to the first real
glyph in the font. Run after every iconfont.ttf regeneration.
"""

import shutil
import sys
from pathlib import Path

try:
    from fontTools.ttLib import TTFont
except ImportError:
    print("fonttools not installed. Run: pip3 install fonttools")
    sys.exit(1)

FONT_PATH = Path(__file__).parent.parent / "assets" / "fonts" / "iconfont.ttf"


def patch_font(path: Path) -> None:
    font = TTFont(path)

    # Check the glyph order before modifying
    all_glyphs = font.getGlyphOrder()
    if len(all_glyphs) < 2:
        print(f"ERROR: font has only {len(all_glyphs)} glyph(s), need at least 2")
        sys.exit(1)

    target_glyph = all_glyphs[1]  # First real glyph (index 0 = .notdef)
    print(f"Mapping U+006D (m) → {target_glyph} (glyph index 1 of {len(all_glyphs)})")

    cmap = font["cmap"]
    added = 0
    for table in cmap.tables:
        if 0x006D not in table.cmap:
            table.cmap[0x006D] = target_glyph
            added += 1
            print(f"  Added to platformID={table.platformID} format={table.format}")

    if added == 0:
        print("  U+006D already mapped, nothing to do")
    else:
        # Backup original
        backup = path.with_suffix(".ttf.bak")
        shutil.copy2(path, backup)
        font.save(path)
        print(f"  Saved. Original backed up to {backup.name}")


if __name__ == "__main__":
    if not FONT_PATH.exists():
        print(f"ERROR: {FONT_PATH} not found")
        sys.exit(1)
    patch_font(FONT_PATH)
