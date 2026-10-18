// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `check`, `setup`, `serve`, `start` and the hidden `supervise`.
//!
//! The front-end command runs preflight, writes a request file into a new
//! session directory and starts a second copy of this executable with
//! `supervise`. That child holds the data-directory lock, joins the session
//! job on Windows and runs the state machine. Keeping the supervisor in a
//! separate process means the user's terminal is never placed in the job
//! and a crash of the supervisor still triggers the job's cleanup.

use super::{Context, server_path};
use crate::{
    cli::{GameArgs, SessionArgs},
    exit::{Exit, Failure, Result},
    paths, platform, preflight,
    records::{self, Action, Request},
    session,
    supervisor::{self, live::LiveSession},
};
use std::path::Path;

/// `check`: preflight only.
pub fn check(context: &Context, args: &GameArgs) -> Result<()> {
    let server = server_path(context, args)?;
    let report = preflight::run(
        &platform::native(),
        preflight::Input {
            game_directory: Some(&args.game_dir),
            data_directory: &context.data_dir,
            server_path: &server,
            for_game: platform::supports_game_launch(),
        },
    )?;
    context.emit(&report, || {
        let mut lines = vec![
            format!(
                "game directory : {}",
                report
                    .game_directory
                    .as_deref()
                    .map(Path::display)
                    .map(|d| d.to_string())
                    .unwrap_or_default()
            ),
            format!("data directory : {}", report.data_directory.display()),
            format!(
                "server         : {} ({})",
                report.server_path.display(),
                report.server_sha256
            ),
        ];
        if let Some(registration) = &report.registered_client_path {
            lines.push(format!("registration   : {registration}"));
        }
        if let Some(name) = &report.compatibility_name {
            lines.push(format!("compatibility  : {name}"));
        }
        lines.push("machine changes: none; no administrator rights required".into());
        lines.join("\n")
    })
}

/// A new session identifier: 32 lowercase hexadecimal characters that are
/// unique per data directory. Uniqueness, not secrecy, is the requirement.
fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let stack = &nanos as *const u128 as usize;
    let seed = format!("{nanos}-{}-{stack}", std::process::id());
    crate::digest::sha256_bytes(seed.as_bytes())[..32].to_owned()
}

/// `setup`, `serve` and `start`: preflight, write the request, supervise.
pub fn launch(context: &Context, action: Action, args: &SessionArgs) -> Result<()> {
    if action == Action::Start && !platform::supports_game_launch() {
        return Err(Failure::refused(
            "Starting the game is supported on Windows only. Use `nfs-afterhours serve` here.",
        ));
    }
    let server = server_path(context, &args.game)?;
    let report = preflight::run(
        &platform::native(),
        preflight::Input {
            game_directory: Some(&args.game.game_dir),
            data_directory: &context.data_dir,
            server_path: &server,
            for_game: action == Action::Start,
        },
    )?;
    let data = report.data_directory.clone();
    std::fs::create_dir_all(&data)?;
    let id = new_session_id();
    let run = paths::session_dir(&data, &id);
    let (request_path, run) = {
        // Hold the lock only while creating the session directory; the
        // supervisor takes it for the session itself.
        let _lock = session::Lock::acquire(&data)?;
        paths::checked_data_path(&run, report.game_directory.as_deref())?;
        std::fs::create_dir_all(&run)?;
        paths::checked_data_path(&run, report.game_directory.as_deref())?;
        let request = Request {
            version: records::VERSION,
            id: id.clone(),
            action,
            game_directory: report.game_directory.clone(),
            data_directory: data.clone(),
            server_path: report.server_path.clone(),
            server_sha256: report.server_sha256.clone(),
            bundled_server: context.bundled_server.is_some() && args.game.server.is_none(),
            player_name: args.player_name.clone(),
            record_protocol: args.record_protocol,
        };
        let request_path = run.join("request.json");
        records::write_new_json(&request_path, &request)?;
        (request_path, run)
    };
    if !context.json {
        println!("Afterhours {action} - data: {}", data.display());
        println!("Session logs: {}", run.display());
    }
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("supervise")
        .arg("--request")
        .arg(&request_path)
        .status()?;
    let record = completed_status(&run)?;
    if context.json
        && let Some(record) = &record
    {
        println!("{}", serde_json::to_string(record)?);
    }
    if record
        .as_ref()
        .is_some_and(|r| r.state == records::State::Interrupted)
    {
        return Err(Failure::session(format!(
            "The supervisor was terminated. Inspect {}.",
            run.display()
        )));
    }
    match status.code() {
        Some(0)
            if record
                .as_ref()
                .is_some_and(|r| r.state == records::State::Finished) =>
        {
            Ok(())
        }
        Some(0) => Err(Failure::session(format!(
            "Supervisor exited without successful completion. Inspect {}.",
            run.display()
        ))),
        Some(code) => Err(Failure {
            exit: match code {
                1 => Exit::Refused,
                3 => Exit::SessionFailed,
                _ => Exit::Internal,
            },
            message: format!("Afterhours session failed. Inspect {}.", run.display()),
        }),
        None => Err(Failure::session(format!(
            "The supervisor was terminated. Inspect {}.",
            run.display()
        ))),
    }
}

/// Once our supervisor has exited, its last live record is interrupted even
/// if Windows terminated it with exit code zero. Read this session's record,
/// not current-session.json, which another invocation may already have moved.
fn completed_status(run: &Path) -> Result<Option<records::Status>> {
    let path = run.join("status.json");
    if !path.exists() {
        return Ok(None);
    }
    let mut record: records::Status = records::read_json(&path)?;
    if record.state.is_live() {
        record.state = records::State::Interrupted;
        record.message = "Supervisor exited before recording completion. Inspect owned processes before restarting; account data is preserved.".into();
    }
    Ok(Some(record))
}

/// `supervise`: run a session from its request file.
pub fn supervise(request_path: &Path) -> Result<()> {
    let (request, data, run) = supervisor::live::load_request(request_path)?;
    nfs_afterhours_sys::capture_stop_requests()?;
    let _lock = session::Lock::acquire(&data)?;
    if run.join(session::STOP_FILE).exists() {
        return Err(Failure::session("Start cancelled before preparation."));
    }
    let digest = crate::digest::sha256_file(&request.server_path)?;
    if digest != request.server_sha256 {
        return Err(Failure::refused(
            "Server changed between request and preflight.",
        ));
    }
    session::point_current_session(&data, &request.id)?;
    let action = request.action;
    let mut live = LiveSession::new(request, data, run)?;
    let outcome = supervisor::run(&mut live, action);
    if outcome.succeeded() {
        Ok(())
    } else {
        Err(Failure::session(outcome.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_valid_and_distinct() {
        let a = new_session_id();
        let b = new_session_id();
        assert!(records::is_id(&a));
        assert_ne!(a, b);
    }

    #[test]
    fn exited_supervisor_cannot_report_a_live_session() {
        let run = crate::testing::scratch("completed-status");
        assert!(completed_status(&run).unwrap().is_none());
        let mut record = records::Status {
            version: records::VERSION,
            id: "a".repeat(32),
            utc: "2026-10-16T06:00:00Z".into(),
            state: records::State::Ready,
            message: "ready".into(),
            worker_pid: 123,
            game_pid: Some(456),
            server_pid: Some(789),
        };
        for state in [
            records::State::Preparing,
            records::State::Activation,
            records::State::Ready,
            records::State::Stopping,
        ] {
            record.state = state;
            records::replace_json(&run.join("status.json"), &record).unwrap();
            let result = completed_status(&run).unwrap().unwrap();
            assert_eq!(result.state, records::State::Interrupted);
            assert_eq!(result.game_pid, record.game_pid);
        }
        for state in [records::State::Finished, records::State::Failed] {
            record.state = state;
            records::replace_json(&run.join("status.json"), &record).unwrap();
            assert_eq!(completed_status(&run).unwrap(), Some(record.clone()));
        }
        std::fs::remove_dir_all(run).unwrap();
    }
}
