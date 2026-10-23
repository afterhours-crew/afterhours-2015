// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `sessions`: read-only summaries of the session folders.

use super::Context;
use crate::{
    cli::SessionsArgs,
    exit::{Failure, Result},
    platform, records,
    sessions::{self, Options, SessionList, SessionSummary},
};
use std::fmt::Write;

/// `sessions`: list session folders, newest first.
pub fn sessions(context: &Context, args: &SessionsArgs) -> Result<()> {
    let list = sessions_report(&context.data_dir, usize::from(args.limit), args.id.clone())?;
    context.emit(&list, || text(&list))
}

/// The `sessions` record: the newest `limit` sessions, or the one `id`
/// names (exit 4 when none has it); the session API's `GET /v1/sessions`
/// answers with it.
pub(crate) fn sessions_report(
    data: &std::path::Path,
    limit: usize,
    id: Option<String>,
) -> Result<sessions::SessionList> {
    if let Some(id) = &id
        && !records::is_id(id)
    {
        return Err(Failure::refused(
            "A session ID is 32 lowercase hexadecimal characters.",
        ));
    }
    let wanted = id.is_some();
    let mut options = Options::new(limit, id);
    options.crash_dump_directory = platform::game_crash_dump_dir();
    options.ea_desktop_logs = platform::ea_desktop_logs_dir();
    let list = sessions::list(data, &options)?;
    if wanted && list.sessions.is_empty() {
        return Err(Failure::no_session("No session folder has that ID."));
    }
    Ok(list)
}

fn summary(session: &SessionSummary) -> String {
    let mut parts = Vec::new();
    if let Some(log) = &session.server_log {
        parts.push(format!(
            "{} errors, {} warnings, {} unsupported",
            log.levels.error, log.levels.warn, log.unsupported_total
        ));
    }
    if let Some(dumps) = session.crash_dumps.as_ref().filter(|d| !d.is_empty()) {
        parts.push(format!("{} crash dumps", dumps.len()));
    }
    if session.current {
        parts.push("current".into());
    }
    parts.join(", ")
}

fn text(list: &SessionList) -> String {
    let mut out = String::new();
    if list.sessions.is_empty() {
        out.push_str("No Afterhours sessions in this data directory.");
        return out;
    }
    let _ = writeln!(
        out,
        "Sessions in {} (newest first):",
        list.data_directory.display()
    );
    for session in &list.sessions {
        let state = session
            .state
            .map_or_else(|| "unknown".to_owned(), |s| s.to_string());
        let action = session
            .action
            .map_or_else(|| "?".to_owned(), |a| a.to_string());
        let _ = writeln!(
            out,
            "  {}  {action:<5}  {state:<11}  {}  {}",
            session.id,
            session.started_utc.as_deref().unwrap_or("start unknown"),
            summary(session)
        );
        if let Some(log) = &session.server_log {
            for route in log.unsupported.iter().take(5) {
                let _ = writeln!(
                    out,
                    "      unsupported {} {} x{}",
                    route.service.as_deref().unwrap_or("?"),
                    route.route.as_deref().unwrap_or("(no route)"),
                    route.warnings
                );
            }
        }
        for warning in &session.warnings {
            let _ = writeln!(out, "      note: {warning}");
        }
    }
    if list.truncated {
        out.push_str("More sessions exist; pass --limit to list more.\n");
    }
    for warning in &list.warnings {
        let _ = writeln!(out, "note: {warning}");
    }
    out.trim_end().to_owned()
}
