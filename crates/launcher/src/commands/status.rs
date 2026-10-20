// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `status`, `stop` and `play`: read-only observation and the file-based
//! requests a live supervisor consumes.

use super::Context;
use crate::{
    contract::{self, PlayReport, StatusReport, StopReport},
    exit::{Failure, Result},
    platform::{self, Platform},
    session::{self, StopOutcome},
};

/// `status`: print the current session's status record.
pub fn status(context: &Context) -> Result<()> {
    let report = StatusReport::new(session::current_status(&context.data_dir)?);
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
    match session::request_stop(&context.data_dir)? {
        StopOutcome::NoSession => Err(Failure::no_session(
            "No Afterhours session has been started.",
        )),
        StopOutcome::AlreadyOver(status) => {
            let message = status.message.clone();
            context.emit(
                &StopReport::new(contract::StopOutcome::AlreadyOver, status, None),
                || message,
            )
        }
        StopOutcome::Requested { stop_file, status } => context.emit(
            &StopReport::new(contract::StopOutcome::Requested, status, Some(stop_file)),
            || "Requested game shutdown, followed by server shutdown.".to_owned(),
        ),
    }
}

/// `play`: ask the live `serve` supervisor to start the game in its session.
pub fn play(context: &Context) -> Result<()> {
    let requested = session::request_play(&context.data_dir, || {
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
    context.emit(
        &PlayReport::new(requested.status, requested.play_file),
        || {
            "Requested the game. Follow it with `nfs-afterhours status`; the server keeps running after the game exits."
                .to_owned()
        },
    )
}
