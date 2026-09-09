#!/usr/bin/env bash
# Extract one version section from RELEASES.md.
#
# Usage: scripts/extract_release_notes.sh <version> [RELEASES.md]
#
# The section starts at "# Version <version>" (an optional leading "v" is
# accepted) and ends at the next "---" separator or the next "# Version"
# heading. The awk input is fully consumed so callers can safely use
# `set -o pipefail` (sed no longer dies with SIGPIPE on early exit).
set -euo pipefail

version="${1:?usage: extract_release_notes.sh <version> [RELEASES.md]}"
file="${2:-RELEASES.md}"

if [ ! -f "$file" ]; then
  echo "release notes file not found: $file" >&2
  exit 1
fi

LC_ALL=C sed $'1s/^\xEF\xBB\xBF//' "$file" | awk -v version="$version" '
  BEGIN {
    in_section = 0
    found = 0
  }
  # Consume the full input so sed does not fail with SIGPIPE under pipefail.
  found && !in_section { next }
  /^# +Version[[:space:]]+/ {
    if (in_section) { in_section = 0; next }
    pattern = "^# +Version[[:space:]]+v?" version "([[:space:]]|$)"
    if ($0 ~ pattern) {
      in_section = 1
      found = 1
      next
    }
  }
  /^---[[:space:]]*$/ {
    if (in_section) { in_section = 0; next }
  }
  in_section {
    print
  }
  END {
    if (!found) exit 1
  }
' | sed '/./,$!d'
