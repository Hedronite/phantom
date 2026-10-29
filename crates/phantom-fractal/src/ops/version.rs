//! `version`: what this binary is, as an envelope. Every field is fixed
//! when the binary is built, so it opens no store, reads no data
//! directory, and works in any directory.

use serde::Serialize;

use super::Failure;

/// The commit `build.rs` found, or empty when it found none.
const GIT_SHA: &str = env!("PHANTOM_FRACTAL_BUILD_GIT_SHA");

/// `data` of `version`, with its fields in this order. A consumer ignores
/// fields it does not know.
#[derive(Serialize)]
pub(crate) struct VersionInfo {
    /// `phantom-fractal`, under either binary name.
    name: &'static str,
    version: &'static str,
    /// 40 lowercase hex characters, or null when the build had no commit.
    git_sha: Option<&'static str>,
    extractor_version: u32,
    store_schema: i64,
}

/// What `version` is asked for: nothing.
pub(crate) struct VersionRequest;

/// What this binary is. It never fails.
pub(crate) fn version(_request: &VersionRequest) -> Result<VersionInfo, Failure> {
    Ok(VersionInfo {
        name: "phantom-fractal",
        version: env!("CARGO_PKG_VERSION"),
        git_sha: (!GIT_SHA.is_empty()).then_some(GIT_SHA),
        extractor_version: phantom_fractal_extract::EXTRACTOR_VERSION,
        store_schema: phantom_fractal_store::SCHEMA_VERSION,
    })
}
