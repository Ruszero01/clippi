"""Build portable outline data; only needed when changing the wall's typeface.

Requires fontTools. Pass a local Inter variable TTF downloaded from:
https://github.com/google/fonts/tree/main/ofl/inter
The generated subset is distributed under the adjacent OFL license.
Normal contributor updates need only Python's standard library.
"""

import argparse
import json
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont


ROOT = Path(__file__).resolve().parent.parent


def number(value):
    return f"{value:.2f}".rstrip("0").rstrip(".")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("font", type=Path)
    args = parser.parse_args()
    source = TTFont(args.font.resolve())
    faces = {}
    characters = " ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-…"
    for weight in (400, 600, 650):
        font = instantiateVariableFont(
            source, {"wght": weight, "opsz": 32 if weight == 650 else 14}, inplace=False,
        )
        glyphs, cmap = font.getGlyphSet(), font.getBestCmap()
        face = {}
        for char in characters:
            glyph_name = cmap[ord(char)]
            pen = SVGPathPen(glyphs, ntos=number)
            glyphs[glyph_name].draw(pen)
            face[char] = {"advance": glyphs[glyph_name].width, "path": pen.getCommands()}
        faces[str(weight)] = face
    data = {
        "font": "Inter", "license": "contributors-font-OFL.txt",
        "source": "https://github.com/google/fonts/tree/main/ofl/inter",
        "units_per_em": source["head"].unitsPerEm, "faces": faces,
    }
    output = ROOT / ".github" / "contributors-font.json"
    output.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
