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
    ///
    /// Each object has a `version`; `schema <record>` prints its JSON Schema.
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

    /// Amount of ambient traffic, as a percentage of the original (0-300).
    ///
    /// 0 turns traffic off. Saved in settings.json in the data directory and
    /// used by later sessions until changed. Default: 100.
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u16).range(0..=300))]
    pub traffic: Option<u16>,

    /// Most roaming rivals active at once (0-128), or `original`.
    ///
    /// 0 turns rivals off; `original` restores the original number. Saved in
    /// settings.json in the data directory like --traffic.
    #[arg(long, value_name = "COUNT", value_parser = crate::settings::parse_rivals)]
    pub rivals: Option<crate::settings::Rivals>,

    /// Report the session as JSON lines on stdout instead of console text.
    ///
    /// One object per line: `session` when the session directory exists,
    /// `status` for every state change, `diagnostic` for each line the
    /// server, game or preparation writes to stderr, and `final` last with
    /// the exit code. Schema: `nfs-afterhours schema event`. Human output
    /// is silenced; the session keeps running if the reader goes away.
    #[arg(long)]
    pub events: bool,
}

/// Arguments for `serve`.
#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    #[command(flatten)]
    pub session: SessionArgs,

    /// Serve without game support, so `play` is unavailable (Windows).
    ///
    /// By default a Windows serve session is ready to host the game: it runs
    /// the same checks as `start` (activation helper, registration, no EA
    /// app or game running, port 3216 free) and starts the server as `start`
    /// does. With this flag none of that applies, as on other platforms.
    #[arg(long)]
    pub server_only: bool,
}

/// Arguments for `sessions`.
#[derive(Debug, Clone, Args)]
pub struct SessionsArgs {
    /// Most sessions to list, newest first (1-200).
    #[arg(long, value_name = "N", default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=200))]
    pub limit: u16,

    /// Show only the session with this 32-character ID.
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,
}

/// Arguments for `car-images`.
#[derive(Debug, Clone, Args)]
pub struct CarImagesArgs {
    /// Installed game to read the images from, read-only. Defaults to the
    /// one the current session started, then the registered one.
    #[arg(long, value_name = "DIR")]
    pub game_dir: Option<PathBuf>,
}

/// Arguments for `profile`.
#[derive(Debug, Clone, Args)]
pub struct ProfileArgs {
    /// List every saved driver: name, REP level, bank and last save.
    #[arg(long, conflicts_with = "account_id")]
    pub list: bool,

    /// The 32-character account ID to show. Defaults to the account in
    /// account.json.
    #[arg(long, value_name = "ID")]
    pub account_id: Option<String>,

    /// Prepared content directory for REP thresholds, table schemas and car
    /// blueprints. Defaults to the one the current session prepared.
    #[arg(long, value_name = "DIR")]
    pub content_dir: Option<PathBuf>,

    /// Installed game to read localized car names from, read-only. Defaults
    /// to the one the current session started, then the registered one.
    #[arg(long, value_name = "DIR", conflicts_with = "list")]
    pub game_dir: Option<PathBuf>,

    /// Locale of the car names, such as en_US or de_DE. Defaults to the
    /// game's registered locale, then en_US.
    #[arg(long, value_name = "LOCALE", conflicts_with = "list")]
    pub language: Option<String>,
}

/// Arguments for `settings`.
#[derive(Debug, Clone, Args)]
pub struct SettingsArgs {
    /// Save the amount of ambient traffic, as a percentage of the original
    /// (0-300); 0 turns traffic off.
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u16).range(0..=300))]
    pub traffic: Option<u16>,

    /// Save the most roaming rivals active at once (0-128), or `original`.
    #[arg(long, value_name = "COUNT", value_parser = crate::settings::parse_rivals)]
    pub rivals: Option<crate::settings::Rivals>,
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

    /// Run the server; on Windows the game can join later with `play`.
    ///
    /// Prepares like `setup`, then starts the server on loopback and
    /// supervises it until it exits, a stop is requested or Ctrl+C. Works on
    /// every platform. On Windows `play` starts the game inside this session;
    /// when the game exits, the server keeps running.
    Serve(ServeArgs),

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

    /// Ask the running `serve` session to start the game. Windows only.
    ///
    /// Writes a play request the serving supervisor consumes. It then starts,
    /// routes and supervises the game exactly as `start` does, in the same
    /// session; follow it with `status` or the session's event stream. When
    /// the game exits the session goes back to serving. Exits with code 4
    /// when no session runs and 1 when the session cannot start the game:
    /// not a serve session, served with --server-only, stopping, or a game
    /// already running or requested.
    Play,

    /// Show a saved driver's profile, or list every saved driver; change nothing.
    ///
    /// Read-only. Opens the account's SQLite save without the data-directory
    /// lock, so it also works while the server runs, and reads REP
    /// thresholds, table schemas and car names from the prepared content.
    /// Stored integers are printed as stored: the game's units for
    /// distances, times and speeds are not confirmed. Exits with code 4
    /// when there is no saved account to show.
    Profile(ProfileArgs),

    /// Decode each car's in-game images from the installation for `profile`.
    ///
    /// Reads the installed game read-only and writes PNG files and an index
    /// to artifacts/content/car-images/<content fingerprint>/ in the data
    /// directory; `profile` then reports them per car. A complete set is
    /// reused, never rewritten. Needs the content a session prepared; exits
    /// with code 4 before the first setup.
    CarImages(CarImagesArgs),

    /// List the session folders and summarize their logs; change nothing.
    ///
    /// Read-only, newest first. For each session: action, start and end,
    /// final state and message, every file with its size and line count,
    /// server log levels and unsupported-operation warnings grouped by
    /// service and route, the protocol recording folder and, on Windows,
    /// game crash dumps written during the session. Every read is bounded.
    Sessions(SessionsArgs),

    /// Show or change the saved traffic and rival amounts.
    ///
    /// Without options, prints the values in settings.json in the data
    /// directory, or the original ones when there is no file yet, and
    /// changes nothing. --traffic and --rivals save new values for later
    /// sessions, as they do on `start` and `serve`; saving is refused while
    /// a session holds the data directory, because a running session keeps
    /// the values it started with. The record also gives the largest
    /// values this build accepts.
    Settings(SettingsArgs),

    /// Show the version, the JSON record versions and the capabilities.
    ///
    /// With --json, the record a desktop front end checks before using any
    /// other command: program and release, source revision, the version of
    /// every JSON record and the optional features this build provides.
    Version,

    /// Print the JSON Schema of one --json record or event line.
    ///
    /// The schemas are generated from the types that print the records and
    /// ship with each release. Record names are listed by `version --json`.
    Schema {
        /// Record name, such as status, sessions or event.
        #[arg(value_name = "RECORD", value_parser = clap::builder::PossibleValuesParser::new(crate::contract::record_names()))]
        name: String,
    },

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
        let profile = Cli::try_parse_from([
            "nfs-afterhours",
            "profile",
            "--json",
            "--content-dir",
            "C",
            "--data-dir",
            "d",
        ])
        .unwrap();
        match profile.command {
            Command::Profile(args) => {
                assert!(!args.list && args.account_id.is_none());
                assert_eq!(args.content_dir, Some(PathBuf::from("C")));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            Cli::try_parse_from(["nfs-afterhours", "profile", "--list", "--account-id", "a"])
                .is_err(),
            "a listing shows every account"
        );
    }

    #[test]
    fn contract_commands_parse_and_bound_their_arguments() {
        let serve = Cli::try_parse_from(["nfs-afterhours", "serve", "--game-dir", "G", "--events"])
            .unwrap();
        assert!(
            matches!(serve.command, Command::Serve(args) if args.session.events && !args.server_only)
        );
        let serve = Cli::try_parse_from([
            "nfs-afterhours",
            "serve",
            "--game-dir",
            "G",
            "--server-only",
        ])
        .unwrap();
        assert!(matches!(serve.command, Command::Serve(args) if args.server_only));
        assert!(
            Cli::try_parse_from([
                "nfs-afterhours",
                "start",
                "--game-dir",
                "G",
                "--server-only"
            ])
            .is_err(),
            "--server-only is a serve option"
        );
        assert!(matches!(
            Cli::try_parse_from(["nfs-afterhours", "play", "--json"])
                .unwrap()
                .command,
            Command::Play
        ));
        match Cli::try_parse_from(["nfs-afterhours", "sessions"])
            .unwrap()
            .command
        {
            Command::Sessions(args) => assert_eq!((args.limit, args.id), (20, None)),
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["nfs-afterhours", "sessions", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["nfs-afterhours", "sessions", "--limit", "201"]).is_err());
        match Cli::try_parse_from(["nfs-afterhours", "settings"])
            .unwrap()
            .command
        {
            Command::Settings(args) => assert_eq!((args.traffic, args.rivals), (None, None)),
            other => panic!("{other:?}"),
        }
        match Cli::try_parse_from([
            "nfs-afterhours",
            "settings",
            "--traffic",
            "200",
            "--rivals",
            "original",
        ])
        .unwrap()
        .command
        {
            Command::Settings(args) => assert_eq!(
                (args.traffic, args.rivals),
                (Some(200), Some(crate::settings::Rivals::Original))
            ),
            other => panic!("{other:?}"),
        }
        for bad in [
            ["--traffic", "301"],
            ["--rivals", "129"],
            ["--rivals", "lots"],
        ] {
            let mut line = vec!["nfs-afterhours", "settings"];
            line.extend(bad);
            assert!(Cli::try_parse_from(line).is_err(), "{bad:?}");
        }
        assert!(Cli::try_parse_from(["nfs-afterhours", "--json", "version"]).is_ok());
        assert!(Cli::try_parse_from(["nfs-afterhours", "schema", "event"]).is_ok());
        assert!(Cli::try_parse_from(["nfs-afterhours", "schema", "nothing"]).is_err());
        assert!(Cli::try_parse_from(["nfs-afterhours", "schema"]).is_err());
    }
}
