#!/usr/bin/env bash
# Fails when a banned name appears in crates/, fixtures/, or README.md
# (clean-room rule §3.3–3.5), or a fleet agent name does (SPEC-w4).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

scope=(crates fixtures README.md)
# Substring match: these never occur inside an ordinary word.
substrings='gitnexus|empryo|soulforge|primordial[[:space:]]+deep'
# Whole-word match: these are ordinary words inside longer ones. The second
# line adds fleet agent names. The shorter ones (eli, lea, leo, oli, uri,
# sati) are left out: as whole words they also occur in ordinary text.
words='forge|mote|marionette|spark|ember|genome|phenotypes'
words+='|vini|merci|marci|jupi|nepi|elio'

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
