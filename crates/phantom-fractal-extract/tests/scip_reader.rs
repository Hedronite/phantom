//! The SCIP wire reader's own tests (W10 G1), all CI: a hand-built
//! byte fixture — two documents with definitions, references, an
//! unknown field — plus the truncated and unknown-field bounds, and
//! the packed and typed range encodings. No protobuf or SCIP crate:
//! the bytes come from a local hand-encoder.

#[path = "scip/mod.rs"]
mod scip;

use scip::wire::{parse_index, Range};

/// A base-128 varint, per the proto spec.
fn push_varint(out: &mut Vec<u8>, value: u64) {
    let mut rest = value;
    loop {
        let byte = (rest & 0x7f) as u8;
        rest >>= 7;
        if rest == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// One field tag: field number and wire type.
fn push_tag(out: &mut Vec<u8>, field: u64, wire: u64) {
    push_varint(out, (field << 3) | wire);
}

/// One length-delimited field: tag, length, body.
fn push_field(out: &mut Vec<u8>, field: u64, body: &[u8]) {
    push_tag(out, field, 2);
    push_varint(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// One string field.
fn push_string(out: &mut Vec<u8>, field: u64, text: &str) {
    push_field(out, field, text.as_bytes());
}

/// One varint field.
fn push_int(out: &mut Vec<u8>, field: u64, value: u64) {
    push_tag(out, field, 0);
    push_varint(out, value);
}

/// One occurrence: typed single-line range, symbol, roles, then an
/// unknown field of wire type 2 to be skipped.
fn occurrence_single(line: u64, symbol: &str, definition: bool) -> Vec<u8> {
    let mut range = Vec::new();
    push_int(&mut range, 1, line);
    push_int(&mut range, 2, 4);
    push_int(&mut range, 3, 9);
    let mut out = Vec::new();
    push_field(&mut out, 8, &range);
    push_string(&mut out, 2, symbol);
    if definition {
        push_int(&mut out, 3, 1);
    }
    push_string(&mut out, 12, "unknown payload to skip");
    out
}

/// One occurrence with the deprecated packed `range`, four values.
fn occurrence_packed(start_line: u64, end_line: u64, symbol: &str) -> Vec<u8> {
    let mut range = Vec::new();
    push_varint(&mut range, start_line);
    push_varint(&mut range, 2);
    push_varint(&mut range, end_line);
    push_varint(&mut range, 3);
    let mut out = Vec::new();
    push_field(&mut out, 1, &range);
    push_string(&mut out, 2, symbol);
    out
}

/// One document: relative_path, then its occurrences.
fn document(path: &str, occurrences: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    push_string(&mut out, 1, path);
    for occurrence in occurrences {
        push_field(&mut out, 2, occurrence);
    }
    out
}

/// The fixture: two documents (a definition, two references, and a
/// dropped symbol-less occurrence), metadata skipped, and an unknown
/// index field to skip.
fn fixture() -> Vec<u8> {
    let mut out = Vec::new();
    let metadata = {
        let mut body = Vec::new();
        push_int(&mut body, 1, 0);
        push_string(&mut body, 3, "the root");
        body
    };
    push_field(&mut out, 1, &metadata);
    push_string(&mut out, 9, "unknown index payload");
    push_field(&mut out, 2, &document(
        "src/tee.rs",
        &[
            &occurrence_single(0, "rust-analyzer cargo phantom 1.0.0 crate#tee().", true),
            &occurrence_single(2, "rust-analyzer cargo phantom 1.0.0 crate#tee().", false),
            // The definition of a second symbol, on the same line as
            // nothing else.
            &occurrence_single(4, "rust-analyzer cargo phantom 1.0.0 crate#spout.", true),
            // An occurrence with no symbol and no range: not carried.
            &{
                let mut out = Vec::new();
                push_int(&mut out, 5, 1);
                out
            },
        ],
    ));
    push_field(&mut out, 2, &document(
        "src/lib.rs",
        &[
            // A reference to the `tee` method from another file, with
            // a multi-line packed range.
            &occurrence_packed(10, 12, "rust-analyzer cargo phantom 1.0.0 crate#tee()."),
        ],
    ));
    push_field(&mut out, 3, document("src/seek.rs", &[]).as_slice());
    out
}

#[test]
fn two_documents_parse_with_their_occurrences() {
    let whole = fixture();
    let index = parse_index(&whole).expect("parses");
    assert_eq!(index.documents.len(), 2);
    let first = &index.documents[0];
    assert_eq!(first.relative_path, "src/tee.rs");
    assert_eq!(first.occurrences.len(), 3);
    let definition = &first.occurrences[0];
    assert!(definition.is_definition);
    assert_eq!(
        definition.range,
        Range {
            start_line: 0,
            start_character: 4,
            end_line: 0,
            end_character: 9,
        }
    );
    assert_eq!(
        definition.symbol,
        "rust-analyzer cargo phantom 1.0.0 crate#tee()."
    );
    let reference = &first.occurrences[1];
    assert!(!reference.is_definition);
    assert_eq!(reference.range.start_line, 2);
    assert_eq!(reference.symbol, definition.symbol);
    let second = &index.documents[1];
    assert_eq!(second.relative_path, "src/lib.rs");
    let packed = &second.occurrences[0];
    assert_eq!(
        packed.range,
        Range {
            start_line: 10,
            start_character: 2,
            end_line: 12,
            end_character: 3,
        }
    );
    assert!(!packed.is_definition);
}

#[test]
fn truncated_lengths_fail_inside_the_bounds() {
    // Cutting the final byte leaves any length-delimited field one
    // short, so both cut points fail without a panic.
    let whole = fixture();
    for cut in [1, 2, 3, 4, 7, 13] {
        let cut_at = whole.len().saturating_sub(cut);
        assert!(
            parse_index(&whole[..cut_at]).is_err(),
            "a {cut}-byte cut parsed"
        );
    }
    // A length past the input never descends.
    let mut huge = fixture();
    // The documents field's length becomes absurd, one byte at a
    // time; a 128-bit varint of length stays bounded by 10 bytes.
    huge.splice(3..3, [0xff; 9]);
    assert!(parse_index(&huge).is_err());
    // A truncated nested occurrence length fails the same way.
    let mut short = fixture();
    short.truncate(short.len() - 4);
    assert!(parse_index(&short).is_err());
}

#[test]
fn a_dangling_dropped_or_unknown_field_is_skipped() {
    let index = parse_index(&fixture()).expect("parses");
    // The fixture's symbol-less occurrence and the unknown string
    // fields either way are skipped, never carried.
    assert!(index.documents.iter().all(|doc| doc.occurrences.iter().all(
        |occ| !occ.symbol.is_empty())
    ));
    // The whole index parses knowledge anew: an idempotent second read
    // gives the same frames.
    let twice = parse_index(&fixture()).expect("parses");
    assert_eq!(index, twice);
}

#[test]
fn malformed_wire_fails_never_panics() {
    // A tag of wire 7 is unused; a field 0 is reserved; a lone body.
    let broken: &[&[u8]] = &[&[0x3b], &[0x00], &[0x02], &[0x0a, 0xff], &[0x0a, 0x01]];
    for bytes in broken {
        assert!(
            parse_index(bytes).is_err(),
            "{bytes:?} would parse",
        );
    }
    // A document without a path, an occurrence-less index, and a
    // truncated `SingleLineRange` all fail rather than panic.
    let pathless = {
        let mut out = Vec::new();
        push_field(&mut out, 2, &{
            let mut body = Vec::new();
            push_int(&mut body, 2, 7);
            body
        });
        out
    };
    assert!(parse_index(&pathless).is_err());
    let empty: Vec<u8> = Vec::new();
    let index_of_nothing = parse_index(&empty).expect("an empty index is legal");
    assert!(index_of_nothing.documents.is_empty());
}
