// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Context;
use crate::{
    cli::StoryArgs,
    exit::{Failure, Result},
    platform::{self, ChildProcess, Keep, Platform, SpawnSpec},
    profile, story,
};
use nfs_services::story::editing::{Applied, Preview};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub fn story(context: &Context, args: &StoryArgs) -> Result<()> {
    if args.preview.is_some() || args.apply.is_some() {
        return edit(context, args);
    }
    let report = story::report(
        &context.data_dir,
        args.account_id.as_deref(),
        args.content_dir.as_deref(),
    )?;
    context.emit(&report, || {
        let mut lines = vec![format!("Story · account {}", report.account_id)];
        for checkpoint in &report.checkpoints {
            let status = if checkpoint.id == report.current_checkpoint {
                "current"
            } else if checkpoint.completed {
                "complete"
            } else {
                "upcoming"
            };
            lines.push(format!(
                "  {}: {} ({status})",
                checkpoint.id, checkpoint.label
            ));
        }
        lines.extend(report.warnings.clone());
        lines.join("\n")
    })
}

// Keep the helper owned on every return path, including a log error or timeout.
struct Tool(Box<dyn ChildProcess>);
impl Drop for Tool {
    fn drop(&mut self) {
        self.0.kill();
    }
}
fn execute(spec: &SpawnSpec, timeout: Duration) -> Result<serde_json::Value> {
    let mut child = Tool(platform::native().spawn(spec)?);
    let deadline = Instant::now() + timeout;
    while !child.0.exited() || !child.0.logs_finished() {
        let _ = child.0.drain_diagnostics();
        child.0.check_logs()?;
        if Instant::now() > deadline {
            return Err(Failure::session(
                "Story command exceeded its deadline; refresh the saved story before retrying.",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.0.check_logs()?;
    if child.0.exit_code() != Some(0) || child.0.output_truncated() {
        return Err(Failure::refused(format!(
            "Story command failed; see {}. Refresh the saved story before retrying.",
            spec.stderr_log.display()
        )));
    }
    Ok(serde_json::from_slice(&std::fs::read(&spec.stdout_log)?)?)
}
fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::refused("Story paths must be valid UTF-8."))
}
fn edit(context: &Context, args: &StoryArgs) -> Result<()> {
    let saves = profile::Saves::open(&context.data_dir)?;
    let id = args
        .account_id
        .as_deref()
        .or_else(|| saves.active())
        .ok_or_else(|| Failure::no_session("No local driver selected; run setup first."))?;
    let _ = saves.inspect(id)?;
    let content = profile::content::locate(saves.data(), args.content_dir.as_deref())?
        .ok_or_else(|| Failure::no_session("No prepared story content found; run setup first."))?;
    let server = args
        .server
        .as_ref()
        .or(context.bundled_server.as_ref())
        .cloned()
        .or_else(platform::default_server_path)
        .ok_or_else(|| Failure::refused("No server executable found; pass --server."))?;
    let mut arguments = Vec::new();
    if args.server.is_none() && context.bundled_server.is_some() {
        arguments.push(super::BUNDLED_SERVER_FLAG.into());
    }
    arguments.extend([
        "story".into(),
        "--root".into(),
        path_text(saves.data())?,
        "--local-account".into(),
        id.into(),
        "--progression-content".into(),
        path_text(&content.directory.join("progression-content.json"))?,
    ]);
    if let Some(target) = &args.preview {
        arguments.extend(["--preview".into(), target.clone()]);
    } else {
        arguments.extend([
            "--apply".into(),
            args.apply
                .clone()
                .ok_or_else(|| Failure::refused("Missing story target."))?,
            "--expected-generation".into(),
            args.expected_generation
                .clone()
                .ok_or_else(|| Failure::refused("Missing preview generation."))?,
            "--plan-id".into(),
            args.plan_id
                .clone()
                .ok_or_else(|| Failure::refused("Missing story preview."))?,
        ]);
    }
    let log_root = saves.data().join("story-commands");
    if !log_root.exists() {
        std::fs::create_dir(&log_root)?;
    }
    let meta = std::fs::symlink_metadata(&log_root)?;
    if crate::paths::is_link(&meta) || !meta.is_dir() {
        return Err(Failure::refused("Invalid story command log directory."));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Failure::internal("System clock is before the epoch."))?;
    let logs = log_root.join(format!("{}-{}", std::process::id(), now.as_nanos()));
    std::fs::create_dir(&logs)?;
    let value = execute(
        &SpawnSpec {
            executable: server,
            arguments,
            working_directory: saves.data().to_owned(),
            stdout_log: logs.join("result.json"),
            stdout_keeps: Keep::Start,
            stderr_log: logs.join("stderr.log"),
            environment: vec![],
            capture_first_line: false,
            map_feed: false,
        },
        Duration::from_secs(30),
    )?;
    if args.preview.is_some() {
        let report: Preview = serde_json::from_value(value)?;
        if report.version != 1 || report.account_id != id {
            return Err(Failure::refused("Unsupported story preview result."));
        }
        context.emit(&report, || {
            format!(
                "{} → {}\n{}",
                report.source_checkpoint,
                report.target_checkpoint,
                report.reasons.join("\n")
            )
        })
    } else {
        let report: Applied = serde_json::from_value(value)?;
        if report.version != 1 || report.account_id != id {
            return Err(Failure::refused("Unsupported story apply result."));
        }
        context.emit(&report, || {
            format!(
                "Story set to {}. Backup: {}.",
                report.target_checkpoint, report.backup.id
            )
        })
    }
}

#[cfg(test)]
mod tests;
