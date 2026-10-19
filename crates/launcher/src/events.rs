// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The session event stream: `setup`, `serve` and `start` with `--events`.
//!
//! Standard output becomes JSON lines, one [`Event`] per line, so a desktop
//! front end can follow a session without polling `status`:
//!
//! 1. `session`: the session directory exists and the supervisor starts.
//! 2. `status`: every state the supervisor records, exactly as written to
//!    `status.json`, in order.
//! 3. `diagnostic`: each line a child wrote to standard error, tagged with
//!    the child that wrote it.
//! 4. `final`: the command's outcome and exit code. Always the last line,
//!    also when the command was refused before a session existed.
//!
//! The front-end process writes `session` and `final`; the supervisor it
//! starts inherits standard output and writes `status` and `diagnostic`.
//! Their writes never interleave: the front end waits for the supervisor.
//! Human console text is silenced in this mode. A write that fails, for
//! example because the reader closed the pipe, disables further events and
//! never stops the session; `status` and `sessions` still observe it.

use crate::{
    clock,
    records::{Action, Status},
    terminal::Lines,
};
use schemars::JsonSchema;
use serde::Serialize;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::{io::Write, path::PathBuf};

/// Version of [`Event`].
pub const FORMAT_VERSION: u32 = 1;

/// One line of the event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Event {
    pub version: u32,
    /// When the event was written, UTC.
    pub utc: String,
    #[serde(flatten)]
    pub body: Body,
}

/// The kind of event, in the `event` property, and its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Body {
    /// The session directory was created and the supervisor is starting.
    Session {
        /// Session ID: 32 lowercase hexadecimal characters.
        id: String,
        action: Action,
        data_directory: PathBuf,
        /// `artifacts/launcher/<id>/` with this session's logs and records.
        session_directory: PathBuf,
    },
    /// A state change, exactly as written to `status.json`.
    Status { status: Status },
    /// One line a child process wrote to standard error, without its line
    /// ending. Lines longer than 16 KiB arrive in several events. When the
    /// supervisor falls behind, a line saying output was skipped replaces
    /// the dropped text; the session's log files keep everything up to
    /// their 4 MiB bound.
    Diagnostic {
        source: DiagnosticSource,
        text: String,
    },
    /// The last line: how the command ended.
    Final {
        /// The process exit code; see the exit code table in `--help`.
        exit_code: u8,
        /// Why the command failed; `null` on success.
        message: Option<String>,
        /// The session's final status; `null` when the command was refused
        /// before the supervisor recorded one.
        status: Option<Status>,
    },
}

/// The child that wrote a diagnostic line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSource {
    /// Content and policy preparation (`prepare.stderr.log`).
    Prepare,
    /// First-run account creation (`create-account.stderr.log`).
    CreateAccount,
    /// The running server (`server.stderr.log`).
    Server,
    /// The game (`game.stderr.log`).
    Game,
}

impl DiagnosticSource {
    const ALL: [DiagnosticSource; 4] = [
        DiagnosticSource::Prepare,
        DiagnosticSource::CreateAccount,
        DiagnosticSource::Server,
        DiagnosticSource::Game,
    ];
}

/// Writes events as JSON lines, or nothing when disabled.
pub struct Emitter {
    out: Option<Box<dyn Write + Send>>,
    pending: [Lines; 4],
}

impl Emitter {
    /// Events off: every call is a no-op.
    pub fn disabled() -> Self {
        Self::with(None)
    }

    /// Events on standard output.
    pub fn stdout() -> Self {
        Self::with(Some(Box::new(std::io::stdout())))
    }

    /// Events to any writer, for tests.
    pub fn to(out: impl Write + Send + 'static) -> Self {
        Self::with(Some(Box::new(out)))
    }

    fn with(out: Option<Box<dyn Write + Send>>) -> Self {
        Self {
            out,
            pending: Default::default(),
        }
    }

    /// Whether events are still being written.
    pub fn enabled(&self) -> bool {
        self.out.is_some()
    }

    /// Write one event. A failed write disables the emitter.
    pub fn emit(&mut self, body: Body) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let event = Event {
            version: FORMAT_VERSION,
            utc: clock::now_iso(),
            body,
        };
        let written = serde_json::to_vec(&event)
            .map_err(std::io::Error::other)
            .and_then(|mut line| {
                line.push(b'\n');
                out.write_all(&line)?;
                out.flush()
            });
        if written.is_err() {
            self.out = None;
        }
    }

    /// Emit a status change.
    pub fn status(&mut self, status: &Status) {
        self.emit(Body::Status {
            status: status.clone(),
        });
    }

    /// Emit every complete line of a child's diagnostic text; keep an
    /// unterminated last line until more text or [`Emitter::flush`].
    pub fn diagnostic(&mut self, source: DiagnosticSource, text: &str) {
        if !self.enabled() || text.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        self.pending[source as usize].collect(text, |line| lines.push(line.to_owned()));
        for text in lines {
            self.emit(Body::Diagnostic { source, text });
        }
    }

    /// Emit an unterminated last line once its child has finished.
    pub fn flush(&mut self, source: DiagnosticSource) {
        let mut last = None;
        self.pending[source as usize].finish(|line| last = Some(line.to_owned()));
        if let Some(text) = last {
            self.emit(Body::Diagnostic { source, text });
        }
    }

    /// [`Emitter::flush`] for every source.
    pub fn flush_all(&mut self) {
        for source in DiagnosticSource::ALL {
            self.flush(source);
        }
    }
}

/// A writer that appends to shared memory, for tests that read the stream.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Capture(pub(crate) Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl Capture {
    /// The captured lines, parsed.
    pub(crate) fn lines(&self) -> Vec<serde_json::Value> {
        let bytes = self.0.lock().unwrap_or_else(|p| p.into_inner()).clone();
        bytes
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("each line is one JSON object"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{self, State};

    fn status(state: State) -> Status {
        Status {
            version: records::VERSION,
            id: "c".repeat(32),
            utc: "2026-10-19T12:00:00Z".into(),
            state,
            message: "m".into(),
            worker_pid: 1,
            game_pid: None,
            server_pid: None,
        }
    }

    #[test]
    fn every_event_kind_is_one_line_that_matches_the_schema() {
        let capture = Capture::default();
        let mut emitter = Emitter::to(capture.clone());
        emitter.emit(Body::Session {
            id: "c".repeat(32),
            action: Action::Serve,
            data_directory: PathBuf::from("data"),
            session_directory: PathBuf::from("data/artifacts/launcher/c"),
        });
        emitter.status(&status(State::Preparing));
        emitter.diagnostic(
            DiagnosticSource::Server,
            "line with \"quotes\" and \u{e5}\n",
        );
        emitter.emit(Body::Final {
            exit_code: 0,
            message: None,
            status: Some(status(State::Finished)),
        });
        emitter.emit(Body::Final {
            exit_code: 1,
            message: Some("Refused.".into()),
            status: None,
        });
        let lines = capture.lines();
        assert_eq!(lines.len(), 5);
        let kinds: Vec<_> = lines.iter().map(|l| l["event"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["session", "status", "diagnostic", "final", "final"]);
        for line in &lines {
            assert_eq!(line["version"], FORMAT_VERSION);
            crate::contract::testing::validate("event", line);
        }
        assert_eq!(lines[1]["status"]["state"], "preparing");
        assert_eq!(lines[2]["source"], "server");
        assert_eq!(lines[2]["text"], "line with \"quotes\" and \u{e5}");
        assert_eq!(lines[4]["status"], serde_json::Value::Null);
    }

    #[test]
    fn diagnostics_are_split_into_lines_per_source_and_flushed() {
        let capture = Capture::default();
        let mut emitter = Emitter::to(capture.clone());
        emitter.diagnostic(DiagnosticSource::Server, "first\r\nsec");
        emitter.diagnostic(DiagnosticSource::Game, "game ");
        emitter.diagnostic(DiagnosticSource::Server, "ond\nthird");
        emitter.diagnostic(DiagnosticSource::Game, "line\n");
        emitter.flush_all();
        emitter.flush_all();
        let lines: Vec<(String, String)> = capture
            .lines()
            .iter()
            .map(|l| {
                (
                    l["source"].as_str().unwrap().to_owned(),
                    l["text"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                ("server".into(), "first".into()),
                ("server".into(), "second".into()),
                ("game".into(), "game line".into()),
                ("server".into(), "third".into()),
            ]
        );
    }

    #[test]
    fn a_failed_write_disables_the_stream_without_failing() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut emitter = Emitter::to(Broken);
        assert!(emitter.enabled());
        emitter.status(&status(State::Ready));
        assert!(!emitter.enabled());
        emitter.status(&status(State::Finished));
        emitter.diagnostic(DiagnosticSource::Server, "ignored\n");
        let mut disabled = Emitter::disabled();
        disabled.status(&status(State::Ready));
        assert!(!disabled.enabled());
    }
}
