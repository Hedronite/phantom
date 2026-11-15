//! `doctor`'s `agents` key (SPEC-w14 G2c): whether `nu` and the
//! `phantom` skill are found. It is informational and never changes
//! `doctor`'s `ok`.
//!
//! It reads only the environment and file metadata. It starts no process
//! (so it cannot say which `nu` version is installed), and it names a
//! place by a label, never by its path.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The skill directories the harnesses read, under a project or a home.
const SKILL_DIRS: [&str; 4] = [
    ".agents/skills/phantom",
    ".claude/skills/phantom",
    ".cursor/skills/phantom",
    ".grok/skills/phantom",
];

/// The files a skill directory must hold to count.
const SKILL_FILES: [&str; 2] = ["SKILL.md", "phantom.nu"];

/// `data.agents` of `doctor`.
#[derive(Serialize)]
pub(crate) struct Agents {
    nu: Nu,
    skill: Skill,
}

/// Whether `nu` is found, and where it was looked for: `PHANTOM_NU` when
/// that is set, else `PATH`, as the skill tests look.
#[derive(Serialize)]
struct Nu {
    found: bool,
    via: &'static str,
}

/// Whether a `phantom` skill is found, and each place it is, as
/// `./.agents/skills/phantom` under the working directory or
/// `~/.claude/skills/phantom` under the home directory.
#[derive(Serialize)]
struct Skill {
    found: bool,
    places: Vec<String>,
}

/// What the lookup reads: the two variables and the two roots.
pub(crate) struct Lookup {
    pub(crate) phantom_nu: Option<OsString>,
    pub(crate) path: Option<OsString>,
    pub(crate) here: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
}

impl Lookup {
    /// This process's `PHANTOM_NU`, `PATH`, working directory and `HOME`.
    pub(crate) fn from_env() -> Self {
        Self {
            phantom_nu: std::env::var_os("PHANTOM_NU"),
            path: std::env::var_os("PATH"),
            here: std::env::current_dir().ok(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

/// The `agents` report for `lookup`.
pub(crate) fn agents(lookup: &Lookup) -> Agents {
    Agents {
        nu: nu(lookup),
        skill: skill(lookup),
    }
}

fn nu(lookup: &Lookup) -> Nu {
    if let Some(over) = lookup.phantom_nu.as_ref().filter(|over| !over.is_empty()) {
        return Nu {
            found: executable(Path::new(over)),
            via: "PHANTOM_NU",
        };
    }
    let found = lookup
        .path
        .as_ref()
        .is_some_and(|path| std::env::split_paths(path).any(|dir| executable(&dir.join("nu"))));
    Nu { found, via: "PATH" }
}

fn skill(lookup: &Lookup) -> Skill {
    let roots = [(".", &lookup.here), ("~", &lookup.home)];
    let places: Vec<String> = roots
        .into_iter()
        .filter_map(|(label, root)| root.as_deref().map(|root| (label, root)))
        .flat_map(|(label, root)| {
            SKILL_DIRS
                .into_iter()
                .filter(move |dir| has_skill(&root.join(dir)))
                .map(move |dir| format!("{label}/{dir}"))
        })
        .collect();
    Skill {
        found: !places.is_empty(),
        places,
    }
}

/// True when `dir` holds every [`SKILL_FILES`] entry as a file, links
/// followed.
fn has_skill(dir: &Path) -> bool {
    SKILL_FILES
        .iter()
        .all(|file| fs::metadata(dir.join(file)).is_ok_and(|meta| meta.is_file()))
}

/// True when `path` is a file, links followed, that someone may run.
fn executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{Lookup, agents};

    /// A fresh directory under the system temp dir, removed on drop.
    struct Temp(PathBuf);

    impl Temp {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "phantom-fractal-agents-{label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file(path: &Path, mode: u32) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, "").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode");
        }
        #[cfg(not(unix))]
        let _ = mode;
    }

    fn report(lookup: &Lookup) -> serde_json::Value {
        serde_json::to_value(agents(lookup)).expect("json")
    }

    fn none() -> Lookup {
        Lookup {
            phantom_nu: None,
            path: None,
            here: None,
            home: None,
        }
    }

    #[test]
    fn nothing_set_finds_nothing() {
        assert_eq!(
            report(&none()),
            serde_json::json!({
                "nu": { "found": false, "via": "PATH" },
                "skill": { "found": false, "places": [] },
            })
        );
    }

    #[test]
    fn nu_on_path_must_be_an_executable_file() {
        let temp = Temp::new("path");
        let bin = temp.0.join("bin");
        let other = temp.0.join("other");
        file(&bin.join("nu"), 0o755);
        file(&other.join("nu"), 0o644);
        fs::create_dir_all(other.join("dir/nu")).expect("dir");
        let lookup = |dirs: &[&Path]| Lookup {
            path: Some(std::env::join_paths(dirs).expect("join")),
            ..none()
        };
        assert_eq!(report(&lookup(&[&other, &bin]))["nu"]["found"], true);
        #[cfg(unix)]
        assert_eq!(report(&lookup(&[&other]))["nu"]["found"], false);
        assert_eq!(report(&lookup(&[&other.join("dir")]))["nu"]["found"], false);
    }

    #[test]
    fn phantom_nu_wins_over_path_and_must_name_an_executable() {
        let temp = Temp::new("over");
        let bin = temp.0.join("bin");
        file(&bin.join("nu"), 0o755);
        let path = Some(bin.clone().into_os_string());
        let over = |value: PathBuf| Lookup {
            phantom_nu: Some(value.into_os_string()),
            path: path.clone(),
            ..none()
        };
        let named = report(&over(bin.join("nu")));
        assert_eq!(
            named["nu"],
            serde_json::json!({ "found": true, "via": "PHANTOM_NU" })
        );
        let missing = report(&over(temp.0.join("absent")));
        assert_eq!(
            missing["nu"],
            serde_json::json!({ "found": false, "via": "PHANTOM_NU" })
        );
        let empty = Lookup {
            phantom_nu: Some("".into()),
            path,
            ..none()
        };
        assert_eq!(
            report(&empty)["nu"],
            serde_json::json!({ "found": true, "via": "PATH" })
        );
    }

    #[test]
    fn a_skill_counts_with_both_files_here_or_at_home() {
        let here = Temp::new("here");
        let home = Temp::new("home");
        file(&here.0.join(".agents/skills/phantom/SKILL.md"), 0o644);
        file(&here.0.join(".agents/skills/phantom/phantom.nu"), 0o644);
        file(&here.0.join(".cursor/skills/phantom/SKILL.md"), 0o644);
        file(&home.0.join(".claude/skills/phantom/SKILL.md"), 0o644);
        file(&home.0.join(".claude/skills/phantom/phantom.nu"), 0o644);
        let lookup = Lookup {
            here: Some(here.0.clone()),
            home: Some(home.0.clone()),
            ..none()
        };
        assert_eq!(
            report(&lookup)["skill"],
            serde_json::json!({
                "found": true,
                "places": ["./.agents/skills/phantom", "~/.claude/skills/phantom"],
            })
        );
    }
}
