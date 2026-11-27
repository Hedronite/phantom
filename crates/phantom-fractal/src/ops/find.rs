//! `find`: indexed symbols by exact name, methods included (SPEC-w14
//! G1b).

use std::collections::BTreeSet;
use std::path::PathBuf;

use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{FindRow, Store, StoreError, SymbolMatch};
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::lookup::Names;
use super::{Failure, Reply, answer, canonical_repo, open_index};

/// Most hits one `find` returns.
const FIND_CAP: usize = 100;

/// What `find` is asked for.
pub(crate) struct FindRequest {
    /// The exact name to look for.
    pub(crate) query: String,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `find`. `truncated` is true when more than
/// [`FIND_CAP`] symbols matched and only the first are in `hits`.
#[derive(Clone, Serialize)]
pub(crate) struct FindResult {
    hits: Vec<SymbolHit>,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

impl Shed for FindResult {
    fn sheddable(&self) -> usize {
        self.hits.len()
    }

    fn shed(&mut self, mut count: usize) {
        drop_last(&mut self.hits, &mut count);
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// One `find` result. `path` is relative to the indexed tree. An item's
/// `name` is its bare name; a method's is qualified, as
/// `crate::meta::Meta::page`, since its bare name alone does not say
/// whose it is.
#[derive(Clone, Serialize)]
struct SymbolHit {
    name: String,
    kind: String,
    visibility: String,
    path: String,
    line: u32,
}

impl SymbolHit {
    /// The hit for one stored row. `names` qualifies a method, and is
    /// read only when a method matched.
    fn new(row: FindRow, store: &Store, names: Option<&Names>) -> Result<Self, Failure> {
        let line = row
            .line
            .and_then(|line| u32::try_from(line).ok())
            .ok_or(StoreError::UnreadableStamp)?;
        let name = format_hit_name(&row, store, names)?;
        Ok(Self {
            name,
            visibility: visibility_text(&row.visibility),
            kind: row.kind,
            path: row.file,
            line,
        })
    }
}

fn format_hit_name(
    row: &FindRow,
    store: &Store,
    names: Option<&Names>,
) -> Result<String, StoreError> {
    let Some(names) = names.filter(|_| row.kind == "method") else {
        return Ok(row.name.clone());
    };
    let raw = names.of(row.id).unwrap_or_else(|| row.name.clone());
    let probe = SymbolMatch {
        id: row.id,
        kind: row.kind.clone(),
        module_path: String::new(),
        qual_name: raw.clone(),
        file: row.file.clone(),
        line: row.line,
    };
    let formatted = store.format_symbol_display(&probe)?;
    let prefix = format!("{}::", row.file);
    Ok(formatted.strip_prefix(&prefix).unwrap_or(&raw).to_owned())
}

/// The stored visibility as `find` has always printed it: `pub`,
/// `pub(crate)`, `private`, or the restriction as written.
fn visibility_text(stored: &str) -> String {
    match stored {
        "public" => "pub".to_owned(),
        "crate" => "pub(crate)".to_owned(),
        other => other
            .strip_prefix("restricted:")
            .unwrap_or(other)
            .to_owned(),
    }
}

/// The store's rows named exactly `request.query` whose `.rs` file the
/// walk of `request.repo` still keeps, as `find` has always read only
/// those files.
fn kept_rows(request: &FindRequest, store: &Store) -> Result<Vec<FindRow>, Failure> {
    let root = canonical_repo(&request.repo)?;
    let walk = phantom_fractal_extract::walk(&root)
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("walk failed: {err}")))?;
    let kept: BTreeSet<&str> = walk.files.iter().map(|file| file.path.as_str()).collect();
    Ok(store
        .symbols_named(&request.query)?
        .into_iter()
        .filter(|row| kept.contains(row.file.as_str()))
        .collect())
}

/// Indexed symbols under `request.repo` named exactly `request.query`,
/// ordered by path and then line, at most [`FIND_CAP`] of them, fitted
/// to `request.max_tokens`.
pub(crate) fn find(request: &FindRequest) -> Result<Reply<FindResult>, Failure> {
    budget::check(request.max_tokens)?;
    let store = open_index(&request.repo)?;
    let result = matches(request, &store)?;
    answer(result, &store, &request.repo, request.max_tokens)
}

/// The matches before any budget. The store's bound FTS match answers an
/// absent name without a walk. Otherwise the store's rows so named are
/// kept when the walk still keeps their `.rs` file.
fn matches(request: &FindRequest, store: &Store) -> Result<FindResult, Failure> {
    let query = request.query.as_str();
    if query.is_empty() || store.match_count(query)? == 0 {
        return Ok(FindResult {
            hits: Vec::new(),
            truncated: false,
            tokens: None,
        });
    }
    let rows = kept_rows(request, store)?;
    let names = if rows.iter().any(|row| row.kind == "method") {
        Some(Names::of_store(store)?)
    } else {
        None
    };
    let mut hits = rows
        .into_iter()
        .map(|row| SymbolHit::new(row, store, names.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = hits.len() > FIND_CAP;
    hits.truncate(FIND_CAP);
    Ok(FindResult {
        hits,
        truncated,
        tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use phantom_fractal_lang::{
        Extract, ExtractError, FileModule, Item, ItemKind, Language, Module, Target, TargetKind,
        Visibility,
    };
    use phantom_fractal_store::{Package, Store};

    use super::{FindRequest, format_hit_name, kept_rows};
    use crate::ops::lookup::Names;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "phantom-find-test-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p.canonicalize().unwrap())
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct GoLang;
    impl Language for GoLang {
        fn name(&self) -> &'static str {
            "go"
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["go"]
        }
        fn qualifier_separator(&self) -> &'static str {
            "."
        }
        fn extract(&self, _s: &str) -> Result<Extract, ExtractError> {
            Ok(Extract {
                items: vec![Item {
                    kind: ItemKind::Struct,
                    name: "Decoder".to_owned(),
                    visibility: Visibility::Public,
                    start_line: 1,
                    end_line: 1,
                    signature: "type Decoder struct".to_owned(),
                    doc_summary: None,
                    is_test: false,
                }],
                item_scopes: vec![String::new()],
                item_owners: vec![None],
                methods: vec![phantom_fractal_lang::Method {
                    owner: "Decoder".to_owned(),
                    owner_path: "Decoder".to_owned(),
                    trait_name: None,
                    name: "Decode".to_owned(),
                    visibility: Visibility::Public,
                    scope: String::new(),
                    start_line: 3,
                    end_line: 3,
                    signature: "func (Decoder) Decode()".to_owned(),
                    doc_summary: None,
                    is_test: false,
                }],
                ..Extract::default()
            })
        }
    }

    #[test]
    fn find_keeps_non_rust_files_and_formats_go_methods_with_dots() {
        let dir = TempDir::new("go");
        fs::create_dir_all(dir.0.join("pkg")).unwrap();
        fs::write(dir.0.join("pkg/dec.go"), "package pkg\n").unwrap();
        let db = dir.0.join("graph.db");
        let mut store = Store::open(&db).unwrap();
        store
            .put_source("pkg/dec.go", "package pkg\n", 0, &GoLang)
            .unwrap();
        store
            .write_crate_graph_with_files(
                &[Package {
                    name: "example.com/mod".to_owned(),
                    version: None,
                    manifest_path: "go.mod".to_owned(),
                }],
                &[Target {
                    id: 1,
                    package: "example.com/mod".to_owned(),
                    name: "pkg".to_owned(),
                    kind: TargetKind::Lib,
                    root: String::new(),
                }],
                &[Module {
                    id: 1,
                    target: 1,
                    parent: None,
                    path: "example.com/mod/pkg".to_owned(),
                    file: String::new(),
                    scope: String::new(),
                }],
                &[FileModule {
                    file: "pkg/dec.go".to_owned(),
                    module: 1,
                }],
                &[],
            )
            .unwrap();

        let req = FindRequest {
            query: "Decode".to_owned(),
            repo: dir.0.clone(),
            max_tokens: None,
        };
        let Ok(rows) = kept_rows(&req, &store) else {
            panic!("kept_rows failed");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file, "pkg/dec.go");
        let Ok(names) = Names::of_store(&store) else {
            panic!("names failed");
        };
        assert_eq!(
            format_hit_name(&rows[0], &store, Some(&names)).unwrap(),
            "example.com/mod/pkg.Decoder.Decode"
        );
        assert_eq!(format_hit_name(&rows[0], &store, None).unwrap(), "Decode");
    }
}
