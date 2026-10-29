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
//! only under `--allow-index`.

mod frame;
mod rpc;
mod tools;

use std::io::{self, Write};
use std::process::ExitCode;

/// Serves stdin until EOF. `allow_index` offers the `index_repo` tool.
pub(crate) fn serve(allow_index: bool) -> ExitCode {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut server = rpc::Server::new(allow_index);
    loop {
        let line = match frame::read_line(&mut input, frame::MAX_LINE) {
            Ok(frame::Line::Eof) => return ExitCode::SUCCESS,
            Ok(line) => line,
            Err(err) => return fatal(&format!("cannot read stdin: {err}")),
        };
        let Some(reply) = server.handle(line) else {
            continue;
        };
        if let Err(err) = writeln!(output, "{reply}").and_then(|()| output.flush()) {
            return fatal(&format!("cannot write stdout: {err}"));
        }
    }
}

/// The one stderr line of a server that cannot go on, and exit 1.
fn fatal(message: &str) -> ExitCode {
    // If stderr is gone too, there is nowhere left to report it.
    let _ = writeln!(io::stderr().lock(), "phantom-fractal mcp: {message}");
    ExitCode::from(1)
}
