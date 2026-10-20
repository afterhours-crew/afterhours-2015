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
    records::{self, Action, Request, SessionPointer, State, Status},
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
/// Play request consumed by a serving supervisor: start the game.
pub const PLAY_FILE: &str = "play";

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
        derive_interrupted(&mut status, lock_is_held(&data)?);
    }
    Ok(Some(status))
}

/// Report a live status as `interrupted` unless a supervisor holds the
/// data-directory lock (`lock` as [`lock_is_held`] returns it). Only the
/// current session can have a supervisor; pass `Some(false)` for any other.
pub fn derive_interrupted(status: &mut Status, lock: Option<bool>) {
    if !status.state.is_live() {
        return;
    }
    if lock != Some(true) {
        // Without its supervisor the session's job has closed its game.
        status.game_running = false;
    }
    match lock {
        None => {
            status.state = State::Interrupted;
            status.message =
                "Supervisor lock is missing. Inspect the data directory before restarting.".into();
        }
        Some(false) => {
            status.state = State::Interrupted;
            status.message = "Supervisor is no longer running. Its owned processes were closed or are orphaned; saved account data is preserved. Start again to recover.".into();
        }
        Some(true) => {}
    }
}

/// Outcome of a stop request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// No session has been started in this data directory.
    NoSession,
    /// The session is already over; carries its final message.
    AlreadyOver(Status),
    /// A stop file was written for the live supervisor to consume.
    Requested { stop_file: PathBuf, status: Status },
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
    Ok(StopOutcome::Requested {
        stop_file: stop,
        status,
    })
}

/// A play request written for the live serving supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayRequested {
    pub play_file: PathBuf,
    /// The session's status when the request was written.
    pub status: Status,
}

/// Ask the live `serve` supervisor to start the game in its session. Only a
/// live serve session started with game support and without a game running
/// or requested is asked; anything else is refused and nothing is written.
/// `ready` runs last, before the request is written, for checks outside the
/// data directory such as conflicting processes.
pub fn request_play(data: &Path, ready: impl FnOnce() -> Result<()>) -> Result<PlayRequested> {
    let data = paths::checked_data_path(data, None)?;
    let Some(status) = current_status(&data)? else {
        return Err(Failure::no_session(
            "No Afterhours session is running. Start one with `nfs-afterhours serve` first.",
        ));
    };
    if !status.state.is_live() {
        return Err(Failure::no_session(
            "The current session has ended. Start one with `nfs-afterhours serve` first.",
        ));
    }
    let run = paths::session_dir(&data, &status.id);
    paths::checked_data_path(&run, None)?;
    let request: Request = records::read_json(&run.join("request.json"))?;
    if request.id != status.id {
        return Err(Failure::refused("Session request identity mismatch."));
    }
    match request.action {
        Action::Serve => {}
        Action::Start => {
            return Err(Failure::refused(
                "This session already runs the game; only a `serve` session starts it on request.",
            ));
        }
        Action::Setup => {
            return Err(Failure::refused(
                "A setup session does not host the game; use `nfs-afterhours serve`.",
            ));
        }
    }
    if !crate::platform::supports_game_launch() {
        return Err(Failure::refused(
            "Starting the game is supported on Windows only.",
        ));
    }
    if !request.game_support {
        return Err(Failure::refused(
            "This session serves without game support. Stop it and serve again without --server-only.",
        ));
    }
    match status.state {
        State::Stopping => return Err(Failure::refused("The session is stopping.")),
        State::Starting | State::Activation => {
            return Err(Failure::refused(
                "The game is already starting in this session.",
            ));
        }
        _ if status.game_running => {
            return Err(Failure::refused(
                "The game is already running in this session.",
            ));
        }
        _ => {}
    }
    let play = run.join(PLAY_FILE);
    if play.exists() {
        return Err(Failure::refused(
            "A game start was already requested for this session.",
        ));
    }
    ready()?;
    match File::options().write(true).create_new(true).open(&play) {
        Ok(_) => Ok(PlayRequested {
            play_file: play,
            status,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(Failure::refused(
            "A game start was already requested for this session.",
        )),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait until no process holds the lock. Another test may be spawning a
    /// child at the same moment, and on Unix a forked child shares the open
    /// file description, and so the lock, until it executes its program.
    fn wait_released(data: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while lock_is_held(data).unwrap() != Some(false) {
            assert!(
                std::time::Instant::now() < deadline,
                "lock was never released"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
                action: Some(crate::records::Action::Serve),
                game_running: false,
                game_pid: None,
                server_pid: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn the_lock_is_exclusive_and_observable() {
        let data = crate::testing::scratch("session");
        assert_eq!(lock_is_held(&data).unwrap(), None);
        let held = Lock::acquire(&data).unwrap();
        assert_eq!(lock_is_held(&data).unwrap(), Some(true));
        assert!(Lock::acquire(&data).is_err());
        drop(held);
        wait_released(&data);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn status_derives_interrupted_when_no_supervisor_holds_the_lock() {
        let data = crate::testing::scratch("session");
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
        wait_released(&data);
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
        let data = crate::testing::scratch("session");
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
        let StopOutcome::Requested { stop_file, status } = &first else {
            panic!("{first:?}")
        };
        assert!(stop_file.exists());
        assert_eq!(status.state, State::Ready);
        assert_eq!(
            request_stop(&data).unwrap(),
            first,
            "repeat requests are idempotent"
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn mismatched_pointer_and_status_are_refused() {
        let data = crate::testing::scratch("session");
        point_current_session(&data, &"a".repeat(32)).unwrap();
        write_status(&data, &"a".repeat(32), State::Ready);
        let run = paths::session_dir(&data, &"a".repeat(32));
        let mut status: Status = records::read_json(&run.join("status.json")).unwrap();
        status.id = "b".repeat(32);
        records::replace_json(&run.join("status.json"), &status).unwrap();
        assert!(current_status(&data).is_err());
        std::fs::remove_dir_all(data).unwrap();
    }

    /// A current session started as `action`, with its request.
    fn play_session(data: &Path, id: &str, action: Action, game_support: bool) {
        write_status(data, id, State::Ready);
        let run = paths::session_dir(data, id);
        let mut status: Status = records::read_json(&run.join("status.json")).unwrap();
        status.action = Some(action);
        records::replace_json(&run.join("status.json"), &status).unwrap();
        let request = Request {
            bundled_server: false,
            version: 1,
            id: id.to_owned(),
            action,
            game_directory: Some(PathBuf::from("game")),
            data_directory: data.to_owned(),
            server_path: PathBuf::from("server"),
            server_sha256: "e".repeat(64),
            player_name: "Driver".into(),
            record_protocol: false,
            events: false,
            game_support,
            settings: Default::default(),
        };
        let _ = std::fs::remove_file(run.join("request.json"));
        records::write_new_json(&run.join("request.json"), &request).unwrap();
        point_current_session(data, id).unwrap();
    }

    fn update_status(data: &Path, id: &str, change: impl FnOnce(&mut Status)) {
        let path = paths::session_dir(data, id).join("status.json");
        let mut status: Status = records::read_json(&path).unwrap();
        change(&mut status);
        records::replace_json(&path, &status).unwrap();
    }

    fn refusal(result: Result<PlayRequested>) -> (crate::exit::Exit, String) {
        let failure = result.unwrap_err();
        (failure.exit, failure.message)
    }

    #[test]
    fn play_needs_a_live_session() {
        use crate::exit::Exit;
        let data = crate::testing::scratch("play");
        let (exit, message) = refusal(request_play(&data, || Ok(())));
        assert_eq!(exit, Exit::NoSession);
        assert!(message.contains("No Afterhours session"));
        let id = "1".repeat(32);
        play_session(&data, &id, Action::Serve, true);
        // No supervisor holds the lock: the session reads as interrupted.
        assert_eq!(refusal(request_play(&data, || Ok(()))).0, Exit::NoSession);
        update_status(&data, &id, |s| s.state = State::Finished);
        let (exit, message) = refusal(request_play(&data, || Ok(())));
        assert_eq!(exit, Exit::NoSession);
        assert!(message.contains("has ended"));
        assert!(!paths::session_dir(&data, &id).join(PLAY_FILE).exists());
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn play_is_refused_unless_a_serve_session_can_start_a_game() {
        use crate::exit::Exit;
        let data = crate::testing::scratch("play");
        let _held = Lock::acquire(&data).unwrap();
        let start = "2".repeat(32);
        play_session(&data, &start, Action::Start, true);
        let (exit, message) = refusal(request_play(&data, || Ok(())));
        assert_eq!(exit, Exit::Refused);
        assert!(message.contains("already runs the game"), "{message}");

        let serve = "3".repeat(32);
        play_session(&data, &serve, Action::Serve, false);
        let (exit, message) = refusal(request_play(&data, || Ok(())));
        assert_eq!(exit, Exit::Refused);
        if crate::platform::supports_game_launch() {
            assert!(message.contains("--server-only"), "{message}");
        } else {
            assert!(message.contains("Windows only"), "{message}");
        }
        let run = paths::session_dir(&data, &serve);
        assert!(!run.join(PLAY_FILE).exists(), "a refusal writes nothing");
        std::fs::remove_dir_all(data).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn play_requests_once_and_refuses_while_a_game_starts_or_runs() {
        use crate::exit::Exit;
        let data = crate::testing::scratch("play");
        let _held = Lock::acquire(&data).unwrap();
        let id = "4".repeat(32);
        play_session(&data, &id, Action::Serve, true);
        let run = paths::session_dir(&data, &id);

        // A failing outside check writes nothing.
        let (exit, message) = refusal(request_play(&data, || {
            Err(Failure::refused("Close the existing NFS16 process."))
        }));
        assert_eq!(exit, Exit::Refused);
        assert!(message.contains("NFS16"));
        assert!(!run.join(PLAY_FILE).exists());

        let requested = request_play(&data, || Ok(())).unwrap();
        assert_eq!(requested.play_file, run.join(PLAY_FILE));
        assert!(requested.play_file.exists());
        assert_eq!(requested.status.action, Some(Action::Serve));
        let (exit, message) = refusal(request_play(&data, || Ok(())));
        assert_eq!(exit, Exit::Refused);
        assert!(message.contains("already requested"), "{message}");

        // The supervisor consumed it and is starting, running or stopping.
        std::fs::remove_file(run.join(PLAY_FILE)).unwrap();
        for (state, running, expected) in [
            (State::Starting, false, "already starting"),
            (State::Activation, true, "already starting"),
            (State::Ready, true, "already running"),
            (State::Stopping, false, "stopping"),
        ] {
            update_status(&data, &id, |s| {
                s.state = state;
                s.game_running = running;
            });
            let (exit, message) = refusal(request_play(&data, || Ok(())));
            assert_eq!(exit, Exit::Refused, "{state}");
            assert!(message.contains(expected), "{state}: {message}");
            assert!(!run.join(PLAY_FILE).exists());
        }
        // Serving again after the game: a new request is accepted.
        update_status(&data, &id, |s| {
            s.state = State::Ready;
            s.game_running = false;
        });
        assert!(request_play(&data, || Ok(())).is_ok());
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_session_without_its_supervisor_never_reports_a_running_game() {
        let data = crate::testing::scratch("play");
        let id = "5".repeat(32);
        play_session(&data, &id, Action::Serve, true);
        update_status(&data, &id, |s| s.game_running = true);
        let status = current_status(&data).unwrap().unwrap();
        assert_eq!(status.state, State::Interrupted);
        assert!(!status.game_running);
        assert_eq!(status.action, Some(Action::Serve));
        std::fs::remove_dir_all(data).unwrap();
    }
}
