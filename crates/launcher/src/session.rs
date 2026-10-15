// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The per-data-directory lock, the current-session pointer, and the stop
//! protocol between the front-end commands and the supervisor.
//!
//! One `launcher.lock` file per data directory is held with an advisory
//! lock for the lifetime of a supervisor. `status` and `stop` never guess
//! at process identifiers: a status whose supervisor no longer holds the
//! lock is reported as `interrupted`, and a stop request is a file the live
//! supervisor consumes, so stopping never kills an arbitrary process.

use crate::{
    exit::{Failure, Result},
    paths,
    records::{self, SessionPointer, State, Status},
};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

/// Name of the lock file in the data directory.
pub const LOCK_FILE: &str = "launcher.lock";
/// Name of the pointer to the current session.
pub const POINTER_FILE: &str = "current-session.json";
/// Stop request consumed by the supervisor: close the game, then the server.
pub const STOP_FILE: &str = "stop";
/// Stop request consumed by the server (`--stop-file`).
pub const STOP_SERVER_FILE: &str = "stop-server";

/// Held advisory lock on the data directory.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Acquire the lock, creating the file if needed. Refuses when another
    /// process holds it.
    pub fn acquire(data: &Path) -> Result<Lock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data.join(LOCK_FILE))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(Failure::refused(
                "Another Afterhours session holds this data directory. Stop it first.",
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

/// Whether some process currently holds the lock. `None` when the file does
/// not exist.
pub fn lock_is_held(data: &Path) -> Result<Option<bool>> {
    let path = data.join(LOCK_FILE);
    let file = match File::options().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match file.try_lock() {
        Ok(()) => {
            file.unlock()?;
            Ok(Some(false))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(Some(true)),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Write the current-session pointer atomically.
pub fn point_current_session(data: &Path, id: &str) -> Result<()> {
    records::replace_json(
        &data.join(POINTER_FILE),
        &SessionPointer {
            version: records::VERSION,
            id: id.to_owned(),
        },
    )
}

/// The current session's status, with `interrupted` derived when its
/// supervisor is gone. `None` when no session was ever started here.
pub fn current_status(data: &Path) -> Result<Option<Status>> {
    let data = paths::checked_data_path(data, None)?;
    let pointer_path = data.join(POINTER_FILE);
    if !pointer_path.exists() {
        return Ok(None);
    }
    let pointer: SessionPointer = records::read_json(&pointer_path)?;
    if pointer.version != records::VERSION || !records::is_id(&pointer.id) {
        return Err(Failure::refused("Invalid session identifier."));
    }
    let run = paths::session_dir(&data, &pointer.id);
    paths::checked_data_path(&run, None)?;
    let mut status: Status = records::read_json(&run.join("status.json"))?;
    if status.id != pointer.id {
        return Err(Failure::refused("Session status identity mismatch."));
    }
    if status.state.is_live() {
        match lock_is_held(&data)? {
            None => {
                status.state = State::Interrupted;
                status.message =
                    "Supervisor lock is missing. Inspect the data directory before restarting."
                        .into();
            }
            Some(false) => {
                status.state = State::Interrupted;
                status.message = "Supervisor is no longer running. Its owned processes were closed or are orphaned; saved account data is preserved. Start again to recover.".into();
            }
            Some(true) => {}
        }
    }
    Ok(Some(status))
}

/// Outcome of a stop request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// No session has been started in this data directory.
    NoSession,
    /// The session is already over; carries its final message.
    AlreadyOver(Status),
    /// A stop file was written for the live supervisor to consume.
    Requested(PathBuf),
}

/// Ask the live supervisor to close the game, then the server.
pub fn request_stop(data: &Path) -> Result<StopOutcome> {
    let data = paths::checked_data_path(data, None)?;
    let Some(status) = current_status(&data)? else {
        return Ok(StopOutcome::NoSession);
    };
    if !status.state.is_live() {
        return Ok(StopOutcome::AlreadyOver(status));
    }
    let run = paths::session_dir(&data, &status.id);
    paths::checked_data_path(&run, None)?;
    let stop = run.join(STOP_FILE);
    match File::options().write(true).create_new(true).open(&stop) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(StopOutcome::Requested(stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "afterhours-session-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // The temporary directory may sit under a symlink (/var on macOS);
        // the path rules refuse links, so tests work from the real path.
        dir.canonicalize().unwrap()
    }

    fn write_status(data: &Path, id: &str, state: State) {
        let run = paths::session_dir(data, id);
        std::fs::create_dir_all(&run).unwrap();
        records::replace_json(
            &run.join("status.json"),
            &Status {
                version: 1,
                id: id.to_owned(),
                utc: "2026-10-15T00:00:00Z".into(),
                state,
                message: "m".into(),
                worker_pid: 1,
                game_pid: None,
                server_pid: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn the_lock_is_exclusive_and_observable() {
        let data = scratch();
        assert_eq!(lock_is_held(&data).unwrap(), None);
        let held = Lock::acquire(&data).unwrap();
        assert_eq!(lock_is_held(&data).unwrap(), Some(true));
        assert!(Lock::acquire(&data).is_err());
        drop(held);
        assert_eq!(lock_is_held(&data).unwrap(), Some(false));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn status_derives_interrupted_when_no_supervisor_holds_the_lock() {
        let data = scratch();
        assert_eq!(current_status(&data).unwrap(), None);
        let id = "f".repeat(32);
        point_current_session(&data, &id).unwrap();
        write_status(&data, &id, State::Ready);
        let status = current_status(&data).unwrap().unwrap();
        assert_eq!(status.state, State::Interrupted);
        assert!(status.message.contains("lock is missing"));
        let held = Lock::acquire(&data).unwrap();
        assert_eq!(current_status(&data).unwrap().unwrap().state, State::Ready);
        drop(held);
        assert_eq!(
            current_status(&data).unwrap().unwrap().state,
            State::Interrupted
        );
        write_status(&data, &id, State::Finished);
        assert_eq!(
            current_status(&data).unwrap().unwrap().state,
            State::Finished
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn stop_writes_a_file_only_for_a_live_session() {
        let data = scratch();
        assert_eq!(request_stop(&data).unwrap(), StopOutcome::NoSession);
        let id = "e".repeat(32);
        point_current_session(&data, &id).unwrap();
        write_status(&data, &id, State::Failed);
        assert!(matches!(
            request_stop(&data).unwrap(),
            StopOutcome::AlreadyOver(_)
        ));
        write_status(&data, &id, State::Ready);
        let _held = Lock::acquire(&data).unwrap();
        let first = request_stop(&data).unwrap();
        let StopOutcome::Requested(path) = &first else {
            panic!("{first:?}")
        };
        assert!(path.exists());
        assert_eq!(
            request_stop(&data).unwrap(),
            first,
            "repeat requests are idempotent"
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn mismatched_pointer_and_status_are_refused() {
        let data = scratch();
        point_current_session(&data, &"a".repeat(32)).unwrap();
        write_status(&data, &"a".repeat(32), State::Ready);
        let run = paths::session_dir(&data, &"a".repeat(32));
        let mut status: Status = records::read_json(&run.join("status.json")).unwrap();
        status.id = "b".repeat(32);
        records::replace_json(&run.join("status.json"), &status).unwrap();
        assert!(current_status(&data).is_err());
        std::fs::remove_dir_all(data).unwrap();
    }
}
