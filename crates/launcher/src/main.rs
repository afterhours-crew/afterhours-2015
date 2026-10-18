// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use clap::Parser;
use nfs_afterhours::{cli::Cli, exit::Exit};

fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap prints help and version to stdout with success, and usage
            // errors to stderr with exit code 2. Both match the exit table.
            let _ = error.print();
            return if error.use_stderr() {
                Exit::Usage.into()
            } else {
                Exit::Success.into()
            };
        }
    };
    match nfs_afterhours::commands::run(cli) {
        Ok(()) => Exit::Success.into(),
        Err(failure) => {
            nfs_afterhours::terminal::error(&failure);
            failure.exit.into()
        }
    }
}
