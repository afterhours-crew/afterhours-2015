// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Context;
use crate::{cli::StoryArgs, exit::Result, story};

pub fn story(context: &Context, args: &StoryArgs) -> Result<()> {
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
