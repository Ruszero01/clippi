# Emoji data

emoji.json is derived from milesj/emojibase packages/data/{zh,en}/data.json
(emojibase-data 17.0.0, retrieved 2026-10-15), retaining Unicode sequences, localized
names, search keywords, groups and skin tone variants.
https://github.com/milesj/emojibase

Emojibase: MIT, see LICENSE-emojibase. Underlying Unicode/CLDR data:
Unicode License v3, see LICENSE-unicode.

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

## Kaomoji

kaomoji.json is derived from kaomojikan/kaomoji-data, pinned to commit
60c92ea4e85279ad42ff525e9a40464ebeb1003e (retrieved 2026-10-16).
https://github.com/kaomojikan/kaomoji-data/tree/60c92ea4e85279ad42ff525e9a40464ebeb1003e

MIT, see LICENSE-kaomojikan. Retain that notice when redistributing the data.
The generator deduplicates exact text while merging all categories and Japanese
metadata, adds curated Chinese/English category names and search aliases, and
retains the original picker selection in kaomoji-curated.json. Spaces, literal
vertical bars and Unicode sequences are preserved. Multiline ASCII pictures and
control characters are excluded to fit the compact single-line picker.

The generated snapshot contains 1,678 unique expressions.
The picker offers 24 categories (23 upstream plus Greetings). Entries may appear
in multiple categories; search results contain each expression once. English
and Chinese names describe categories, not individually translated expressions.

Regenerate explicitly with scripts/update-kaomoji-data.py. The build and running
application load the checked-in JSON and never fetch kaomoji from the network.
