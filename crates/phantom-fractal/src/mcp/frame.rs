//! Newline framing with a size cap.

use std::io::{self, BufRead};

/// The longest message line read, newline excluded: 1 MiB.
pub(crate) const MAX_LINE: usize = 1 << 20;

/// One line of input.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// The input ended before any byte of a new line.
    Eof,
    /// A line without its newline. The last line may lack one.
    Text(Vec<u8>),
    /// A line over the cap. Its bytes were read through its newline and
    /// dropped.
    TooLong,
}

/// Reads one line of at most `cap` bytes. A longer line is consumed to its
/// end and never held whole in memory.
pub(crate) fn read_line(input: &mut impl BufRead, cap: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    let mut started = false;
    loop {
        let buf = match input.fill_buf() {
            Ok(buf) => buf,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        if buf.is_empty() {
            return Ok(match (started, too_long) {
                (false, _) => Line::Eof,
                (true, true) => Line::TooLong,
                (true, false) => Line::Text(line),
            });
        }
        started = true;
        let newline = buf.iter().position(|byte| *byte == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if !too_long {
            if line.len() + chunk.len() > cap {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = newline.map_or(buf.len(), |at| at + 1);
        input.consume(used);
        if newline.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Text(line)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;

    fn lines(input: &[u8], cap: usize, capacity: usize) -> Vec<Line> {
        let mut reader = BufReader::with_capacity(capacity, Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        loop {
            let line = read_line(&mut reader, cap).expect("read");
            if line == Line::Eof {
                return out;
            }
            out.push(line);
        }
    }

    fn text(bytes: &[u8]) -> Line {
        Line::Text(bytes.to_vec())
    }

    #[test]
    fn splits_on_newlines_and_keeps_a_last_line_without_one() {
        assert_eq!(
            lines(b"a\nbc\n\nd", 8, 3),
            [text(b"a"), text(b"bc"), text(b""), text(b"d")]
        );
        assert_eq!(lines(b"", 8, 3), []);
    }

    #[test]
    fn a_line_at_the_cap_is_kept_and_one_over_is_dropped_whole() {
        // Small buffers make the long line span many reads.
        assert_eq!(
            lines(b"abcd\nabcde\nz\n", 4, 2),
            [text(b"abcd"), Line::TooLong, text(b"z")]
        );
        assert_eq!(lines(b"abcdefgh", 4, 3), [Line::TooLong]);
    }
}
