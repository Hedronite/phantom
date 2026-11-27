#![forbid(unsafe_code)]
//! Go source extractor, build-tag evaluator, walk-only module graph, and Tiers 1-3 resolver.

pub mod build_tags;
pub mod extract;
pub mod go_mod;
pub mod resolve;

pub use extract::{GoLanguage, LinknameDirective, parse_linknames};
pub use resolve::GoResolver;
