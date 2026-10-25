#!/usr/bin/env bash
# Fails when a banned name appears in crates/ or README.md (clean-room rule §3.3–3.5).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

scope=(crates README.md)
# Substring match: these never occur inside an ordinary word.
substrings='gitnexus|empryo|soulforge|primordial[[:space:]]+deep'
# Whole-word match: these are ordinary words inside longer ones.
words='forge|mote|marionette|spark|ember|genome|phenotypes'

status=0
if git ls-files -- "${scope[@]}" | grep -i -E "$substrings"; then
  echo "banned name in a tracked path" >&2
  status=1
fi
if git grep -n -I -i -E "$substrings" -- "${scope[@]}"; then
  echo "banned name in file contents" >&2
  status=1
fi
if git grep -n -I -i -w -E "$words" -- "${scope[@]}"; then
  echo "banned word in file contents" >&2
  status=1
fi
exit "$status"
