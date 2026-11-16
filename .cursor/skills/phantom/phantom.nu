#!/usr/bin/env nu
# Deterministic body for the phantom skill. Wraps `phantom-fractal --json`.
#
# The binary is `$PHANTOM_FRACTAL_BIN` when set, else `phantom-fractal` on
# PATH. It is the only program this script runs, and every value travels
# as its own argv element: nothing is joined into a shell string.
#
# Every command prints one JSON line and exits with its `exit`:
#   { ok: true, command, data, meta, exit: 0 }
#   { ok: false, command, error, kind, stderr, exit }
# A failure copies the CLI's `error.kind`, and its `matches` (the ambiguous
# candidates) and `did_you_mean`, onto the envelope unchanged.
#
# Flags go before `--`. After `--` every word is a name or a path, and a
# known flag there is a usage error. String values travel as
# `--flag=value`, and names and paths after `--`, so a value that starts
# with `-` is never read as a flag. A usage error runs nothing.
#
# `help`, and `--help` or `-h` before `--` anywhere, print the command
# table and run nothing. The global flags `--repo`, `--limit` and `--depth`
# may also come before the command; they are handed to it unchanged, so a
# command that does not take one says so as a usage error.
#
# A name in `where`'s display shape, `file.rs:LINE Owner::name`, is read
# as the path-qualified `file.rs::Owner::name`. No name the CLI resolves
# has that shape, so no answered query changes.
#
# CLI each command actually runs:
#   where        show [--repo=R] -- NAME, then find -- NAME on not_found
#   callers      callers [--repo=R] -- NAME
#   callees      callees [--repo=R] -- NAME
#   impact       impact [--depth=N] [--min-confidence=X] [--no-tests] [--limit=N] [--repo=R] -- NAME
#   diff-impact  impact --diff [--against=REF] [--depth=N] [--min-confidence=X] [--no-tests] [--repo=R]
#   path         path [--max-depth=N] [--repo=R] -- FROM TO
#   cochange     cochange --count [--limit=N] [--repo=R] -- FILE
#   cochange --rust  cochange [--limit=N] [--min-lift=X] [--repo=R] -- FILE
#   map          map --budget=N [--focus=P]... [--detail=D] [--repo=R]
#   rank         rank [--files] [--limit=N] [--focus=P]... [--repo=R]
#   show         show [--source] [--repo=R] -- NAME
#   status       status [--repo=R]
#   index        index -- PATH
# `index` writes Phantom's own data directory, never the repository. It may
# run at any time; no read command sends it.

const COUNT_NOTE = "raw counts: for every tracked file, the non-merge commits it shares with FILE in the scanned window, no decay and no lift floor; commits over scope.bulk_skip files are skipped and counted in scan.skipped_bulk; history, not dependency"
const RUST_NOTE = "--rust: indexed Rust files only, decay-weighted (90 days) and lift-filtered; together_decayed is a weight, not a commit count; for raw counts over every tracked file, drop --rust"

const HELP_FLAGS = ["--help" "-h"]
const GLOBAL_FLAGS = ["--repo" "--limit" "--depth"]
# Keys the CLI adds at the end of `callers` and `impact` only when it
# flags a re-exported target; copied through in this order.
const FLAG_KEYS = ["reexported" "confirm_with_grep"]
# The list usage errors name, as W14 printed it; `help` is in SKILL.md.
const COMMANDS = ["where" "callers" "callees" "impact" "diff-impact" "path" "cochange" "map" "rank" "show" "status" "index"]

# The command table `help` prints. It runs nothing.
const HELP = {
  commands: [
    { usage: "where NAME", answers: "where X is defined: file:line, kind, signature; every candidate of a bare name" }
    { usage: "callers NAME [--limit N]", answers: "who calls X, grouped by caller, with sites and files" }
    { usage: "callees NAME [--limit N]", answers: "what X calls, and how many calls were unresolved" }
    { usage: "impact NAME [--depth 1..5] [--limit N] [--no-tests]", answers: "what breaks if X changes: callers by depth, tests apart, risk" }
    { usage: "diff-impact [--against REF]", answers: "what the uncommitted change reaches" }
    { usage: "path FROM TO", answers: "how A reaches B" }
    { usage: "cochange FILE [--limit N]", answers: "which tracked files change in the same commits: raw counts" }
    { usage: "cochange --rust FILE [--limit N] [--min-lift X]", answers: "which Rust files tend to move with FILE: weights" }
    { usage: "map [--budget N] [--focus PATH]", answers: "orient: ranked files and their symbols" }
    { usage: "rank [--files] [--limit N] [--focus PATH]", answers: "the most central symbols or files" }
    { usage: "show NAME [--source]", answers: "one symbol with its callers and callees" }
    { usage: "status", answers: "is the index current" }
    { usage: "index [PATH]", answers: "builds or refreshes the index; Phantom's data directory only" }
  ]
  flags: "--repo R, --limit N and --depth N go before or after the command; a word after -- is never a flag"
  names: "NAME: a bare name, Type::method, module::name, or a display from any reply; on kind ambiguous, rerun with one of matches[].display"
  envelope: "one JSON line: ok, command, data, meta, exit; or ok false, command, error, kind, stderr, exit"
  read: "SKILL.md beside this script says when to trust each answer"
}

def help-envelope [] {
  { ok: true, command: "help", data: $HELP, meta: null }
}

def phantom-bin [] {
  let over = ($env.PHANTOM_FRACTAL_BIN? | default "")
  if $over == "" { "phantom-fractal" } else { $over }
}

def usage [command: string, error: string] {
  { ok: false, command: $command, error: $error, kind: "usage", stderr: "", exit: 1 }
}

def missing-phantom [command: string, detail: string] {
  {
    ok: false
    command: $command
    error: "phantom-fractal is not on PATH, and PHANTOM_FRACTAL_BIN does not name a program"
    kind: "missing_phantom"
    stderr: ($detail | str substring 0..<400)
    exit: 127
  }
}

# Print the envelope as one line and exit with its code. A success gains
# `exit: 0` as its last key; every other key is as it was.
def finish [e: record] {
  let out = if ($e.ok? | default false) and ($e.exit? == null) { $e | upsert exit 0 } else { $e }
  print ($out | to json --raw)
  exit ($out.exit? | default 0)
}

# Run the CLI once with `--json`. The arguments are a list, never a string.
def call [command: string, args: list<string>] {
  let exe = (phantom-bin)
  let r = (try {
    ^$exe --json ...$args | complete
  } catch {|err|
    { exit_code: 127, stdout: "", stderr: ($err.msg? | default "not found"), missing: true }
  })
  if ($r.missing? | default false) {
    return (missing-phantom $command $r.stderr)
  }
  let code = ($r.exit_code? | default 1)
  let stdout = ($r.stdout? | default "" | str trim)
  let stderr = ($r.stderr? | default "" | str trim)
  let doc = (try { $stdout | from json } catch { null })
  let parsed = ($doc != null) and (($doc | describe) | str starts-with "record")
  if $code == 0 and $parsed and ($doc.ok? | default false) {
    { ok: true, command: $command, data: ($doc.data? | default null), meta: ($doc.meta? | default null) }
  } else {
    failure $command $code $stdout $stderr (if $parsed { $doc } else { null })
  }
}

# A failed run as an envelope, the CLI's own fields copied through.
def failure [command: string, code: int, stdout: string, stderr: string, doc: any] {
  let err = if $doc == null { null } else { $doc.error? | default null }
  let data = if $doc == null { null } else { $doc.data? | default null }
  let raw = ([$stderr $stdout] | str join " " | str trim)
  let msg = if $err == null { $raw } else { $err.message? | default $raw }
  # Clap prints text and exits 1 before an envelope can be written.
  let kind = if $err != null {
    $err.kind? | default "error"
  } else if $code == 1 {
    "usage"
  } else {
    "error"
  }
  mut out = {
    ok: false
    command: $command
    error: ($msg | str substring 0..<400)
    kind: $kind
    stderr: ($stderr | str substring 0..<400)
    exit: (if $code == 0 { 1 } else { $code })
  }
  let matches = (first-set [
    ($err | get-key "matches")
    ($data | get-key "matches")
    ($data | get-key "candidates")
  ])
  if $matches != null { $out = ($out | upsert matches $matches) }
  let total = ($data | get-key "total")
  if $total != null { $out = ($out | upsert total $total) }
  let dym = (first-set [($err | get-key "did_you_mean") ($data | get-key "did_you_mean")])
  if $dym != null { $out = ($out | upsert did_you_mean $dym) }
  $out
}

def is-record [v: any] {
  ($v | describe) | str starts-with "record"
}

# A success from the CLI that lacks the symbol the reshaping needs: an
# envelope, never a nu error.
def no-symbol [command: string, ran: string] {
  {
    ok: false
    command: $command
    error: $"($ran) answered without a symbol record; rerun, or run status"
    kind: "error"
    stderr: ""
    exit: 1
  }
}

def get-key [key: string] {
  let v = $in
  if $v == null { null } else if (($v | describe) | str starts-with "record") {
    $v | get --optional $key
  } else { null }
}

def first-set [values: list] {
  let set = ($values | where {|v| $v != null })
  if ($set | is-empty) { null } else { $set | first }
}

# --- argument parsing --------------------------------------------------

def valued-name [tok: string, valued: list<string>] {
  let hit = ($valued | where {|n| $tok == $n or ($tok | str starts-with $"($n)=") })
  if ($hit | is-empty) { "" } else { $hit | first }
}

# Flags before `--`, in either `--flag value` or `--flag=value` form, in
# any order with the positionals. After `--` every word is positional, and
# a known flag there is a usage error. Unknown flags are usage errors, and
# `--help` or `-h` is the help envelope.
# Returns { ok: true, on, values, pos }, or an envelope to print as it is:
# only an envelope has a `command` key.
def parse-args [
  command: string
  line: string
  args: list<string>
  switches: list<string>
  valued: list<string>
] {
  mut i = 0
  mut after = false
  mut on = []
  mut values = {}
  mut pos = []
  let n = ($args | length)
  while $i < $n {
    let tok = ($args | get $i)
    let named = (valued-name $tok $valued)
    let known = ($named != "") or ($tok in $switches)
    if $tok == "--" and not $after {
      $after = true
      $i = $i + 1
    } else if (not $after) and ($tok in $HELP_FLAGS) {
      return (help-envelope)
    } else if $after and $known {
      return (usage $command $"($tok) is a flag and must come before --. Usage: ($line)")
    } else if $after {
      $pos = ($pos | append $tok)
      $i = $i + 1
    } else if $named != "" {
      let eq = $"($named)="
      mut value = ""
      if ($tok | str starts-with $eq) {
        $value = ($tok | str substring ($eq | str length)..)
        $i = $i + 1
      } else if ($i + 1) >= $n {
        return (usage $command $"($named) needs a value. Usage: ($line)")
      } else {
        $value = ($args | get ($i + 1))
        $i = $i + 2
      }
      let prior = ($values | get --optional $named | default [])
      $values = ($values | upsert $named ($prior | append $value))
    } else if $tok in $switches {
      $on = ($on | append $tok)
      $i = $i + 1
    } else if ($tok | str starts-with "-") {
      return (usage $command $"unknown flag ($tok). Usage: ($line)")
    } else {
      $pos = ($pos | append $tok)
      $i = $i + 1
    }
  }
  { ok: true, on: $on, values: $values, pos: $pos }
}

# An envelope parse-args returned in place of the parsed arguments.
def printable [p: record] {
  "command" in ($p | columns)
}

def opt [p: record, name: string] {
  let vs = ($p.values | get --optional $name | default [])
  if ($vs | is-empty) { "" } else { $vs | last }
}

def opts [p: record, name: string] {
  $p.values | get --optional $name | default []
}

def has [p: record, name: string] {
  $name in $p.on
}

# `--flag=value` when the flag was given.
def pass [p: record, name: string] {
  let v = (opt $p $name)
  if $v == "" { [] } else { [$"($name)=($v)"] }
}

def pass-all [p: record, name: string] {
  opts $p $name | each {|v| $"($name)=($v)" }
}

def switch [p: record, name: string] {
  if (has $p $name) { [$name] } else { [] }
}

# An integer flag: null when absent, else the number or an error text.
def int-opt [p: record, name: string] {
  let v = (opt $p $name)
  if $v == "" {
    { value: null, error: "" }
  } else {
    let n = (try { $v | into int } catch { null })
    if $n == null or $n < 1 {
      { value: null, error: $"($name) must be a positive integer, not ($v)" }
    } else {
      { value: $n, error: "" }
    }
  }
}

# Integer flags the CLI range-checks itself; this only refuses non-numbers.
def check-ints [command: string, line: string, p: record, names: list<string>] {
  let bad = ($names | each {|n| (int-opt $p $n).error } | where {|e| $e != "" })
  if ($bad | is-empty) { null } else { usage $command $"($bad | first). Usage: ($line)" }
}

# --- reshaping -----------------------------------------------------------

def is-upper [s: string] {
  if ($s | is-empty) { false } else {
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ" | str contains ($s | str substring 0..<1)
  }
}

# `crate::tui::app::App::watch_dir` -> `App::watch_dir`; a free function
# keeps its own name. The owner is kept when it looks like a type.
def short-name [qual: string] {
  let segs = ($qual | split row "::" | where {|s| $s != "" and $s != "crate" })
  let n = ($segs | length)
  if $n == 0 {
    $qual
  } else if $n >= 2 and (is-upper ($segs | get ($n - 2))) {
    $"($segs | get ($n - 2))::($segs | last)"
  } else {
    $segs | last
  }
}

# `file::Owner::name`, the shape callers and callees answer in.
def at-file [file: string, qual: string] {
  $"($file)::(short-name $qual)"
}

# `file:line Owner::name`, the shape where answers in.
def at-line [path: string, line: any, qual: string] {
  let l = if $line == null { "" } else { $":($line)" }
  $"($path)($l) (short-name $qual)"
}

def symbol-row [s: any] {
  if $s == null { null } else {
    let qual = ($s.qual_name? | default ($s.name? | default ""))
    {
      display: ($s.display? | default (at-file ($s.path? | default "") $qual))
      name: $qual
      kind: ($s.kind? | default null)
      path: ($s.path? | default null)
      line: ($s.line? | default null)
    }
  }
}

# Call-site hits grouped by the symbol at the other end. A caller hit's
# `path` and `line` are the site; `symbol_path` is the file that symbol is in.
def group-calls [hits: list, key: string] {
  let rows = ($hits | each {|h|
    let file = ($h.symbol_path? | default ($h.path? | default ""))
    {
      display: ($h.display? | default (at-file $file ($h.name? | default "")))
      name: ($h.name? | default "")
      file: $file
      site: ($h.line? | default null)
      confidence: ($h.confidence? | default null)
      reason: ($h.reason? | default null)
    }
  })
  let names = ($rows | get display | uniq | sort)
  let groups = ($names | each {|d|
    let mine = ($rows | where display == $d)
    let one = ($mine | first)
    {
      display: $d
      name: $one.name
      path: $one.file
      sites: ($mine | get site | where {|s| $s != null } | uniq | sort)
      min_confidence: ($mine | get confidence | where {|c| $c != null } | math min)
      reasons: ($mine | get reason | where {|r| $r != null } | uniq | sort)
    }
  })
  {
    $key: $groups
    files: ($rows | get file | uniq | sort)
    site_count: ($rows | length)
  }
}

def calls-data [data: record, key: string, limit: any] {
  let g = (group-calls ($data.hits? | default []) $key)
  let all = ($g | get $key)
  let kept = if $limit == null { $all } else { $all | first $limit }
  mut out = {
    symbol: (symbol-row ($data.symbol? | default null))
    $key: $kept
    files: $g.files
    site_count: $g.site_count
    truncated: (($data.truncated? | default false) or (($kept | length) < ($all | length)))
  }
  if "unresolved_calls" in ($data | columns) {
    $out = ($out | upsert unresolved_calls $data.unresolved_calls)
  }
  $out | merge (copy-keys $data $FLAG_KEYS)
}

# The CLI's `keys` that `data` holds, in that order, values unchanged.
def copy-keys [data: record, keys: list<string>] {
  $keys | where {|k| $k in ($data | columns) } | reduce --fold {} {|k, acc| $acc | upsert $k ($data | get $k) }
}

def reached [h: record] {
  $h.display? | default (at-file ($h.path? | default "") ($h.name? | default ""))
}

# impact and diff-impact: callers by depth, tests apart, the files reached.
def impact-data [data: record] {
  let groups = ($data.groups? | default [])
  let tests = ($data.tests? | default [])
  let by_depth = ($groups | each {|g|
    {
      depth: $g.depth
      total: ($g.total? | default null)
      truncated: ($g.truncated? | default false)
      callers: ($g.hits? | default [] | each {|h| reached $h })
    }
  })
  let files = ($groups | each {|g| $g.hits? | default [] | get --optional path } | flatten)
  let test_files = ($tests | get --optional path | default [])
  {
    risk: ($data.risk? | default null)
    exact: ($data.exact? | default null)
    reasons: ($data.reasons? | default [])
    counts: ($data.counts? | default null)
    by_depth: $by_depth
    tests: ($tests | each {|h| reached $h })
    files: ($files ++ $test_files | where {|f| $f != null } | uniq | sort)
  } | merge (copy-keys $data (["limit"] ++ $FLAG_KEYS))
}

def iso [t: any] {
  if $t == null { null } else {
    try { $t * 1_000_000_000 | into datetime | format date "%Y-%m-%dT%H:%M:%SZ" } catch { null }
  }
}

def partner-row [p: record] {
  {
    path: ($p.path? | default null)
    together_decayed: ($p.together_decayed? | default ($p.together? | default null))
    lift: ($p.lift? | default null)
    conf: ($p.conf? | default null)
    last_time_iso: (iso ($p.last_time? | default null))
  }
}

def node [n: any] {
  if $n == null { null } else {
    $"($n.path? | default '')::($n.name? | default '')"
  }
}

# --- commands --------------------------------------------------------------

def repo-arg [p: record] { pass $p "--repo" }

# `file.rs:LINE Owner::name`, the display `where` answers in, as the
# path-qualified `file.rs::Owner::name`; any other name as it is.
def symbol-name [name: string] {
  let m = ($name | parse --regex '\A(?<file>[\w./-]+\.rs):[0-9]+ (?<qual>[\w:]+)\z')
  if ($m | is-empty) { $name } else { $"(($m | first).file)::(($m | first).qual)" }
}

def one-name [command: string, line: string, p: record] {
  if ($p.pos | length) != 1 { usage $command $"one name required. Usage: ($line)" } else { null }
}

def where-hit [s: record] {
  let qual = ($s.qual_name? | default ($s.name? | default ""))
  {
    display: (at-line ($s.path? | default "") ($s.line? | default null) $qual)
    name: $qual
    kind: ($s.kind? | default null)
    path: ($s.path? | default null)
    line: ($s.line? | default null)
    signature: ($s.signature? | default null)
    visibility: ($s.visibility? | default null)
  }
}

def --wrapped "main where" [...args: string] {
  let line = "where [--repo R] NAME"
  let p = (parse-args "where" $line $args [] ["--repo"])
  if (printable $p) { finish $p }
  let bad = (one-name "where" $line $p)
  if $bad != null { finish $bad }
  let name = (symbol-name ($p.pos | first))
  let shown = (call "where" (["show"] ++ (repo-arg $p) ++ ["--" $name]))
  if $shown.ok {
    let symbol = ($shown.data | get-key "symbol")
    if not (is-record $symbol) {
      finish (no-symbol "where" "show")
    }
    finish ($shown | upsert data { hits: [(where-hit $symbol)], total: 1 })
  } else if $shown.kind == "ambiguous" and ($shown.matches? != null) {
    let hits = ($shown.matches | where {|c| is-record $c } | each {|c| where-hit $c })
    finish {
      ok: true
      command: "where"
      data: { hits: $hits, total: ($shown.total? | default ($hits | length)) }
      meta: null
    }
  } else if $shown.kind == "not_found" {
    let found = (call "where" (["find"] ++ (repo-arg $p) ++ ["--" $name]))
    let hits = if $found.ok { $found.data | get-key "hits" | default [] | where {|h| is-record $h } } else { [] }
    if ($hits | is-empty) {
      finish $shown
    } else {
      finish ($found | upsert data { hits: ($hits | each {|h| where-hit $h }), total: ($hits | length) })
    }
  } else {
    finish $shown
  }
}

def calls-command [command: string, args: list<string>, key: string] {
  let line = $"($command) [--limit N] [--repo R] NAME"
  let p = (parse-args $command $line $args [] ["--limit" "--repo"])
  if (printable $p) { finish $p }
  let bad = (one-name $command $line $p)
  if $bad != null { finish $bad }
  let limit = (int-opt $p "--limit")
  if $limit.error != "" { finish (usage $command $"($limit.error). Usage: ($line)") }
  let e = (call $command ([$command] ++ (repo-arg $p) ++ ["--" (symbol-name ($p.pos | first))]))
  if not $e.ok { finish $e }
  finish ($e | upsert data (calls-data $e.data $key $limit.value))
}

def --wrapped "main callers" [...args: string] { calls-command "callers" $args "callers" }

def --wrapped "main callees" [...args: string] { calls-command "callees" $args "callees" }

const IMPACT_SWITCHES = ["--no-tests"]
const IMPACT_VALUED = ["--depth" "--min-confidence" "--repo"]

def impact-args [p: record] {
  (pass $p "--depth") ++ (pass $p "--min-confidence") ++ (switch $p "--no-tests") ++ (repo-arg $p)
}

def --wrapped "main impact" [...args: string] {
  let line = "impact [--depth 1..5] [--limit N] [--min-confidence X] [--no-tests] [--repo R] NAME"
  let p = (parse-args "impact" $line $args $IMPACT_SWITCHES ($IMPACT_VALUED ++ ["--limit"]))
  if (printable $p) { finish $p }
  let bad = (one-name "impact" $line $p)
  if $bad != null { finish $bad }
  let ints = (check-ints "impact" $line $p ["--depth" "--limit"])
  if $ints != null { finish $ints }
  let e = (call "impact" (["impact"] ++ (impact-args $p) ++ (pass $p "--limit") ++ ["--" (symbol-name ($p.pos | first))]))
  if not $e.ok { finish $e }
  let d = $e.data
  finish ($e | upsert data ({ symbol: (symbol-row ($d.symbol? | default null)) } | merge (impact-data $d)))
}

def --wrapped "main diff-impact" [...args: string] {
  let line = "diff-impact [--against REF] [--depth 1..5] [--min-confidence X] [--no-tests] [--repo R]"
  let p = (parse-args "diff-impact" $line $args $IMPACT_SWITCHES ($IMPACT_VALUED ++ ["--against"]))
  if (printable $p) { finish $p }
  if not ($p.pos | is-empty) { finish (usage "diff-impact" $"no name is taken. Usage: ($line)") }
  let ints = (check-ints "diff-impact" $line $p ["--depth"])
  if $ints != null { finish $ints }
  let e = (call "diff-impact" (["impact" "--diff"] ++ (pass $p "--against") ++ (impact-args $p)))
  if not $e.ok { finish $e }
  let d = $e.data
  let head = {
    base: ($d.base? | default null)
    changed: ($d.changed? | default [] | each {|c|
      { display: (at-file ($c.path? | default "") ($c.name? | default "")), change: ($c.change? | default null), in_index: ($c.in_index? | default null) }
    })
    files_only: ($d.files_only? | default [])
    manifest_changed: ($d.manifest_changed? | default null)
  }
  finish ($e | upsert data ($head | merge (impact-data ($d.impact? | default {}))))
}

def --wrapped "main path" [...args: string] {
  let line = "path [--max-depth 1..20] [--repo R] FROM TO"
  let p = (parse-args "path" $line $args [] ["--max-depth" "--repo"])
  if (printable $p) { finish $p }
  if ($p.pos | length) != 2 { finish (usage "path" $"FROM and TO required. Usage: ($line)") }
  let ints = (check-ints "path" $line $p ["--max-depth"])
  if $ints != null { finish $ints }
  let e = (call "path" (["path"] ++ (pass $p "--max-depth") ++ (repo-arg $p) ++ ["--"] ++ ($p.pos | each {|n| symbol-name $n })))
  if not $e.ok { finish $e }
  let d = $e.data
  let hops = ($d.hops? | default [] | each {|h|
    { display: (node $h), edge_kind: ($h.edge_kind? | default null), confidence: ($h.confidence? | default null) }
  })
  let from = (node ($d.from? | default null))
  finish ($e | upsert data {
    found: ($d.found? | default false)
    from: $from
    hops: $hops
    route: ([$from] ++ ($hops | get --optional display | default []) | where {|x| $x != null } | str join " -> ")
    confidence: ($d.confidence? | default null)
    farthest: ($d.farthest? | default null)
  })
}

# Raw counts by default (`--count` says so explicitly); `--rust` is the
# decayed, lift-filtered mode over indexed Rust files.
def --wrapped "main cochange" [...args: string] {
  let line = "cochange [--count | --rust] [--limit N] [--min-lift X (with --rust)] [--repo R] FILE"
  let p = (parse-args "cochange" $line $args ["--count" "--rust"] ["--limit" "--min-lift" "--repo"])
  if (printable $p) { finish $p }
  if ($p.pos | length) != 1 { finish (usage "cochange" $"one file required. Usage: ($line)") }
  let ints = (check-ints "cochange" $line $p ["--limit"])
  if $ints != null { finish $ints }
  let rust = (has $p "--rust")
  if $rust and (has $p "--count") { finish (usage "cochange" $"--count and --rust are two modes; give one. Usage: ($line)") }
  if (not $rust) and (opt $p "--min-lift") != "" { finish (usage "cochange" $"--min-lift needs --rust: raw counts have no lift floor. Usage: ($line)") }
  let mode = if $rust { [] } else { ["--count"] }
  let note = if $rust { $RUST_NOTE } else { $COUNT_NOTE }
  let e = (call "cochange" (["cochange"] ++ $mode ++ (pass $p "--limit") ++ (pass $p "--min-lift") ++ (repo-arg $p) ++ ["--" ($p.pos | first)]))
  if not $e.ok { finish (if $rust { $e | upsert note $note } else { $e }) }
  let d = $e.data
  let partners = if $rust {
    $d.partners? | default [] | each {|x| partner-row $x }
  } else {
    $d.partners? | default [] | each {|x| { path: ($x.path? | default null), count: ($x.count? | default null) } }
  }
  finish ($e | upsert data {
    file: ($d.file? | default null)
    mode: (if $rust { "rust" } else { "count" })
    partners: $partners
    truncated: ($d.truncated? | default false)
    scan: ($d.scan? | default null)
    scope: ($d.scope? | default null)
    note: $note
  })
}

def --wrapped "main map" [...args: string] {
  let line = "map [--budget N] [--focus PATH]... [--detail files|symbols|full] [--repo R]"
  let p = (parse-args "map" $line $args [] ["--budget" "--focus" "--detail" "--repo"])
  if (printable $p) { finish $p }
  if not ($p.pos | is-empty) { finish (usage "map" $"no positional is taken. Usage: ($line)") }
  let ints = (check-ints "map" $line $p ["--budget"])
  if $ints != null { finish $ints }
  let budget = (opt $p "--budget" | if $in == "" { "1500" } else { $in })
  let e = (call "map" (["map" $"--budget=($budget)"] ++ (pass-all $p "--focus") ++ (pass $p "--detail") ++ (repo-arg $p)))
  if not $e.ok { finish $e }
  finish ($e | upsert data ($e.data | reject --optional rank_epoch))
}

def --wrapped "main rank" [...args: string] {
  let line = "rank [--files] [--limit N] [--focus PATH]... [--repo R]"
  let p = (parse-args "rank" $line $args ["--files"] ["--limit" "--focus" "--repo"])
  if (printable $p) { finish $p }
  if not ($p.pos | is-empty) { finish (usage "rank" $"no positional is taken. Usage: ($line)") }
  let ints = (check-ints "rank" $line $p ["--limit"])
  if $ints != null { finish $ints }
  let e = (call "rank" (["rank"] ++ (switch $p "--files") ++ (pass $p "--limit") ++ (pass-all $p "--focus") ++ (repo-arg $p)))
  if not $e.ok { finish $e }
  let d = $e.data
  let hits = ($d.hits? | default [] | each {|h|
    if ($d.kind? | default "") == "files" {
      { path: ($h.path? | default null), score: ($h.score? | default null) }
    } else {
      {
        display: (at-file ($h.path? | default "") ($h.qual_name? | default ($h.name? | default "")))
        kind: ($h.kind? | default null)
        line: ($h.line? | default null)
        score: ($h.score? | default null)
      }
    }
  })
  finish ($e | upsert data { kind: ($d.kind? | default null), focus: ($d.focus? | default []), hits: $hits, truncated: ($d.truncated? | default false) })
}

def --wrapped "main show" [...args: string] {
  let line = "show [--source] [--repo R] NAME"
  let p = (parse-args "show" $line $args ["--source"] ["--repo"])
  if (printable $p) { finish $p }
  let bad = (one-name "show" $line $p)
  if $bad != null { finish $bad }
  let e = (call "show" (["show"] ++ (switch $p "--source") ++ (repo-arg $p) ++ ["--" (symbol-name ($p.pos | first))]))
  if not $e.ok { finish $e }
  let d = $e.data
  let callers = ($d.callers? | default {})
  let callees = ($d.callees? | default {})
  finish ($e | upsert data ($d
    | upsert symbol (where-hit ($d.symbol? | default {}) | merge {
        stable_key: ($d.symbol?.stable_key? | default null)
        end_line: ($d.symbol?.end_line? | default null)
        doc_summary: ($d.symbol?.doc_summary? | default null)
        is_test: ($d.symbol?.is_test? | default null)
      })
    | upsert callers ({ count: ($callers.count? | default null) } | merge (calls-data $callers "callers" null) | reject symbol)
    | upsert callees ({ count: ($callees.count? | default null) } | merge (calls-data $callees "callees" null) | reject symbol)))
}

def --wrapped "main status" [...args: string] {
  let line = "status [--repo R]"
  let p = (parse-args "status" $line $args [] ["--repo"])
  if (printable $p) { finish $p }
  if not ($p.pos | is-empty) { finish (usage "status" $"no positional is taken. Usage: ($line)") }
  let e = (call "status" (["status"] ++ (repo-arg $p)))
  if not $e.ok { finish $e }
  let d = $e.data
  finish ($e | upsert data {
    stale: ($d.stale? | default null)
    head: ($d.head? | default null)
    index_head: ($d.index_head? | default null)
    changes: ($d.changes? | default null)
    counts: ($d.counts? | default null)
    cochange: ($d.cochange? | default null)
    lock: ($d.lock? | default null)
  })
}

def --wrapped "main index" [...args: string] {
  let line = "index [PATH]"
  let p = (parse-args "index" $line $args [] [])
  if (printable $p) { finish $p }
  if ($p.pos | length) > 1 { finish (usage "index" $"at most one path. Usage: ($line)") }
  let path = if ($p.pos | is-empty) { "." } else { $p.pos | first }
  finish (call "index" ["index" "--" $path])
}

def --wrapped "main help" [...args: string] { finish (help-envelope) }

# The command, after any global flags, and the words it is handed: the
# global flags first, then the words that followed the command.
def split-command [args: list<string>] {
  mut i = 0
  mut lead = []
  let n = ($args | length)
  while $i < $n {
    let tok = ($args | get $i)
    let named = (valued-name $tok $GLOBAL_FLAGS)
    if $tok in $HELP_FLAGS {
      return { help: true }
    } else if $named != "" and ($tok | str starts-with $"($named)=") {
      $lead = ($lead | append $tok)
      $i = $i + 1
    } else if $named != "" {
      if ($i + 1) >= $n { return { error: $"($named) needs a value, then a command" } }
      $lead = ($lead | append [$tok ($args | get ($i + 1))])
      $i = $i + 2
    } else if ($tok | str starts-with "-") {
      return { error: $"unknown flag ($tok) before the command; only --repo, --limit and --depth may come first" }
    } else {
      return { command: $tok, rest: ($lead ++ ($args | skip ($i + 1))) }
    }
  }
  if ($lead | is-empty) { { error: "a command is required" } } else { { error: "a command is required after the flags" } }
}

def --wrapped main [...args: string] {
  let known = ($COMMANDS | str join ", ")
  let s = (split-command $args)
  if ($s.help? | default false) { finish (help-envelope) }
  if ($s.error? != null) { finish (usage "" $"($s.error): ($known)") }
  let rest = $s.rest
  match $s.command {
    "help" => { main help ...$rest }
    "where" => { main where ...$rest }
    "callers" => { main callers ...$rest }
    "callees" => { main callees ...$rest }
    "impact" => { main impact ...$rest }
    "diff-impact" => { main diff-impact ...$rest }
    "path" => { main path ...$rest }
    "cochange" => { main cochange ...$rest }
    "map" => { main map ...$rest }
    "rank" => { main rank ...$rest }
    "show" => { main show ...$rest }
    "status" => { main status ...$rest }
    "index" => { main index ...$rest }
    _ => { finish (usage $s.command $"unknown command ($s.command). Commands: ($known)") }
  }
}
