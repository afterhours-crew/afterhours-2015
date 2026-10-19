// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `status` and `stop`: read-only observation and the file-based stop request.

use super::Context;
use crate::{
    contract::{self, StatusReport, StopReport},
    exit::{Failure, Result},
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
            format!(
                "{} : {} (session {}, {})\nRecorded process IDs: supervisor {}, server {}, game {}. Verify identity before stopping a recorded process.",
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
