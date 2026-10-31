//! The SCIP wire score (W10 G1): the hand-written reader and the span
//! matcher the two test drivers share. No protobuf or SCIP crate —
//! the wire format here is read by hand from the field numbers of
//! `scip.proto` (sourcegraph/scip), std only.
//!
//! The two drivers share the module but not its whole surface: the read
//! driver uses only the wire reader, the scorer both halves — hence the
//! dead-code allowance the module carries.

#![allow(dead_code)]

pub mod score;
pub mod wire;
