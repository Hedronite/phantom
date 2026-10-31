//! A hand-written reader for the slice of the SCIP wire format the
//! scorer uses (SPEC-w10 G1) — `Index.documents`, each
//! `Document.relative_path` and its `occurrences`, and each
//! `Occurrence`'s `range` (both encodings), `symbol` and
//! `symbol_roles`'s Definition bit. Unknown fields are skipped by wire
//! type; malformed input is an error and never a panic; every length
//! is bounded by the bytes of the input.

/// One source range, half-open `[start, end)`, 0-based. The deprecated
/// field and the typed encoders carry three or four 0-based values:
/// three mean single-line, four span lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub start_line: i64,
    pub start_character: i64,
    pub end_line: i64,
    pub end_character: i64,
}

/// One occurrence of the field set the matcher reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub range: Range,
    pub symbol: String,
    /// `symbol_roles`'s Definition bit (`0x1`) is this.
    pub is_definition: bool,
}

/// One document of the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub relative_path: String,
    pub occurrences: Vec<Occurrence>,
}

/// The parts of a SCIP index the scorer reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScipIndex {
    pub documents: Vec<Document>,
}

/// A parse failure: malformed wire, a length past the bytes, or a tag
/// no wire type decodes. Never a panic.
#[derive(Debug, Clone, PartialEq)]
pub struct Malformed {
    /// What broke, without quoting any bytes.
    pub reason: &'static str,
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

const VARINT: u32 = 0;
const FIXED64: u32 = 1;
const LENGTH: u32 = 2;
const FIXED32: u32 = 5;

/// A cursor over one message body; every read is checked, bounded,
/// and advancing.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    /// One byte, or a failure at the end of the input.
    fn byte(&mut self, reason: &'static str) -> Result<u8, Malformed> {
        let byte = self
            .bytes
            .get(self.at)
            .copied()
            .ok_or(Malformed { reason })?;
        self.at += 1;
        Ok(byte)
    }

    /// One base-128 varint, bounded at ten bytes.
    fn varint(&mut self, reason: &'static str) -> Result<u64, Malformed> {
        let mut value: u64 = 0;
        for shift in 0..10 {
            let byte = self.byte(reason)?;
            value |= u64::from(byte & 0x7f) << (7 * shift);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Malformed { reason })
    }

    /// One length-delimited payload, cropped by the remaining input.
    fn payload(&mut self, reason: &'static str) -> Result<&'a [u8], Malformed> {
        let length = usize::try_from(self.varint(reason)?).map_err(|_| Malformed { reason })?;
        if length > self.remaining() {
            return Err(Malformed { reason });
        }
        let body = &self.bytes[self.at..self.at + length];
        self.at += length;
        Ok(body)
    }

    /// Skips one field without descending into it.
    fn skip_field(&mut self, wire: u32, reason: &'static str) -> Result<(), Malformed> {
        match wire {
            VARINT => {
                self.varint(reason)?;
            }
            FIXED64 => {
                if self.remaining() < 8 {
                    return Err(Malformed { reason });
                }
                self.at += 8;
            }
            LENGTH => {
                self.payload(reason)?;
            }
            FIXED32 => {
                if self.remaining() < 4 {
                    return Err(Malformed { reason });
                }
                self.at += 4;
            }
            _ => return Err(Malformed { reason }),
        }
        Ok(())
    }
}

/// One protobuf field: the field number, the wire type, and the
/// payload view of a length-delimited field, dropped past the input.
fn next_field<'c>(cursor: &mut Cursor<'c>) -> Result<(u64, u32, Option<&'c [u8]>), Malformed> {
    const REASON: &str = "malformed tag";
    let tag = cursor.varint(REASON)?;
    let field = tag >> 3;
    let wire = match tag & 0x7 {
        0 => VARINT,
        1 => FIXED64,
        2 => LENGTH,
        5 => FIXED32,
        _ => return Err(Malformed { reason: REASON }),
    };
    if field == 0 {
        return Err(Malformed { reason: REASON });
    }
    let payload = match wire {
        LENGTH => Some(cursor.payload(REASON)?),
        _ => None,
    };
    Ok((field, wire, payload))
}

/// The deprecated `repeated int32 range`: packed or not, a small body
/// is a run of varints either way. Three values are single-line; four
/// span lines.
fn range_from_run(run: &[i64]) -> Result<Range, Malformed> {
    const REASON: &str = "malformed range";
    match run {
        [start_line, start_character, end_character] => Ok(Range {
            start_line: *start_line,
            start_character: *start_character,
            end_line: *start_line,
            end_character: *end_character,
        }),
        [start_line, start_character, end_line, end_character] => Ok(Range {
            start_line: *start_line,
            start_character: *start_character,
            end_line: *end_line,
            end_character: *end_character,
        }),
        _ => Err(Malformed { reason: REASON }),
    }
}

/// An i64 from a bounded varint.
fn bounded(value: u64) -> Result<i64, Malformed> {
    i64::try_from(value)
        .ok()
        .filter(|value| *value >= 0)
        .ok_or(Malformed {
            reason: "malformed range",
        })
}

/// One occurrence body: `range` (field 1), the typed `single_line`
/// (8) or `multi_line` (9) range, `symbol` (2) and `symbol_roles` (3);
/// the rest skipped by wire type. An occurrence without a symbol or a
/// range is not carried — it carries no call either way.
pub(crate) fn parse_occurrence(body: &[u8]) -> Result<Option<Occurrence>, Malformed> {
    const REASON: &str = "malformed occurrence";
    let mut cursor = Cursor::new(body);
    let mut range: Option<Range> = None;
    let mut symbol: Option<String> = None;
    let mut roles: u64 = 0;
    while !cursor.done() {
        let (field, wire, payload) = next_field(&mut cursor)?;
        match (field, wire) {
            (1, LENGTH) => {
                let mut run = Cursor::new(payload.expect("length"));
                let mut values = Vec::new();
                while !run.done() {
                    values.push(bounded(run.varint(REASON)?)?);
                }
                let parsed = range_from_run(&values)?;
                if range.is_none() {
                    range = Some(parsed);
                }
            }
            (8, LENGTH) => {
                let single = parse_single_line(payload.expect("length"))?;
                if range.is_none() {
                    range = Some(single);
                }
            }
            (9, LENGTH) => {
                let multi = parse_multi_line(payload.expect("length"))?;
                if range.is_none() {
                    range = Some(multi);
                }
            }
            (2, LENGTH) => {
                let text = std::str::from_utf8(payload.expect("length"))
                    .map_err(|_| Malformed { reason: REASON })?;
                symbol = Some(text.to_owned());
            }
            (3, VARINT) => {
                roles |= cursor.varint(REASON)?;
            }
            // `next_field` already consumed a LENGTH field's payload;
            // only the unconsumed shapes skip.
            _ => {
                if payload.is_none() {
                    cursor.skip_field(wire, REASON)?;
                }
            }
        }
    }
    Ok(range.zip(symbol).map(|(range, symbol)| Occurrence {
        range,
        symbol,
        is_definition: roles & 0x1 != 0,
    }))
}

/// `SingleLineRange`: `line` (1), `start_character` (2),
/// `end_character` (3).
fn parse_single_line(body: &[u8]) -> Result<Range, Malformed> {
    const REASON: &str = "malformed range";
    let mut cursor = Cursor::new(body);
    let (mut line, mut start, mut end) = (None, None, None);
    while !cursor.done() {
        let (field, wire, payload) = next_field(&mut cursor)?;
        match (field, wire) {
            (1, VARINT) => line = Some(bounded(cursor.varint(REASON)?)?),
            (2, VARINT) => start = Some(bounded(cursor.varint(REASON)?)?),
            (3, VARINT) => end = Some(bounded(cursor.varint(REASON)?)?),
            // `next_field` already consumed a LENGTH field's payload;
            // only the unconsumed shapes skip.
            _ => {
                if payload.is_none() {
                    cursor.skip_field(wire, REASON)?;
                }
            }
        }
    }
    let (Some(line), Some(start), Some(end)) = (line, start, end) else {
        return Err(Malformed { reason: REASON });
    };
    Ok(Range {
        start_line: line,
        start_character: start,
        end_line: line,
        end_character: end,
    })
}

/// `MultiLineRange`: `start_line` (1), `start_character` (2),
/// `end_line` (3), `end_character` (4).
fn parse_multi_line(body: &[u8]) -> Result<Range, Malformed> {
    const REASON: &str = "malformed range";
    let mut cursor = Cursor::new(body);
    let mut fields = [None; 4];
    while !cursor.done() {
        let (field, wire, payload) = next_field(&mut cursor)?;
        match (field, wire) {
            (1..=4, VARINT) => {
                let value = bounded(cursor.varint(REASON)?)?;
                fields[(field - 1) as usize] = Some(value);
            }
            // `next_field` already consumed a LENGTH field's payload;
            // only the unconsumed shapes skip.
            _ => {
                if payload.is_none() {
                    cursor.skip_field(wire, REASON)?;
                }
            }
        }
    }
    let [
        Some(start_line),
        Some(start_character),
        Some(end_line),
        Some(end_character),
    ] = fields
    else {
        return Err(Malformed { reason: REASON });
    };
    Ok(Range {
        start_line,
        start_character,
        end_line,
        end_character,
    })
}

/// One document body: `relative_path` (1) and `occurrences` (2); the
/// rest skipped by wire type. A document without a path is a failure.
pub(crate) fn parse_document(body: &[u8]) -> Result<Document, Malformed> {
    const REASON: &str = "malformed document";
    let mut cursor = Cursor::new(body);
    let mut path: Option<String> = None;
    let mut occurrences = Vec::new();
    while !cursor.done() {
        let (field, wire, payload) = next_field(&mut cursor)?;
        match (field, wire) {
            (1, LENGTH) => {
                let text = std::str::from_utf8(payload.expect("length"))
                    .map_err(|_| Malformed { reason: REASON })?;
                path = Some(text.to_owned());
            }
            (2, LENGTH) => {
                let body = payload.expect("length");
                if let Some(occurrence) = parse_occurrence(body)? {
                    occurrences.push(occurrence);
                }
            }
            // `next_field` already consumed a LENGTH field's payload;
            // only the unconsumed shapes skip.
            _ => {
                if payload.is_none() {
                    cursor.skip_field(wire, REASON)?;
                }
            }
        }
    }
    let Some(relative_path) = path else {
        return Err(Malformed { reason: REASON });
    };
    Ok(Document {
        relative_path,
        occurrences,
    })
}

/// `Index`: every `documents` (2) payload; `metadata` (1) and
/// `external_symbols` (3) skipped by wire type.
pub fn parse_index(bytes: &[u8]) -> Result<ScipIndex, Malformed> {
    const REASON: &str = "malformed index";
    let mut cursor = Cursor::new(bytes);
    let mut documents = Vec::new();
    while !cursor.done() {
        let (field, wire, payload) = next_field(&mut cursor)?;
        match (field, wire) {
            (2, LENGTH) => {
                let body = payload.expect("length");
                documents.push(parse_document(body)?);
            }
            // `next_field` already consumed a LENGTH field's payload;
            // only the unconsumed shapes skip.
            _ => {
                if payload.is_none() {
                    cursor.skip_field(wire, REASON)?;
                }
            }
        }
    }
    Ok(ScipIndex { documents })
}
