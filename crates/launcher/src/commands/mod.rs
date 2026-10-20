// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One function per subcommand. Each takes parsed arguments, performs the
//! command and returns either nothing or a [`Failure`] with its exit code.
//! Text goes to standard output; with `--json` each command prints exactly
//! one JSON object instead, a record described in [`crate::contract`].

mod launch;
mod map;
mod profile;
mod recover;
mod sessions;
mod settings;
pub(crate) mod status;
mod version;

use crate::{
    cli::{Cli, Command, GameArgs, Global},
    contract::Build,
    exit::{Failure, Result},
    platform,
};
use std::path::PathBuf;

/// Resolved global options.
pub struct Context {
    pub data_dir: PathBuf,
    pub json: bool,
    pub bundled_server: Option<PathBuf>,
}

/// Explicit dispatch argument used by the single-executable distribution.
pub const BUNDLED_SERVER_FLAG: &str = "--afterhours-server";

impl Context {
    fn from(global: &Global) -> Result<Self> {
        let data_dir = match &global.data_dir {
            Some(dir) => dir.clone(),
            None => platform::default_data_dir().ok_or_else(|| {
                Failure::refused("No default data directory for this user; pass --data-dir.")
            })?,
        };
        Ok(Self {
            data_dir,
            json: global.json,
            bundled_server: None,
        })
    }

    /// Print a result as JSON or as the given text.
    pub fn emit<T: serde::Serialize>(
        &self,
        value: &T,
        text: impl FnOnce() -> String,
    ) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string(value)?);
        } else {
            println!("{}", text());
        }
        Ok(())
    }
}

/// Resolve the server path: the given one, or `nfs-server` next to us.
pub fn server_path(context: &Context, args: &GameArgs) -> Result<PathBuf> {
    match args.server.as_ref().or(context.bundled_server.as_ref()) {
        Some(path) => Ok(path.clone()),
        None => platform::default_server_path().ok_or_else(|| {
            Failure::refused(
                "No nfs-server executable next to this launcher; build it or pass --server.",
            )
        }),
    }
}

/// Dispatch a parsed command line.
pub fn run(cli: Cli) -> Result<()> {
    dispatch(cli, None, Build::LAUNCHER)
}

/// Run the existing launcher with the given executable also providing the
/// server role. An explicit `--server` still selects a standalone server.
/// `build` identifies the bundle in `version`.
pub fn run_bundled(cli: Cli, executable: PathBuf, build: Build) -> Result<()> {
    dispatch(cli, Some(executable), build)
}

fn dispatch(cli: Cli, bundled_server: Option<PathBuf>, build: Build) -> Result<()> {
    // These describe the program, not a data directory, so they need none.
    match &cli.command {
        Command::Version => return version::version(cli.global.json, &build),
        Command::Schema { name } => return version::schema(name),
        _ => {}
    }
    let mut context = Context::from(&cli.global)?;
    context.bundled_server = bundled_server;
    match cli.command {
        Command::Check(args) => launch::check(&context, &args),
        Command::Setup(args) => {
            launch::launch(&context, crate::records::Action::Setup, &args, false)
        }
        Command::Serve(args) => launch::launch(
            &context,
            crate::records::Action::Serve,
            &args.session,
            args.server_only,
        ),
        Command::Start(args) => {
            launch::launch(&context, crate::records::Action::Start, &args, false)
        }
        Command::Status => status::status(&context),
        Command::Stop => status::stop(&context),
        Command::Play => status::play(&context),
        Command::Api => status::api(&context),
        Command::Profile(args) => profile::profile(&context, &args),
        Command::CarImages(args) => profile::car_images(&context, &args),
        Command::MapImages(args) => map::map_images(&context, &args),
        Command::Sessions(args) => sessions::sessions(&context, &args),
        Command::Settings(args) => settings::settings(&context, &args),
        Command::Recover { account_id } => recover::recover(&context, &account_id),
        Command::Supervise { request } => launch::supervise(&request, build),
        Command::Version | Command::Schema { .. } => unreachable!("handled above"),
    }
}
