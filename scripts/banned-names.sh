#!/usr/bin/env bash
# Fails when a banned name appears in crates/ or README.md (clean-room rule §3.3–3.5),
# or a fleet agent name appears in any tracked file.
#
# The fleet names are not in the repo. They come from the BANNED_NAMES
# environment variable (the CI secret of that name), separated by commas or
# newlines. An empty list fails closed, except on a fork PR, where secrets
# are not available: BANNED_NAMES_FORK_PR=true skips the fleet check with a
# notice. A fleet hit prints only path:line. Nothing here prints the list.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

scope=(crates README.md)
# Substring match: these never occur inside an ordinary word.
substrings='gitnexus|empryo|soulforge|primordial[[:space:]]+deep'
# Whole-word match: these are ordinary words inside longer ones.
words='forge|mote|marionette|spark|ember|genome|phenotypes'

# One `-e NAME` pair per fleet name, lowercased and trimmed.
fleet=()
while IFS= read -r entry || [[ -n $entry ]]; do
  entry="${entry#"${entry%%[![:space:]]*}"}"
  entry="${entry%"${entry##*[![:space:]]}"}"
  [[ -z $entry ]] && continue
  if ! [[ $entry =~ ^[a-z0-9-]+$ ]]; then
    echo "BANNED_NAMES has an entry that is not [a-z0-9-]+" >&2
    exit 2
  fi
  fleet+=(-e "$entry")
done < <(printf '%s\n' "${BANNED_NAMES-}" | tr ',' '\n' | tr '[:upper:]' '[:lower:]')

if [[ ${#fleet[@]} -eq 0 ]]; then
  if [[ ${BANNED_NAMES_FORK_PR-} == true ]]; then
    echo "::notice::fleet-name check skipped: secrets are not available on a fork PR"
  else
    echo "BANNED_NAMES is empty: failing closed" >&2
    exit 2
  fi
fi

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

if [[ ${#fleet[@]} -gt 0 ]]; then
  # git grep exits 1 when nothing matches and above 1 on an error. Its
  # stderr is dropped so that no message can echo a pattern.
  files=0
  git grep -l -z -I -i -w -E "${fleet[@]}" >/dev/null 2>&1 || files=$?
  if [[ $files -gt 1 ]]; then
    echo "fleet-name check failed to run" >&2
    exit 2
  fi
  if [[ $files -eq 0 ]]; then
    while IFS= read -r -d '' path; do
      # -h drops the path, so each line starts with its number.
      { git grep -n -h -I -i -w -E "${fleet[@]}" -- ":(literal)$path" 2>/dev/null || true; } |
        cut -d: -f1 |
        while IFS= read -r line; do
          echo "$path:$line"
        done
    done < <(git grep -l -z -I -i -w -E "${fleet[@]}" 2>/dev/null)
    echo "fleet name in file contents" >&2
    status=1
  fi
fi
exit "$status"
