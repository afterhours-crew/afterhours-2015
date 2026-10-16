// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Command-line surface. Parsing only; behavior lives in the commands.

use crate::exit::Exit;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

const LONG_ABOUT: &str = "\
Prepare, start and supervise the Afterhours 2015 local server, and on Windows
also start and route the installed game.

Everything the launcher writes goes into one dedicated data directory. It
reads the existing client registration but changes no registry value, service,
firewall, DNS or hosts setting, never asks for elevation and never modifies
game files. Run it as an ordinary user.

Typical first run on Windows:
  nfs-afterhours check --game-dir \"F:\\EA Games\\Need for Speed\"
  nfs-afterhours start --game-dir \"F:\\EA Games\\Need for Speed\"

Server only, any platform:
  nfs-afterhours serve --game-dir /mnt/games/nfs2015";

/// `nfs-afterhours`: launcher for the Afterhours 2015 local server and game.
#[derive(Debug, Parser)]
#[command(
    name = "nfs-afterhours",
    version,
    about = "Launcher for the Afterhours 2015 local server and game",
    long_about = LONG_ABOUT,
    after_help = Exit::TABLE,
    after_long_help = Exit::TABLE,
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

/// Options accepted by every subcommand.
#[derive(Debug, Clone, Args)]
pub struct Global {
    /// Dedicated local directory for accounts, content, logs and sessions.
    ///
    /// Defaults to %LOCALAPPDATA%\Afterhours2015 on Windows,
    /// ~/Library/Application Support/Afterhours2015 on macOS and
    /// $XDG_DATA_HOME/afterhours2015 elsewhere. Keep it outside the game
    /// installation; drive roots, network shares and linked paths are refused.
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Print results as one JSON object instead of text.
    #[arg(long, global = true)]
    pub json: bool,
}

/// Arguments shared by the commands that need the installation.
#[derive(Debug, Clone, Args)]
pub struct GameArgs {
    /// Installed Need for Speed (2015) directory containing NFS16.exe.
    #[arg(long, value_name = "DIR")]
    pub game_dir: PathBuf,

    /// Built nfs-server executable.
    ///
    /// Defaults to nfs-server next to this executable, which is where
    /// `cargo build --release` puts both.
    #[arg(long, value_name = "PATH")]
    pub server: Option<PathBuf>,
}

/// Arguments for commands that create or run a session.
#[derive(Debug, Clone, Args)]
pub struct SessionArgs {
    #[command(flatten)]
    pub game: GameArgs,

    /// Display name for the first account. Does not rename an existing one.
    #[arg(long, value_name = "NAME", default_value = "Driver")]
    pub player_name: String,

    /// Record raw protocol payloads for diagnosis.
    ///
    /// Recordings can contain identities and local authentication inputs.
    /// Review them before sharing. Off by default.
    #[arg(long)]
    pub record_protocol: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Verify the installation, paths, registration and conflicts; change nothing.
    ///
    /// Read-only. Hashes the installed game and activation helper, validates
    /// the data directory, reads the client registration and checks for
    /// running game or launcher processes and an occupied port 3216.
    Check(GameArgs),

    /// Prepare content and the local account without starting anything.
    ///
    /// Generates local policy from the installation and creates a separate
    /// local account on first use. Existing accounts are reused, never reset.
    Setup(SessionArgs),

    /// Run the server without launching the game. Works on every platform.
    ///
    /// Prepares like `setup`, then starts the server on loopback and
    /// supervises it until it exits, a stop is requested or Ctrl+C.
    Serve(SessionArgs),

    /// Run the server, then start and route the installed game. Windows only.
    ///
    /// Prepares like `setup`, starts the server, starts the game with the
    /// date marker the client expects, waits for any activation dialog you
    /// complete yourself, applies loopback routing, and supervises both until
    /// the game exits. Quitting the game stops the server.
    Start(SessionArgs),

    /// Report the current session's state.
    ///
    /// Prints the latest status record. A live state whose supervisor no
    /// longer holds the data-directory lock is reported as `interrupted`.
    Status,

    /// Ask the live session to close the game, then the server.
    ///
    /// Writes a stop request the supervisor consumes. Never kills a process
    /// by identifier. Exits with code 4 when there is no live session.
    Stop,

    /// Restore the account pointer for an existing saved database.
    ///
    /// Use after an interrupted setup that committed the database but not
    /// `account.json`. Never creates, imports or resets a database.
    Recover {
        /// The 32-character account ID from the creation output or the
        /// saved `<id>.sqlite` file name.
        #[arg(long, value_name = "ID")]
        account_id: String,
    },

    /// Internal: run a session from a request file. Started by `start`,
    /// `serve` and `setup`; not for direct use.
    #[command(hide = true)]
    Supervise {
        /// Path to the session's request.json.
        #[arg(long, value_name = "FILE")]
        request: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_public_subcommand_has_help_text_and_the_exit_table_is_attached() {
        let command = Cli::command();
        for sub in command.get_subcommands() {
            if sub.is_hide_set() {
                continue;
            }
            assert!(sub.get_about().is_some(), "{} lacks about", sub.get_name());
            assert!(
                sub.get_long_about().is_some(),
                "{} lacks long about",
                sub.get_name()
            );
        }
        let help = command.clone().render_long_help().to_string();
        assert!(help.contains("Exit codes:"));
        assert!(help.contains("4  no live session"));
    }

    #[test]
    fn global_options_parse_before_or_after_the_subcommand() {
        let a = Cli::try_parse_from(["nfs-afterhours", "--json", "status"]).unwrap();
        let b =
            Cli::try_parse_from(["nfs-afterhours", "status", "--json", "--data-dir", "d"]).unwrap();
        assert!(a.global.json && b.global.json);
        assert_eq!(b.global.data_dir, Some(PathBuf::from("d")));
        let start = Cli::try_parse_from([
            "nfs-afterhours",
            "start",
            "--game-dir",
            "G",
            "--record-protocol",
        ])
        .unwrap();
        match start.command {
            Command::Start(args) => {
                assert_eq!(args.game.game_dir, PathBuf::from("G"));
                assert_eq!(args.player_name, "Driver");
                assert!(args.record_protocol);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            Cli::try_parse_from(["nfs-afterhours", "start"]).is_err(),
            "game-dir is required"
        );
        assert!(
            Cli::try_parse_from(["nfs-afterhours", "recover"]).is_err(),
            "account-id is required"
        );
    }
}
