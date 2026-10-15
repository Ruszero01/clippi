"""Regenerate the offline picker data from a pinned Emojibase release.

Run explicitly when updating assets; this is never part of the application build.
"""
import json
import urllib.request
from pathlib import Path

VERSION = "17.0.0"
ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "assets" / "emoji" / "emoji.json"


def fetch(locale):
    url = f"https://cdn.jsdelivr.net/npm/emojibase-data@{VERSION}/{locale}/data.json"
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.load(response)


def main():
    english = {}
    for entry in fetch("en"):
        english[entry["hexcode"]] = entry
        for skin in entry.get("skins", []):
            english[skin["hexcode"]] = skin

    rows = []
    for entry in fetch("zh"):
        if "group" not in entry:
            continue
        translated = english.get(entry["hexcode"], {})
        keywords = " ".join(entry.get("tags", []) + translated.get("tags", []))
        for variant, item in [(False, entry)] + [
            (True, skin) for skin in entry.get("skins", [])
        ]:
            row = {
                "text": item["emoji"],
                "name": item["label"],
                "name_en": english.get(item["hexcode"], translated).get("label", ""),
                "keywords": keywords,
                "group": entry["group"],
            }
            if variant:
                row["variant"] = True
            rows.append(row)

    # Validate before replacing the checked-in resource.
    assert len(rows) > 3000
    assert len({row["text"] for row in rows}) == len(rows)
    DESTINATION.write_text(
        json.dumps(rows, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    print(f"Generated {len(rows)} entries from emojibase-data {VERSION}")


if __name__ == "__main__":
    main()
