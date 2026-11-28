//! Pinned Go build-constraint evaluator (`GOOS=linux`, `GOARCH=amd64`,
//! `unix=true`, `cgo=false`, `gc=true`).

/// Default Go release minor version when no `go.mod` `go 1.N` line is given.
pub const DEFAULT_GO_MINOR: u32 = 26;

const MAX_EXPR_DEPTH: usize = 64;

const KNOWN_GOOS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];

const KNOWN_GOARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "mipsle",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

/// Pinned Go build environment (`linux/amd64`, `unix`, `!cgo`, `gc`, `go1.1..=go1.N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildEnv {
    /// Highest enabled `go1.N` release tag (inclusive).
    pub go_minor: u32,
}

impl Default for BuildEnv {
    fn default() -> Self {
        Self {
            go_minor: DEFAULT_GO_MINOR,
        }
    }
}

impl BuildEnv {
    /// Constructs a pinned environment capped at release `go1.<go_minor>`.
    pub fn with_go_minor(go_minor: u32) -> Self {
        Self { go_minor }
    }

    /// Evaluates one build tag identifier against the pinned configuration.
    pub fn eval_tag(self, tag: &str) -> bool {
        if matches!(tag, "linux" | "amd64" | "unix" | "gc") {
            return true;
        }
        let Some(rest) = tag.strip_prefix("go1.") else {
            return false;
        };
        rest.parse::<u32>()
            .is_ok_and(|minor| minor >= 1 && minor <= self.go_minor)
    }
}

/// Returns true when `path` ends with `_test.go`.
pub fn is_test_filename(path: &str) -> bool {
    file_name(path).ends_with("_test.go")
}

/// Returns true when `path` matches the pinned `linux/amd64` filename suffixes.
pub fn matches_filename(path: &str) -> bool {
    let base = file_name(path);
    let Some(without_ext) = base.strip_suffix(".go") else {
        return false;
    };
    let stem = without_ext.strip_suffix("_test").unwrap_or(without_ext);
    let mut parts = stem.rsplit('_');
    let last = parts.next().unwrap_or("");
    let prev = parts.next().unwrap_or("");
    if KNOWN_GOARCH.contains(&last) {
        let os_ok = !KNOWN_GOOS.contains(&prev) || prev == "linux";
        return last == "amd64" && os_ok;
    }
    !KNOWN_GOOS.contains(&last) || last == "linux"
}

/// Returns true when `source`'s pre-`package` build comments pass `env`.
pub fn matches_directives(source: &str, env: &BuildEnv) -> bool {
    let (go_build, plus_builds) = scan_header_directives(source);
    if let Some(expr) = go_build {
        return eval_go_build(&expr, env);
    }
    if plus_builds.is_empty() {
        return true;
    }
    plus_builds.iter().all(|line| eval_plus_line(line, env))
}

/// Returns true when both filename suffixes and pre-`package` constraints pass.
pub fn matches_file(path: &str, source: &str, env: &BuildEnv) -> bool {
    matches_filename(path) && matches_directives(source, env)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn scan_header_directives(source: &str) -> (Option<String>, Vec<String>) {
    let mut go_build = None;
    let mut plus_builds = Vec::new();
    let mut in_block = false;
    for raw in source.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        let Some(rest) = strip_block_comment(line, &mut in_block) else {
            continue;
        };
        if !record_header_line(rest, &mut go_build, &mut plus_builds) {
            break;
        }
    }
    (go_build, plus_builds)
}

fn strip_block_comment<'a>(line: &'a str, in_block: &mut bool) -> Option<&'a str> {
    if *in_block {
        let (_, after) = line.split_once("*/")?;
        *in_block = false;
        return Some(after.trim());
    }
    if let Some(body) = line.strip_prefix("/*") {
        let Some((_, after)) = body.split_once("*/") else {
            *in_block = true;
            return None;
        };
        return Some(after.trim());
    }
    Some(line)
}

fn record_header_line(
    line: &str,
    go_build: &mut Option<String>,
    plus_builds: &mut Vec<String>,
) -> bool {
    if line.is_empty() {
        return true;
    }
    if let Some(expr) = line.strip_prefix("//go:build") {
        go_build.get_or_insert_with(|| expr.trim().to_owned());
        return true;
    }
    if let Some(expr) = line.strip_prefix("// +build") {
        plus_builds.push(expr.trim().to_owned());
        return true;
    }
    line.starts_with("//")
}

fn eval_plus_line(line: &str, env: &BuildEnv) -> bool {
    let mut any_term = false;
    for term in line.split_whitespace() {
        any_term = true;
        if eval_plus_term(term, env) {
            return true;
        }
    }
    !any_term
}

fn eval_plus_term(term: &str, env: &BuildEnv) -> bool {
    term.split(',').all(|sub| {
        if let Some(tag) = sub.strip_prefix('!') {
            !env.eval_tag(tag)
        } else {
            env.eval_tag(sub)
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    LParen,
    RParen,
    Not,
    And,
    Or,
    Ident(&'a str),
}

fn tokenize(expr: &str) -> Option<Vec<Token<'_>>> {
    let mut rem = expr;
    let mut tokens = Vec::new();
    while !rem.is_empty() {
        rem = rem.trim_start_matches([' ', '\t']);
        if rem.is_empty() {
            break;
        }
        let (tok, rest) = next_token(rem)?;
        tokens.push(tok);
        rem = rest;
    }
    Some(tokens)
}

fn next_token(rem: &str) -> Option<(Token<'_>, &str)> {
    if let Some(rest) = rem.strip_prefix("&&") {
        return Some((Token::And, rest));
    }
    if let Some(rest) = rem.strip_prefix("||") {
        return Some((Token::Or, rest));
    }
    if let Some(tok) = single_char_token(rem.as_bytes()[0]) {
        return Some((tok, &rem[1..]));
    }
    let len = rem.bytes().take_while(|&b| is_tag_byte(b)).count();
    is_tag_byte(rem.as_bytes()[0]).then(|| {
        let (ident, rest) = rem.split_at(len);
        (Token::Ident(ident), rest)
    })
}

fn single_char_token(b: u8) -> Option<Token<'static>> {
    match b {
        b'(' => Some(Token::LParen),
        b')' => Some(Token::RParen),
        b'!' => Some(Token::Not),
        _ => None,
    }
}

fn is_tag_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

struct Parser<'a> {
    tokens: &'a [Token<'a>],
    env: BuildEnv,
}

impl<'a> Parser<'a> {
    fn consume(&mut self, expected: Token<'_>) -> bool {
        if self.tokens.first().copied() == Some(expected) {
            self.tokens = &self.tokens[1..];
            true
        } else {
            false
        }
    }

    fn parse_or(&mut self, depth: usize) -> Option<bool> {
        let mut lhs = self.parse_and(depth)?;
        while self.consume(Token::Or) {
            let rhs = self.parse_and(depth)?;
            lhs = lhs || rhs;
        }
        Some(lhs)
    }

    fn parse_and(&mut self, depth: usize) -> Option<bool> {
        let mut lhs = self.parse_unary(depth)?;
        while self.consume(Token::And) {
            let rhs = self.parse_unary(depth)?;
            lhs = lhs && rhs;
        }
        Some(lhs)
    }

    fn parse_unary(&mut self, depth: usize) -> Option<bool> {
        let (&tok, rest) = self.next_checked(depth)?;
        self.tokens = rest;
        match tok {
            Token::Not => self.parse_not(depth + 1),
            Token::LParen => self.parse_paren(depth + 1),
            Token::Ident(tag) => Some(self.env.eval_tag(tag)),
            _ => None,
        }
    }

    fn parse_not(&mut self, depth: usize) -> Option<bool> {
        self.parse_unary(depth).map(|v| !v)
    }

    fn next_checked(&self, depth: usize) -> Option<(&Token<'a>, &'a [Token<'a>])> {
        (depth < MAX_EXPR_DEPTH)
            .then(|| self.tokens.split_first())
            .flatten()
    }

    fn parse_paren(&mut self, depth: usize) -> Option<bool> {
        let inner = self.parse_or(depth)?;
        self.consume(Token::RParen).then_some(inner)
    }
}

fn eval_go_build(expr: &str, env: &BuildEnv) -> bool {
    let Some(tokens) = tokenize(expr) else {
        return false;
    };
    let mut parser = Parser {
        tokens: &tokens,
        env: *env,
    };
    parser
        .parse_or(0)
        .is_some_and(|ok| parser.tokens.is_empty() && ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_tag_evaluation_follows_pinned_linux_amd64_rules() {
        let env = BuildEnv::with_go_minor(24);
        assert!(env.eval_tag("linux"));
        assert!(env.eval_tag("amd64"));
        assert!(env.eval_tag("unix"));
        assert!(env.eval_tag("gc"));
        assert!(env.eval_tag("go1.1"));
        assert!(env.eval_tag("go1.24"));
        assert!(!env.eval_tag("go1.0"));
        assert!(!env.eval_tag("go1.25"));
        assert!(!env.eval_tag("go1.notnum"));
        assert!(!env.eval_tag("cgo"));
        assert!(!env.eval_tag("darwin"));
        assert!(!env.eval_tag("special"));
        assert!(!env.eval_tag("ignore"));
        assert_eq!(BuildEnv::default().go_minor, DEFAULT_GO_MINOR);
    }

    #[test]
    fn filename_suffixes_filter_os_and_arch_with_optional_test_suffix() {
        assert!(matches_filename("a/b/core.go"));
        assert!(matches_filename("lib/lib_test.go"));
        assert!(matches_filename("lib/os_linux.go"));
        assert!(matches_filename("lib/os_amd64.go"));
        assert!(matches_filename("lib/os_linux_amd64.go"));
        assert!(matches_filename("lib/os_linux_amd64_test.go"));
        assert!(matches_filename("lib/custom_amd64_test.go"));
        assert!(is_test_filename("lib/core_test.go"));
        assert!(!is_test_filename("lib/core.go"));
        assert!(!matches_filename("lib/os_darwin.go"));
        assert!(!matches_filename("lib/os_windows_amd64.go"));
        assert!(!matches_filename("lib/os_linux_arm64.go"));
        assert!(!matches_filename("lib/os_arm64.go"));
        assert!(!matches_filename("lib/os_darwin_test.go"));
        assert!(!matches_filename("go.mod"));
    }

    #[test]
    fn go_build_precedes_plus_build_and_parses_boolean_algebra() {
        let env = BuildEnv::with_go_minor(26);
        assert!(matches_directives(
            "//go:build (linux && amd64) || darwin\n// +build ignore\npackage p\n",
            &env
        ));
        assert!(!matches_directives("//go:build special\npackage p\n", &env));
        assert!(matches_directives("//go:build !my_tag\npackage p\n", &env));
        assert!(!matches_directives(
            "//go:build linux && darwin\npackage p\n",
            &env
        ));
        assert!(!matches_directives(
            "//go:build darwin && linux\npackage p\n",
            &env
        ));
        assert!(matches_directives(
            "//go:build !!((linux))\npackage p\n",
            &env
        ));
        assert!(!matches_directives("//go:build !unix\npackage p\n", &env));
        assert!(matches_directives(
            "\u{feff}/* header comment */\n\n// comment\n//go:build linux && !cgo\npackage p\n",
            &env
        ));
        assert!(matches_directives(
            "/* multi\nline */ package p\n//go:build ignore\n",
            &env
        ));
        assert!(matches_directives(
            "/* closed */ package p\n//go:build ignore\n",
            &env
        ));
        assert!(!matches_directives(
            "/* c */ //go:build darwin\npackage p\n",
            &env
        ));
    }

    #[test]
    fn legacy_plus_build_and_malformed_go_build_edges() {
        let env = BuildEnv::with_go_minor(24);
        assert!(matches_directives(
            "// +build linux,!cgo darwin\n// +build go1.20\npackage p\n",
            &env
        ));
        assert!(!matches_directives(
            "// +build darwin windows\npackage p\n",
            &env
        ));
        assert!(matches_directives("// +build\npackage p\n", &env));
        assert!(!matches_directives("//go:build (linux\npackage p\n", &env));
        assert!(!matches_directives(
            "//go:build linux $ amd64\npackage p\n",
            &env
        ));
        assert!(!matches_directives(
            "//go:build linux amd64\npackage p\n",
            &env
        ));
        assert!(!matches_directives("//go:build )\npackage p\n", &env));
        assert!(!matches_directives("//go:build linux &\npackage p\n", &env));
        assert!(!matches_directives("//go:build linux |\npackage p\n", &env));

        let deep_fail = format!(
            "//go:build {}linux\npackage p\n",
            "!".repeat(MAX_EXPR_DEPTH)
        );
        let deep_ok = format!(
            "//go:build {}linux\npackage p\n",
            "!".repeat(MAX_EXPR_DEPTH - 2)
        );
        let paren_fail = format!(
            "//go:build {}linux{}\npackage p\n",
            "(".repeat(MAX_EXPR_DEPTH),
            ")".repeat(MAX_EXPR_DEPTH)
        );
        assert!(!matches_directives(&deep_fail, &env));
        assert!(matches_directives(&deep_ok, &env));
        assert!(!matches_directives(&paren_fail, &env));
        assert!(matches_file("lib/os_linux.go", "package lib\n", &env));
        assert!(!matches_file("lib/os_darwin.go", "package lib\n", &env));
        assert!(!matches_file(
            "lib/os_linux.go",
            "//go:build ignore\npackage lib\n",
            &env
        ));
    }
}
