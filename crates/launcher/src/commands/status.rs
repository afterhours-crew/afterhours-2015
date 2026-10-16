// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `status` and `stop`: read-only observation and the file-based stop request.

use super::Context;
use crate::{
    exit::{Failure, Result},
    session::{self, StopOutcome},
};

/// `status`: print the current session's status record.
pub fn status(context: &Context) -> Result<()> {
    match session::current_status(&context.data_dir)? {
        None => context.emit(&serde_json::json!({"state": "none"}), || {
            "No Afterhours session has been started.".to_owned()
        }),
        Some(status) => context.emit(&status, || {
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
        StopOutcome::AlreadyOver(status) => context.emit(
            &serde_json::json!({"outcome": "already-over", "status": status}),
            || status.message.clone(),
        ),
        StopOutcome::Requested(path) => context.emit(
            &serde_json::json!({"outcome": "requested", "stop_file": path}),
            || "Requested game shutdown, followed by server shutdown.".to_owned(),
        ),
    }
}
