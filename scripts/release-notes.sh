#!/usr/bin/env bash
# Prints the CHANGELOG.md section of one version (without its heading): the release notes.
#
#   scripts/release-notes.sh 0.2.0
#
# A section starts with a line like "## 0.2.0 - 2026-12-01" (or "## [0.2.0] ...") and ends
# at the next "## " line. Fails if there is no section or it is empty.
set -euo pipefail
version="${1:?usage: release-notes.sh VERSION (without the v)}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."

notes="$(awk -v v="$version" '
  /^## / { if (found) exit; if ($2 == v || $2 == "[" v "]") { found = 1; next } }
  found { print }
' CHANGELOG.md)"

if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
    echo "CHANGELOG.md has no entry for $version (add a '## $version - YYYY-MM-DD' section)" >&2
    exit 1
fi
printf '%s\n' "$notes"
