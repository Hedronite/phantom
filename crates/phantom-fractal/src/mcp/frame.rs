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
    let mut reading = Reading {
        cap,
        line: Vec::new(),
        too_long: false,
        started: false,
    };
    loop {
        let filled = input.fill_buf();
        if interrupted(&filled) {
            continue;
        }
        let (used, line) = reading.take(filled?);
        input.consume(used);
        if let Some(line) = line {
            return Ok(line);
        }
    }
}

/// True when a read was interrupted and is to be tried again.
fn interrupted(filled: &io::Result<&[u8]>) -> bool {
    filled
        .as_ref()
        .is_err_and(|err| err.kind() == io::ErrorKind::Interrupted)
}

/// One line read so far.
struct Reading {
    cap: usize,
    /// The bytes kept: none once the line is over the cap.
    line: Vec<u8>,
    too_long: bool,
    /// Set once any byte of the line was read.
    started: bool,
}

impl Reading {
    /// Takes in one buffer of input, empty at its end: how many of its
    /// bytes the line used, and the line once it is whole.
    fn take(&mut self, buf: &[u8]) -> (usize, Option<Line>) {
        if buf.is_empty() {
            return (0, Some(self.at_end()));
        }
        self.started = true;
        let newline = buf.iter().position(|byte| *byte == b'\n');
        self.keep(&buf[..newline.unwrap_or(buf.len())]);
        match newline {
            Some(at) => (at + 1, Some(self.line())),
            None => (buf.len(), None),
        }
    }

    /// Keeps `chunk` while the line stays within the cap, and drops the
    /// line once it goes over.
    fn keep(&mut self, chunk: &[u8]) {
        if self.too_long {
            return;
        }
        if self.line.len() + chunk.len() > self.cap {
            self.too_long = true;
            self.line = Vec::new();
        } else {
            self.line.extend_from_slice(chunk);
        }
    }

    /// The line when the input ends: none before its first byte.
    fn at_end(&mut self) -> Line {
        if self.started { self.line() } else { Line::Eof }
    }

    /// The line read, or [`Line::TooLong`].
    fn line(&mut self) -> Line {
        if self.too_long {
            Line::TooLong
        } else {
            Line::Text(std::mem::take(&mut self.line))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor, Read};

    use super::*;

    /// `work`'s value, or a panic when it has not returned within 10 s:
    /// every input ends in a line, the end of input, or an error.
    fn returns<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(work());
        });
        got.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|err| panic!("the read did not return: {err:?}"))
    }

    fn lines(input: &[u8], cap: usize, capacity: usize) -> Vec<Line> {
        let input = input.to_vec();
        returns(move || {
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(input));
            let mut out = Vec::new();
            loop {
                let line = read_line(&mut reader, cap).expect("read");
                if line == Line::Eof {
                    return out;
                }
                out.push(line);
            }
        })
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

    /// A reader that fails once with `kind`, then reads `rest`.
    struct FailOnce {
        kind: Option<io::ErrorKind>,
        rest: Cursor<Vec<u8>>,
    }

    impl Read for FailOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.kind.take() {
                Some(kind) => Err(io::Error::from(kind)),
                None => self.rest.read(buf),
            }
        }
    }

    fn fail_once(kind: io::ErrorKind) -> BufReader<FailOnce> {
        BufReader::new(FailOnce {
            kind: Some(kind),
            rest: Cursor::new(b"ab\n".to_vec()),
        })
    }

    #[test]
    fn an_interrupted_read_is_tried_again_and_any_other_error_is_returned() {
        let read =
            |kind| returns(move || read_line(&mut fail_once(kind), 8).map_err(|err| err.kind()));
        assert_eq!(read(io::ErrorKind::Interrupted), Ok(text(b"ab")));
        assert_eq!(
            read(io::ErrorKind::BrokenPipe),
            Err(io::ErrorKind::BrokenPipe)
        );
    }
}
