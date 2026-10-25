//! Temp copies of `fixtures/index/repo` for the walk and index tests.
// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_extract::MAX_FILE_BYTES;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/index/repo");

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-extract-{label}-{}-{nanos}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self {
            path: path.canonicalize().expect("canonical temp dir"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The fixture repo, plus what git cannot carry: a `.git` directory with an
/// exclude rule, a file over the size cap, one exactly at it, and symlinks.
pub struct FixtureRepo {
    outside: TempDir,
    root: TempDir,
}

impl FixtureRepo {
    pub fn new() -> Self {
        let root = TempDir::new("repo");
        let outside = TempDir::new("outside");
        copy_tree(Path::new(FIXTURE), root.path());
        let path = root.path();
        fs::rename(path.join("gitignore"), path.join(".gitignore")).expect("dot gitignore");
        write(path, ".git/HEAD", b"ref: refs/heads/main\n");
        write(path, ".git/config", b"[core]\n\tbare = false\n");
        write(path, ".git/info/exclude", b"/excluded.rs\n");
        write(
            path,
            ".git/objects/pack/fixture.rs",
            b"pub fn inside_git() {}\n",
        );
        write(path, "excluded.rs", b"pub fn excluded_by_info() {}\n");
        write(path, "big.rs", &rust_bytes(BIG_HEAD, MAX_FILE_BYTES + 1));
        write(path, "edge.rs", &rust_bytes(EDGE_HEAD, MAX_FILE_BYTES));
        write(
            outside.path(),
            "elsewhere.rs",
            b"pub fn outside_root() {}\n",
        );
        symlink(path.join("src/ledger.rs"), path.join("link_in.rs")).expect("link in");
        symlink(
            outside.path().join("elsewhere.rs"),
            path.join("link_out.rs"),
        )
        .expect("link out");
        symlink(outside.path(), path.join("linked_dir")).expect("dir link out");
        symlink(path.join("missing.rs"), path.join("dangling.rs")).expect("dangling link");
        Self { outside, root }
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Every entry under the root, `.git` included: file bytes, or the link
    /// target for a symlink.
    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        snapshot_into(self.root(), self.root(), &mut out);
        snapshot_into(self.outside.path(), self.outside.path(), &mut out);
        out
    }
}

fn snapshot_into(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let kind = entry.file_type().expect("file type");
        let key = path.strip_prefix(base).expect("under base").to_path_buf();
        if kind.is_symlink() {
            let target = fs::read_link(&path).expect("read link");
            out.insert(key, target.as_os_str().as_encoded_bytes().to_vec());
        } else if kind.is_dir() {
            out.insert(key, Vec::new());
            snapshot_into(base, &path, out);
        } else {
            out.insert(key, fs::read(&path).expect("read file"));
        }
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            fs::create_dir_all(&target).expect("copy dir");
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// First line of `big.rs`, one byte over the cap.
pub const BIG_HEAD: &[u8] = b"pub fn big_prefix() {}\n";
/// First line of `edge.rs`, exactly at the cap.
pub const EDGE_HEAD: &[u8] = b"pub fn edge_symbol() {}\n";

/// Writes `bytes` to `rel` under `root`, creating parent directories.
pub fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("parent dir");
    fs::write(path, bytes).expect("write file");
}

/// `head`, then Rust line comments, `len` bytes in all.
pub fn rust_bytes(head: &[u8], len: u64) -> Vec<u8> {
    let len = usize::try_from(len).expect("fits usize");
    let line = b"// size cap filler\n";
    let filler = line.iter().copied().cycle();
    let mut out: Vec<u8> = head.iter().copied().chain(filler).take(len).collect();
    if let Some(last) = out.last_mut() {
        *last = b'\n';
    }
    out
}
