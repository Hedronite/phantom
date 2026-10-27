//! Directory chain work: W2's `0700` and `0600` rows, plus the W3
//! symlink refusal and effective-uid rows.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_lang_rust::RustLanguage;

use super::{TempData, TempDb, assert_outside_workspace, file_mode, fixture, mkdir_explicit};
use crate::Store;

#[test]
#[cfg(unix)]
fn fresh_data_directories_are_created_0700() {
    let data = TempData::new("dirs");
    let db = data.db();
    let store = Store::open(&db).expect("open");
    assert_eq!(
        store.producer().expect("stamp").as_deref(),
        Some(crate::PRODUCER)
    );
    for dir in [
        data.root.as_path(),
        data.root.join("repos").as_path(),
        data.repo_dir().as_path(),
    ] {
        assert_eq!(file_mode(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(file_mode(&db), 0o600, "graph.db");
}

#[test]
#[cfg(unix)]
fn open_tightens_repos_directories_and_leaves_the_data_root() {
    let data = TempData::new("tighten");
    mkdir_explicit(&data.root, 0o751);
    let repos = data.root.join("repos");
    mkdir_explicit(&repos, 0o755);
    let repo_dir = repos.join("ledger-cafe1111");
    mkdir_explicit(&repo_dir, 0o757);
    let db = repo_dir.join("graph.db");
    Store::open(&db).expect("first open creates the db");
    // A stray user chmod on the repo directory is narrowed on the next
    // open of an existing database.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&repo_dir, std::fs::Permissions::from_mode(0o757)).expect("widen");
    }
    Store::open(&db).expect("second open");
    assert_eq!(file_mode(&data.root), 0o751, "data root");
    assert_eq!(file_mode(&repos), 0o700, "repos");
    assert_eq!(file_mode(&repo_dir), 0o700, "repo dir");
}

#[test]
#[cfg(unix)]
fn open_leaves_a_wider_non_repos_directory_alone() {
    let db = TempDb::new("plain");
    assert_outside_workspace(db.path());
    // The temp dir already exists with a wider umask-carried mode; opening
    // a plain database under it must not change that parent at all.
    let before = file_mode(db.path().parent().expect("parent"));
    Store::open(db.path()).expect("open");
    assert_eq!(
        file_mode(db.path().parent().expect("parent")),
        before,
        "parent mode changed"
    );
}

#[test]
#[cfg(unix)]
fn database_and_wal_files_are_0600_after_index() {
    let data = TempData::new("wal");
    let db = data.db();
    let mut store = Store::open(&db).expect("open");
    let language = RustLanguage::new().expect("grammar");
    let (source, mtime_ns) = fixture();
    store
        .put_source("src/lib.rs", &source, mtime_ns, &language)
        .expect("put");
    let text = db.to_string_lossy().into_owned();
    for suffix in ["", "-wal", "-shm"] {
        let path = format!("{text}{suffix}");
        assert_eq!(file_mode(Path::new(&path)), 0o600, "{path}");
    }
}

#[test]
#[cfg(unix)]
fn a_symlinked_repos_refuses_open_without_chmod() {
    let data = TempData::new("symlink-repos");
    mkdir_explicit(&data.root, 0o755);
    let target = data.root.join("target");
    mkdir_explicit(&target, 0o755);
    std::os::unix::fs::symlink(&target, data.root.join("repos")).expect("symlink");
    let opened = Store::open(&data.db());
    assert!(opened.is_err(), "open must refuse a symlinked repos/");
    assert_eq!(file_mode(&target), 0o755, "target mode");
    assert!(
        fs::read_dir(&target).expect("target").next().is_none(),
        "nothing was created through the link"
    );
}

#[test]
#[cfg(unix)]
fn a_symlinked_repo_directory_refuses_open_without_chmod() {
    let data = TempData::new("symlink-repo");
    mkdir_explicit(&data.root, 0o755);
    mkdir_explicit(&data.root.join("repos"), 0o755);
    let target = data.root.join("target");
    mkdir_explicit(&target, 0o744);
    std::os::unix::fs::symlink(&target, data.repo_dir()).expect("symlink");
    let opened = Store::open(&data.db());
    assert!(opened.is_err(), "open must refuse a symlinked repo dir");
    assert_eq!(file_mode(&target), 0o744, "target mode");
    assert!(
        fs::read_dir(&target).expect("target").next().is_none(),
        "nothing was created through the link"
    );
}

#[test]
#[cfg(unix)]
fn non_directory_repos_refuses_open() {
    let data = TempData::new("file-repos");
    mkdir_explicit(&data.root, 0o755);
    fs::write(data.root.join("repos"), b"not a directory").expect("write");
    let opened = Store::open(&data.db());
    assert!(opened.is_err());
}

#[test]
#[cfg(unix)]
fn owner_comparison_rejects_a_mismatched_uid() {
    use std::os::unix::fs::MetadataExt;
    let probe = unique_path("owner-compare");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .expect("probe file");
    let meta = fs::metadata(&probe).expect("metadata");
    let uid = meta.uid();
    assert!(!crate::dirs::owned_by(&meta, 4242));
    assert!(crate::dirs::owned_by(&meta, uid));
    fs::remove_file(&probe).expect("cleanup");
}

#[test]
#[cfg(unix)]
fn the_real_owner_path_uses_the_effective_uid() {
    let data = TempData::new("owner-real");
    mkdir_explicit(&data.root, 0o755);
    mkdir_explicit(&data.root.join("repos"), 0o755);
    let repo_dir = data.root.join("repos").join("ledger-cafe1111");
    mkdir_explicit(&repo_dir, 0o700);
    let chain = crate::dirs::Chain::new(&data.db()).expect("chain");
    let probe = repo_dir.join(".owner-check-probe");
    let uid = crate::dirs::effective_uid(&repo_dir, &probe).expect("probe");
    assert!(!probe.exists(), "probe was removed");
    crate::dirs::verify_owned(&chain, uid).expect("the chain is our own");
    let refused = crate::dirs::verify_owned(&chain, 4242);
    assert!(refused.is_err(), "a foreign effective uid must refuse");
}

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "phantom-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0)
    ))
}
