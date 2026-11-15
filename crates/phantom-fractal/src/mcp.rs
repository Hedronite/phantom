//! `mcp`: a Model Context Protocol server on stdin and stdout (SPEC-w6 G2).
//!
//! Synchronous, newline-delimited JSON-RPC 2.0: one message per line in,
//! one compact message per line out, one request at a time. Nothing but
//! messages reaches stdout, and stderr carries one line only when the
//! server cannot go on. EOF on stdin ends the server with exit 0.
//!
//! The server speaks the `initialize`-based revisions in
//! [`rpc::PROTOCOL_VERSIONS`]. Its tools call the same [`crate::ops`]
//! functions as the command line, and each result carries the envelope the
//! command line prints. It starts no process, opens no socket, and writes
//! nothing outside the data directory, which only `index_repo` writes, and
//! only under `--allow-index`. A tool reaches only the trees in
//! [`roots::Roots`].

mod frame;
mod roots;
mod rpc;
mod tools;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Serves stdin until EOF. `allow_index` offers the `index_repo` tool, and
/// `roots` are the trees the tools may use: the working directory when
/// empty. A root that is not a directory stops the server before it reads
/// a line.
pub(crate) fn serve(allow_index: bool, roots: &[PathBuf]) -> ExitCode {
    match roots::Roots::new(roots) {
        Ok(roots) => answer(
            &mut rpc::Server::new(allow_index, roots),
            &mut io::stdin().lock(),
            &mut io::stdout().lock(),
        ),
        Err(message) => fatal(&message),
    }
}

/// Answers each line of `input` on `output` until `input` ends, or until
/// a read or a write fails.
fn answer(server: &mut rpc::Server, input: &mut impl BufRead, output: &mut impl Write) -> ExitCode {
    loop {
        let line = match frame::read_line(input, frame::MAX_LINE) {
            Ok(frame::Line::Eof) => return ExitCode::SUCCESS,
            Ok(line) => line,
            Err(err) => return fatal(&format!("cannot read stdin: {err}")),
        };
        if let Err(err) = reply(server, line, output) {
            return fatal(&format!("cannot write stdout: {err}"));
        }
    }
}

/// Writes the reply to `line`, if it has one, and flushes it.
fn reply(server: &mut rpc::Server, line: frame::Line, output: &mut impl Write) -> io::Result<()> {
    let Some(reply) = server.handle(line) else {
        return Ok(());
    };
    writeln!(output, "{reply}")?;
    output.flush()
}

/// The one stderr line of a server that cannot go on, and exit 1.
fn fatal(message: &str) -> ExitCode {
    // If stderr is gone too, there is nowhere left to report it.
    let _ = writeln!(io::stderr().lock(), "phantom-fractal mcp: {message}");
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read, Write};

    use super::*;

    fn server() -> rpc::Server {
        rpc::Server::new(
            false,
            roots::Roots::new(&[]).expect("the working directory"),
        )
    }

    /// Input that cannot be read, and output that cannot be written.
    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// `work`'s value, or a panic when it has not returned within 10 s:
    /// every input ends in a line, the end of input, or an error.
    fn returns<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(work());
        });
        got.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|err| panic!("the server did not return: {err:?}"))
    }

    const PING: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";

    #[test]
    fn the_end_of_input_ends_the_server_with_exit_0() {
        let (code, output) = returns(|| {
            let mut output = Vec::new();
            let code = answer(&mut server(), &mut Cursor::new(PING), &mut output);
            (code, output)
        });
        assert_eq!(code, ExitCode::SUCCESS);
        let reply = String::from_utf8(output).expect("utf-8");
        assert_eq!(reply.lines().count(), 1, "{reply}");
        assert!(
            reply.ends_with('\n') && reply.contains("\"result\":{}"),
            "{reply}"
        );
    }

    #[test]
    fn a_failed_read_or_write_ends_the_server_with_exit_1() {
        let code = returns(|| {
            answer(
                &mut server(),
                &mut io::BufReader::new(Broken),
                &mut Vec::new(),
            )
        });
        assert_eq!(code, ExitCode::from(1));
        let code = returns(|| answer(&mut server(), &mut Cursor::new(PING), &mut Broken));
        assert_eq!(code, ExitCode::from(1));
    }
}
