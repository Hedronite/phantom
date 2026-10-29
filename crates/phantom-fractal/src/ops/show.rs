//! `show`: one symbol's stored detail, its members, a capped look at its
//! callers and callees, and its source only when asked.
//!
//! By name the lookup and its errors are those of `callers`, except that
//! each `Ambiguous` candidate carries its stable key. By stable key the
//! match is exact.

use std::path::PathBuf;

use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::SymbolDetail;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::calls::{CallOut, Direction, edges};
use super::lookup::{Names, current_index, lookup_keyed};
use super::source::{SourceOut, read_source};
use super::{Failure, canonical_repo};

/// Most members one `show` returns.
const MEMBER_CAP: usize = 100;

/// Most callers, and most callees, one `show` returns.
const SHOW_CALL_CAP: usize = 20;

/// Which symbol `show` is asked about.
pub(crate) enum Target {
    /// A bare, in-file, or module-qualified name, as `callers` takes.
    Name(String),
    /// An exact stable key.
    Id(String),
}

/// What `show` is asked for.
pub(crate) struct ShowRequest {
    pub(crate) target: Target,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// Read the symbol's lines from its file. Without it no source text
    /// is read or returned.
    pub(crate) source: bool,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `show`, with its keys in this order. `source`
/// is present only when it was asked for.
#[derive(Clone, Serialize)]
pub(crate) struct ShowResult {
    symbol: DetailOut,
    members: Members,
    callers: Calls,
    callees: Calls,
    macro_args: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<SourceOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

/// Under a budget, `source` goes first as a whole, then the callees, the
/// callers and the members, each from the end. The symbol and the counts
/// stay.
impl Shed for ShowResult {
    fn sheddable(&self) -> usize {
        usize::from(self.source.is_some())
            + self.callees.hits.len()
            + self.callers.hits.len()
            + self.members.hits.len()
    }

    fn shed(&mut self, mut count: usize) {
        if count > 0 && self.source.take().is_some() {
            count -= 1;
        }
        drop_last(&mut self.callees.hits, &mut count);
        drop_last(&mut self.callers.hits, &mut count);
        drop_last(&mut self.members.hits, &mut count);
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// One symbol's detail. `name` is the bare name and `qual_name` the name
/// `callers` prints for it; `path` is relative to the indexed tree.
#[derive(Clone, Serialize)]
struct DetailOut {
    name: String,
    qual_name: String,
    kind: String,
    visibility: String,
    path: String,
    line: Option<i64>,
    end_line: Option<i64>,
    signature: Option<String>,
    doc_summary: Option<String>,
    is_test: bool,
    stable_key: String,
}

/// The methods attributed to a struct, enum, or trait: at most
/// [`MEMBER_CAP`], with `truncated` set when there were more.
#[derive(Clone, Serialize)]
struct Members {
    hits: Vec<DetailOut>,
    truncated: bool,
}

/// One side of the symbol's edges: `count` of them, of which `hits` holds
/// the first [`SHOW_CALL_CAP`] in the `callers` or `callees` order.
#[derive(Clone, Serialize)]
struct Calls {
    count: usize,
    hits: Vec<CallOut>,
    truncated: bool,
    /// Call, method-call, and macro references in the symbol with no edge.
    /// `callees` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    unresolved_calls: Option<i64>,
}

/// The detail of the one symbol `request.target` names in the index of
/// `request.repo`, fitted to `request.max_tokens`.
pub(crate) fn show(request: &ShowRequest) -> Result<ShowResult, Failure> {
    budget::check(request.max_tokens)?;
    let (store, names) = current_index(&request.repo)?;
    let id = match &request.target {
        Target::Name(query) => lookup_keyed(&store, &names, query)?.id,
        Target::Id(key) => store
            .symbol_by_stable_key(key)?
            .ok_or_else(|| Failure::new(ErrorKind::NotFound, "no symbol has that stable key"))?,
    };
    let detail = store
        .symbol_detail(id)?
        .ok_or_else(|| Failure::new(ErrorKind::NotFound, "the symbol left the index"))?;
    let mut members = store.members(id)?;
    let members_truncated = members.len() > MEMBER_CAP;
    members.truncate(MEMBER_CAP);
    let callers = calls(
        edges(&store, &names, Direction::Callers, id, &detail.path)?,
        None,
    );
    let callees = calls(
        edges(&store, &names, Direction::Callees, id, &detail.path)?,
        Some(store.unresolved_calls(id)?),
    );
    let macro_args = store.macro_args_named(&detail.name)?;
    let source = if request.source {
        let root = canonical_repo(&request.repo)?;
        Some(read_source(&store, &root, &detail)?)
    } else {
        None
    };
    let result = ShowResult {
        symbol: detail_out(&names, detail),
        members: Members {
            hits: members
                .into_iter()
                .map(|member| detail_out(&names, member))
                .collect(),
            truncated: members_truncated,
        },
        callers,
        callees,
        macro_args,
        source,
        tokens: None,
    };
    budget::fit(result, request.max_tokens)
}

fn calls(mut hits: Vec<CallOut>, unresolved_calls: Option<i64>) -> Calls {
    let count = hits.len();
    let truncated = count > SHOW_CALL_CAP;
    hits.truncate(SHOW_CALL_CAP);
    Calls {
        count,
        hits,
        truncated,
        unresolved_calls,
    }
}

/// A detail row as `show` prints it. A symbol the resolver input does not
/// hold keeps its stored in-file `qual_name`.
fn detail_out(names: &Names, detail: SymbolDetail) -> DetailOut {
    DetailOut {
        qual_name: names.of(detail.id).unwrap_or(detail.qual_name),
        name: detail.name,
        kind: detail.kind,
        visibility: visibility_text(&detail.visibility),
        path: detail.path,
        line: detail.start_line,
        end_line: detail.end_line,
        signature: detail.signature,
        doc_summary: detail.doc_summary,
        is_test: detail.is_test,
        stable_key: detail.stable_key,
    }
}

/// The stored visibility in `find`'s spelling: `private`, `pub`,
/// `pub(crate)`, or the restricted text such as `pub(super)`.
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
