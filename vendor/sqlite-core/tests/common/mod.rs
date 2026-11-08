//! Shared helpers for the real-`sqlite3` gold-oracle tests.
//!
//! These fixtures drive a **long-lived `sqlite3` reader process** (held open so
//! the `-wal` sidecar is retained) while separate short-lived writer processes
//! mutate the same database. Statements sent to the reader execute
//! asynchronously in that other process, so the fixture must know when they
//! have actually run before the writers touch the schema they create.
//!
//! The obvious way to "know" is a `sleep`, and it is wrong: it encodes a guess
//! about how fast the host is. Losing that race produces
//! `Error: in prepare, no such table: t` from the writer — a failure with
//! nothing to do with the code under test, appearing only on slower or busier
//! machines. [`HeldReader::run`] replaces the guess with an acknowledgement.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// Upper bound on how long to wait for one statement to be acknowledged.
///
/// Generous on purpose: this bounds a hang, it does not pace the test. The
/// happy path returns as soon as the sentinel arrives, typically in
/// microseconds, so a large value costs nothing when things work.
const ACK_TIMEOUT: Duration = Duration::from_secs(30);

/// A `sqlite3` reader process whose statements are acknowledged before the
/// caller proceeds.
pub struct HeldReader {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seq: u32,
}

impl HeldReader {
    /// Spawn `bin` against `db`, with stdout piped so statements can be
    /// acknowledged.
    pub fn spawn(bin: &str, db: &Path) -> Self {
        let mut child = Command::new(bin)
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Drain stdout on a thread: the pipe must never fill, or the reader
        // blocks on write and the whole fixture deadlocks.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            lines,
            seq: 0,
        }
    }

    /// Send `sql` to the reader and block until it has actually executed.
    ///
    /// A unique sentinel is appended and awaited on stdout. `sqlite3` executes
    /// a script strictly in order on one connection, so the sentinel cannot be
    /// printed until every preceding statement has run — the acknowledgement is
    /// a consequence of ordering, not of timing.
    ///
    /// Panics with the offending SQL on timeout rather than hanging.
    pub fn run(&mut self, sql: &str) {
        self.seq += 1;
        let token = format!("__ack_{}__", self.seq);
        let stdin = self.stdin.as_mut().expect("reader stdin already released");
        writeln!(stdin, "{sql}\nSELECT '{token}';").unwrap();
        stdin.flush().unwrap();

        loop {
            match self.lines.recv_timeout(ACK_TIMEOUT) {
                Ok(line) if line.trim() == token => return,
                // Statement output (row counts, pragma results) precedes the
                // sentinel; skip it.
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    panic!("sqlite3 did not acknowledge within {ACK_TIMEOUT:?}: {sql}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("sqlite3 reader exited before acknowledging: {sql}")
                }
            }
        }
    }

    /// Close the held read transaction and reap the process.
    pub fn finish(mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "COMMIT;\n.quit");
        }
        let _ = self.child.wait();
    }
}

/// Run one short-lived writer connection against `db`.
pub fn writer_sql(bin: &str, db: &Path, sql: &str) {
    let out = Command::new(bin).arg(db).arg(sql).output().unwrap();
    assert!(
        out.status.success(),
        "sqlite3 writer failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
