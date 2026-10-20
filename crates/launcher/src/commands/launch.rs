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
    contract::Build,
    events::{self, Emitter},
    exit::{Exit, Failure, Result},
    paths, platform, preflight,
    records::{self, Action, Request, Status},
    retention, session, settings,
    supervisor::{self, live::LiveSession},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

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
/// With `--json` the final status is printed as one object; with
/// `--events` the whole session is a JSON-lines stream ending in `final`.
/// With `--api` alone the stream holds only `session` and `final`: the
/// session itself is followed through its API.
/// `server_only` keeps a Windows `serve` session from hosting the game.
pub fn launch(
    context: &Context,
    action: Action,
    args: &SessionArgs,
    server_only: bool,
) -> Result<()> {
    let mut emitter = if args.events || args.api {
        crate::terminal::silence();
        Emitter::stdout()
    } else {
        Emitter::disabled()
    };
    let game_support = action == Action::Start
        || (action == Action::Serve && platform::supports_game_launch() && !server_only);
    let started = start_session(context, action, args, game_support, &mut emitter);
    let (record, result) = match started {
        Ok((run, exit)) => match completed_status(&run) {
            Ok(record) => {
                let result = outcome(&run, exit, record.as_ref());
                (record, result)
            }
            Err(failure) => (None, Err(failure)),
        },
        Err(failure) => (None, Err(failure)),
    };
    if args.events || args.api {
        emitter.emit(events::Body::Final {
            exit_code: result.as_ref().map_or_else(|f| f.exit as u8, |()| 0),
            message: result.as_ref().err().map(|f| f.message.clone()),
            status: record,
        });
    } else if context.json
        && let Some(record) = &record
    {
        println!("{}", serde_json::to_string(record)?);
    }
    result
}

/// Preflight, create the session directory and request, then run the
/// supervisor to completion. Returns the session directory and the
/// supervisor's exit code (`None` when it was terminated). With
/// `game_support` the game checks run and the server starts as for `start`.
fn start_session(
    context: &Context,
    action: Action,
    args: &SessionArgs,
    game_support: bool,
    emitter: &mut Emitter,
) -> Result<(PathBuf, Option<i32>)> {
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
            for_game: game_support,
        },
    )?;
    let data = report.data_directory.clone();
    std::fs::create_dir_all(&data)?;
    let id = new_session_id();
    let run = paths::session_dir(&data, &id);
    let (request_path, run, settings, pruned) = {
        // Hold the lock only while creating the session directory; the
        // supervisor takes it for the session itself.
        let _lock = session::Lock::acquire(&data)?;
        paths::checked_data_path(&run, report.game_directory.as_deref())?;
        // Saved under the lock so concurrent launches never interleave writes.
        let settings = settings::resolve(&data, args.traffic, args.rivals)?;
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
            events: args.events,
            api: args.api,
            game_support,
            settings,
        };
        let request_path = run.join("request.json");
        records::write_new_json(&request_path, &request)?;
        // Under the same lock, so no other launch reads or adds a session.
        let pruned = retention::prune(&data, retention::KEPT_SESSIONS, &id);
        (request_path, run, settings, pruned)
    };
    if !context.json {
        crate::terminal::detail("Saves and cache", data.display());
        crate::terminal::detail("Session logs", run.display());
        if !pruned.removed.is_empty() {
            crate::terminal::detail(
                "Older sessions removed",
                format!(
                    "{} (the newest {} are kept)",
                    pruned.removed.len(),
                    retention::KEPT_SESSIONS
                ),
            );
        }
        for warning in &pruned.warnings {
            crate::terminal::note(warning);
        }
        crate::terminal::detail(
            "Traffic and rivals",
            format!(
                "{} (change with --traffic and --rivals, or edit {})",
                settings.describe(),
                data.join(settings::FILE).display()
            ),
        );
    }
    emitter.emit(events::Body::Session {
        id,
        action,
        data_directory: data,
        session_directory: run.clone(),
    });
    // The supervisor inherits standard output and continues the stream.
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("supervise")
        .arg("--request")
        .arg(&request_path)
        .status()?;
    Ok((run, status.code()))
}

/// The command's result from the supervisor's exit code and final record.
fn outcome(run: &Path, exit: Option<i32>, record: Option<&Status>) -> Result<()> {
    if record.is_some_and(|r| r.state == records::State::Interrupted) {
        return Err(Failure::session(format!(
            "The supervisor was terminated. Inspect {}.",
            run.display()
        )));
    }
    match exit {
        Some(0) if record.is_some_and(|r| r.state == records::State::Finished) => Ok(()),
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
        record.game_running = false;
        record.message = "Supervisor exited before recording completion. Inspect owned processes before restarting; account data is preserved.".into();
    }
    Ok(Some(record))
}

/// `supervise`: run a session from its request file. With `--api` the
/// session API serves the session from just after it becomes current until
/// its last status; its event stream starts with the first status.
pub fn supervise(request_path: &Path, build: Build) -> Result<()> {
    let (request, data, run) = supervisor::live::load_request(request_path)?;
    // With `--api` alone the supervisor writes nothing to standard output:
    // its events go to the API only.
    let mut emitter = if request.events {
        crate::terminal::silence();
        Emitter::stdout()
    } else {
        if request.api {
            crate::terminal::silence();
        }
        Emitter::disabled()
    };
    let started = (|| -> Result<session::Lock> {
        nfs_afterhours_sys::capture_stop_requests()?;
        let lock = session::Lock::acquire(&data)?;
        if run.join(session::STOP_FILE).exists() {
            return Err(Failure::session("Start cancelled before preparation."));
        }
        let digest = crate::digest::sha256_file(&request.server_path)?;
        if digest != request.server_sha256 {
            return Err(Failure::refused(
                "Server changed between request and preflight.",
            ));
        }
        Ok(lock)
    })();
    let _lock = match started {
        Ok(lock) => lock,
        Err(failure) => {
            record_refusal(&run, &request.id, request.action, &failure, &mut emitter);
            return Err(failure);
        }
    };
    let hub = request.api.then(crate::api::hub::Hub::new);
    if let Some(hub) = &hub {
        emitter.attach(Arc::clone(hub));
    }
    begin(&data, &run, &request, &mut emitter)?;
    let action = request.action;
    let id = request.id.clone();
    let api = match hub {
        Some(hub) => {
            let control = Arc::new(crate::api::LiveControl::new(data.clone(), build));
            match crate::api::SessionApi::start(control, hub, &id, &run) {
                Ok(api) => Some(api),
                Err(failure) => {
                    record_failure(&run, &id, action, &failure, &mut emitter);
                    return Err(failure);
                }
            }
        }
        None => None,
    };
    let mut live = match LiveSession::new(request, data, run.clone(), emitter) {
        Ok(live) => live,
        Err(failure) => {
            record_failure(&run, &id, action, &failure, &mut Emitter::disabled());
            if let Some(api) = api {
                api.finish();
            }
            return Err(failure);
        }
    };
    let outcome = supervisor::run(&mut live, action);
    if let Some(api) = api {
        api.finish();
    }
    if outcome.succeeded() {
        Ok(())
    } else {
        Err(Failure::session(outcome.message))
    }
}

/// The message of a session's first status, until its state machine runs.
const STARTING: &str = "Starting the session.";

/// Make the locked session current: record its first status, `preparing`,
/// then point current-session.json at it. In this order `status`, `stop`,
/// `play` and `api` never find the current session without a status; a
/// stop requested now ends the session at its first check. A failure is
/// recorded in the session's status.
fn begin(data: &Path, run: &Path, request: &Request, emitter: &mut Emitter) -> Result<()> {
    let first = supervisor_status(
        &request.id,
        request.action,
        records::State::Preparing,
        STARTING,
    );
    if let Err(failure) = records::replace_json(&run.join("status.json"), &first) {
        record_refusal(run, &request.id, request.action, &failure, emitter);
        return Err(failure);
    }
    emitter.status(&first);
    if let Err(failure) = session::point_current_session(data, &request.id) {
        record_failure(run, &request.id, request.action, &failure, emitter);
        return Err(failure);
    }
    Ok(())
}

/// A status this supervisor records before its state machine runs.
fn supervisor_status(id: &str, action: Action, state: records::State, message: &str) -> Status {
    Status {
        version: records::VERSION,
        id: id.to_owned(),
        action: Some(action),
        game_running: false,
        utc: crate::clock::now_iso(),
        state,
        message: message.to_owned(),
        worker_pid: std::process::id(),
        game_pid: None,
        server_pid: None,
    }
}

/// Record why a supervisor stopped before its session began, so `sessions`
/// and the event stream show the reason. Never replaces an existing status.
fn record_refusal(run: &Path, id: &str, action: Action, failure: &Failure, emitter: &mut Emitter) {
    let record = supervisor_status(id, action, records::State::Failed, &failure.message);
    if records::write_new_json(&run.join("status.json"), &record).is_ok() {
        emitter.status(&record);
    }
}

/// Record why a supervisor stopped after [`begin`], before its state
/// machine ran. The failure replaces the session's first status, which
/// would otherwise read as `preparing` until the lock is released.
fn record_failure(run: &Path, id: &str, action: Action, failure: &Failure, emitter: &mut Emitter) {
    let record = supervisor_status(id, action, records::State::Failed, &failure.message);
    if records::replace_json(&run.join("status.json"), &record).is_ok() {
        emitter.status(&record);
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
            action: Some(crate::records::Action::Serve),
            game_running: false,
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

    #[test]
    fn the_outcome_follows_the_supervisor_exit_and_its_final_record() {
        let run = Path::new("session");
        let mut record = records::Status {
            version: records::VERSION,
            id: "a".repeat(32),
            utc: "2026-10-19T06:00:00Z".into(),
            state: records::State::Finished,
            message: "done".into(),
            worker_pid: 1,
            action: Some(crate::records::Action::Serve),
            game_running: false,
            game_pid: None,
            server_pid: None,
        };
        assert!(outcome(run, Some(0), Some(&record)).is_ok());
        assert_eq!(
            outcome(run, Some(0), None).unwrap_err().exit,
            Exit::SessionFailed
        );
        assert_eq!(
            outcome(run, None, Some(&record)).unwrap_err().exit,
            Exit::SessionFailed
        );
        assert_eq!(outcome(run, Some(1), None).unwrap_err().exit, Exit::Refused);
        assert_eq!(
            outcome(run, Some(3), None).unwrap_err().exit,
            Exit::SessionFailed
        );
        assert_eq!(
            outcome(run, Some(9), None).unwrap_err().exit,
            Exit::Internal
        );
        record.state = records::State::Interrupted;
        assert_eq!(
            outcome(run, Some(0), Some(&record)).unwrap_err().exit,
            Exit::SessionFailed
        );
    }

    #[test]
    fn a_refused_supervisor_records_why_once_and_streams_it() {
        let run = crate::testing::scratch("refusal");
        let capture = crate::events::Capture::default();
        let mut emitter = Emitter::to(capture.clone());
        let failure = Failure::refused("Another Afterhours session holds this data directory.");
        record_refusal(&run, &"b".repeat(32), Action::Serve, &failure, &mut emitter);
        let stored: Status = records::read_json(&run.join("status.json")).unwrap();
        assert_eq!(stored.state, records::State::Failed);
        assert_eq!(stored.message, failure.message);
        let lines = capture.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "status");
        assert_eq!(lines[0]["status"]["state"], "failed");
        // A status the session already wrote is never replaced.
        record_refusal(
            &run,
            &"b".repeat(32),
            Action::Serve,
            &Failure::internal("later"),
            &mut emitter,
        );
        let again: Status = records::read_json(&run.join("status.json")).unwrap();
        assert_eq!(again.message, failure.message);
        assert_eq!(capture.lines().len(), 1);
        std::fs::remove_dir_all(run).unwrap();
    }

    /// A `serve` request for session `id` in `data`, with its directory.
    fn serve_request(data: &Path, id: &str) -> (Request, PathBuf) {
        let run = paths::session_dir(data, id);
        std::fs::create_dir_all(&run).unwrap();
        let request = Request {
            version: records::VERSION,
            id: id.to_owned(),
            action: Action::Serve,
            game_directory: None,
            data_directory: data.to_owned(),
            server_path: PathBuf::from("server"),
            server_sha256: "e".repeat(64),
            bundled_server: false,
            player_name: "Driver".into(),
            record_protocol: false,
            events: true,
            api: false,
            game_support: false,
            settings: Default::default(),
        };
        (request, run)
    }

    fn states(capture: &crate::events::Capture) -> Vec<String> {
        capture
            .lines()
            .iter()
            .map(|line| line["status"]["state"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn a_session_is_current_only_once_its_first_status_is_recorded() {
        // Polled right after the supervisor makes its session current,
        // `status`, `stop` and `api` see a starting session, not a missing
        // status file (exit 5).
        let data = crate::testing::scratch("begin");
        let _lock = session::Lock::acquire(&data).unwrap();
        let id = "c".repeat(32);
        let (request, run) = serve_request(&data, &id);
        let capture = crate::events::Capture::default();
        let mut emitter = Emitter::to(capture.clone());
        begin(&data, &run, &request, &mut emitter).unwrap();

        let status = session::current_status(&data).unwrap().unwrap();
        assert_eq!(status.id, id);
        assert_eq!(status.state, records::State::Preparing);
        assert_eq!(status.message, STARTING);
        assert_eq!(status.action, Some(Action::Serve));
        assert_eq!(status.worker_pid, std::process::id());
        // The stream carries the record exactly as written.
        let lines = capture.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["status"], serde_json::to_value(&status).unwrap());
        // No API yet, and a stop is a request the state machine checks first.
        assert_eq!(
            crate::api::current(&data).unwrap_err().exit,
            Exit::NoSession
        );
        let stop = session::request_stop(&data).unwrap();
        assert!(
            matches!(stop, session::StopOutcome::Requested { ref status, .. } if status.state == records::State::Preparing),
            "{stop:?}"
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_session_that_cannot_become_current_records_why() {
        let data = crate::testing::scratch("begin-failed");
        let _lock = session::Lock::acquire(&data).unwrap();
        // The pointer cannot be replaced while a directory holds its name.
        std::fs::create_dir_all(data.join(session::POINTER_FILE)).unwrap();
        let id = "d".repeat(32);
        let (request, run) = serve_request(&data, &id);
        let capture = crate::events::Capture::default();
        let mut emitter = Emitter::to(capture.clone());
        let failure = begin(&data, &run, &request, &mut emitter).unwrap_err();

        // The failure replaces the first status, so the session never reads
        // as `preparing` once its supervisor has given up.
        let stored: Status = records::read_json(&run.join("status.json")).unwrap();
        assert_eq!(stored.state, records::State::Failed);
        assert_eq!(stored.message, failure.message);
        assert_eq!(states(&capture), ["preparing", "failed"]);
        // A refusal recorded before `begin` would still never replace it.
        record_refusal(
            &run,
            &id,
            Action::Serve,
            &Failure::internal("later"),
            &mut emitter,
        );
        let again: Status = records::read_json(&run.join("status.json")).unwrap();
        assert_eq!(again, stored);
        assert_eq!(states(&capture).len(), 2);
        std::fs::remove_dir_all(data).unwrap();
    }
}
