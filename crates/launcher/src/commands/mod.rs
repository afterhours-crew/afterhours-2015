// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One function per subcommand. Each takes parsed arguments, performs the
//! command and returns either nothing or a [`Failure`] with its exit code.

use crate::{
    cli::{Cli, Command},
    exit::{Failure, Result},
};

/// Dispatch a parsed command line.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Status
        | Command::Stop
        | Command::Check(_)
        | Command::Setup(_)
        | Command::Serve(_)
        | Command::Start(_)
        | Command::Recover { .. }
        | Command::Supervise { .. } => Err(Failure::internal("not implemented yet")),
    }
}
