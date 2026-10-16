// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Exit codes and the single error type every command returns.
//!
//! Scripts and other tools key off the exit code, so each code has one
//! meaning and never changes. Humans read the message on standard error.

use std::fmt;

/// Process exit codes. The numeric values are part of the public contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// The command did what was asked.
    Success = 0,
    /// A safety or precondition check refused the request. Nothing changed.
    Refused = 1,
    /// The command line was invalid. Reserved for the argument parser.
    Usage = 2,
    /// A session started and then failed; inspect its session directory.
    SessionFailed = 3,
    /// There was no live session or record to act on.
    NoSession = 4,
    /// An input/output or internal error outside the launcher's control.
    Internal = 5,
}

impl Exit {
    /// Human description used in `--help` and in the crate README.
    pub const TABLE: &'static str = "\
Exit codes:
  0  success
  1  refused by a safety or precondition check; nothing was changed
  2  invalid command line
  3  a session started and then failed; inspect its session directory
  4  no live session or record to act on
  5  input/output or internal error";
}

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        std::process::ExitCode::from(exit as u8)
    }
}

/// Error carrying the exit code the process must end with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub exit: Exit,
    pub message: String,
}

impl Failure {
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::Refused,
            message: message.into(),
        }
    }
    pub fn session(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::SessionFailed,
            message: message.into(),
        }
    }
    pub fn no_session(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::NoSession,
            message: message.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::Internal,
            message: message.into(),
        }
    }
    /// Prefix the message with what was being attempted.
    pub fn context(mut self, what: &str) -> Self {
        self.message = format!("{what}: {}", self.message);
        self
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Failure::internal(error.to_string())
    }
}

impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Self {
        Failure::internal(format!("invalid JSON: {error}"))
    }
}

/// Shorthand used throughout the crate.
pub type Result<T> = std::result::Result<T, Failure>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(Exit::Success as u8, 0);
        assert_eq!(Exit::Refused as u8, 1);
        assert_eq!(Exit::Usage as u8, 2);
        assert_eq!(Exit::SessionFailed as u8, 3);
        assert_eq!(Exit::NoSession as u8, 4);
        assert_eq!(Exit::Internal as u8, 5);
    }

    #[test]
    fn io_errors_map_to_internal_and_keep_their_text() {
        let failure: Failure =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied").into();
        assert_eq!(failure.exit, Exit::Internal);
        assert_eq!(failure.to_string(), "denied");
        assert_eq!(failure.context("open lock").message, "open lock: denied");
    }
}
