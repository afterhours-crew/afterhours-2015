// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `status`, `stop`, `play` and `api`: read-only observation and the
//! file-based requests a live supervisor consumes. The session API answers
//! with the same reports ([`crate::api`]).

use super::Context;
use crate::{
    contract::{self, PlayReport, StatusReport, StopReport},
    exit::{Failure, Result},
    platform::{self, Platform},
    session::{self, StopOutcome},
};
use std::path::Path;

/// The `status` record of the data directory's current session.
pub(crate) fn status_report(data: &Path) -> Result<StatusReport> {
    Ok(StatusReport::new(session::current_status(data)?))
}

/// Ask the live supervisor to stop; the `stop` record, or exit 4 when no
/// session was ever started.
pub(crate) fn stop_report(data: &Path) -> Result<StopReport> {
    match session::request_stop(data)? {
        StopOutcome::NoSession => Err(Failure::no_session(
            "No Afterhours session has been started.",
        )),
        StopOutcome::AlreadyOver(status) => Ok(StopReport::new(
            contract::StopOutcome::AlreadyOver,
            status,
            None,
        )),
        StopOutcome::Requested { stop_file, status } => Ok(StopReport::new(
            contract::StopOutcome::Requested,
            status,
            Some(stop_file),
        )),
    }
}

/// Ask the live `serve` supervisor to start the game; the `play` record.
pub(crate) fn play_report(data: &Path) -> Result<PlayReport> {
    let requested = session::request_play(data, || {
        // As `start` checks before launching: no second game or EA app. The
        // session's own server carries a Debug compatibility name.
        let running = platform::native().running_processes(platform::PLAY_CONFLICTS)?;
        match running.first() {
            Some(name) => Err(Failure::refused(format!(
                "Close the existing {name} process before starting the game."
            ))),
            None => Ok(()),
        }
    })?;
    Ok(PlayReport::new(requested.status, requested.play_file))
}

/// `status`: print the current session's status record.
pub fn status(context: &Context) -> Result<()> {
    let report = status_report(&context.data_dir)?;
    match &report.session {
        None => context.emit(&report, || {
            "No Afterhours session has been started.".to_owned()
        }),
        Some(status) => context.emit(&report, || {
            let kind = match (status.action, status.game_running) {
                (_, true) => "game running".to_owned(),
                (Some(action), false) => action.to_string(),
                (None, false) => "earlier launcher".to_owned(),
            };
            format!(
                "{} : {} (session {}, {kind}, {})\nRecorded process IDs: supervisor {}, server {}, game {}. Verify identity before stopping a recorded process.",
                status.state, status.message, status.id, status.utc,
                status.worker_pid,
                status.server_pid.map_or_else(|| "none".into(), |id| id.to_string()),
                status.game_pid.map_or_else(|| "none".into(), |id| id.to_string())
            )
        }),
    }
}

/// `stop`: ask the live supervisor to close the game, then the server.
pub fn stop(context: &Context) -> Result<()> {
    let report = stop_report(&context.data_dir)?;
    let text = match report.outcome {
        contract::StopOutcome::AlreadyOver => report.status.message.clone(),
        contract::StopOutcome::Requested => {
            "Requested game shutdown, followed by server shutdown.".to_owned()
        }
    };
    context.emit(&report, || text)
}

/// `play`: ask the live `serve` supervisor to start the game in its session.
pub fn play(context: &Context) -> Result<()> {
    context.emit(&play_report(&context.data_dir)?, || {
        "Requested the game. Follow it with `nfs-afterhours status`; the server keeps running after the game exits."
            .to_owned()
    })
}

/// `api`: how to reach the running session's API.
pub fn api(context: &Context) -> Result<()> {
    let record = crate::api::current(&context.data_dir)?;
    context.emit(&record, || {
        format!(
            "Session {} serves its API at {}. Send the token in {} as Authorization: Bearer.",
            record.id,
            record.url,
            crate::paths::session_dir(&context.data_dir, &record.id)
                .join(crate::api::FILE)
                .display()
        )
    })
}
