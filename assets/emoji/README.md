# Emoji data

emoji.json is derived from milesj/emojibase packages/data/{zh,en}/data.json
(emojibase-data 17.0.0, retrieved 2026-10-15), retaining Unicode sequences, localized
names, search keywords, groups and skin tone variants.
https://github.com/milesj/emojibase

Emojibase: MIT, see LICENSE-emojibase. Underlying Unicode/CLDR data:
Unicode License v3, see LICENSE-unicode.

Kaomoji are a small manually curated selection in src/core/emoji.rs.
No remote data is fetched by the application.

To regenerate the JSON, run scripts/update-emoji-data.py explicitly.

## Symbols

symbols.json is generated directly from Unicode 17.0.0 UnicodeData.txt and
CLDR release-48 common/annotations/zh.xml, with curated Chinese names and
search aliases for common characters. Both sources use the Unicode license
included in LICENSE-unicode.

- https://www.unicode.org/Public/17.0.0/ucd/UnicodeData.txt
- https://github.com/unicode-org/cldr/blob/release-48/common/annotations/zh.xml

The generator selects named letters, numbers, punctuation and symbols from
explicit ranges and removes controls, format characters, combining marks,
spaces, private-use and unassigned characters. Each character is stored once;
the Common category is a view over the same entries. Names without a Chinese
translation retain their official English name. System font coverage varies.

Regenerate explicitly with scripts/update-symbol-data.py. Category IDs are
shared with src/core/symbols.rs. Generation is not part of the build.
