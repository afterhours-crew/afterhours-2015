// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use clap::Parser;
use nfs_afterhours::{cli::Cli, exit::Exit, terminal};
use std::{ffi::OsString, io::IsTerminal, path::Path};

fn arguments(executable: &Path, supplied: Vec<OsString>) -> Vec<OsString> {
    let mut args = vec![executable.as_os_str().to_owned()];
    if supplied.is_empty() {
        args.push("start".into());
        args.push("--game-dir".into());
        args.push(executable.parent().unwrap_or(Path::new(".")).into());
    } else {
        args.extend(supplied);
    }
    args
}

fn run(executable: &Path, supplied: Vec<OsString>, automatic: bool) -> std::process::ExitCode {
    if supplied
        .first()
        .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        print_version();
        return Exit::Success.into();
    }
    if automatic {
        terminal::heading("AFTERHOURS  /  Need for Speed 2015");
        terminal::note("Local single-player preview");
        terminal::detail(
            "Build",
            format_args!(
                "{} ({})",
                env!("CARGO_PKG_VERSION"),
                option_env!("AFTERHOURS_RELEASE").unwrap_or("development")
            ),
        );
        terminal::detail(
            "Game folder",
            executable.parent().unwrap_or(Path::new(".")).display(),
        );
        terminal::note("First launch prepares your game data. Later launches reuse it.");
        terminal::note("Keep this window open while you play. Ctrl+C stops the session.");
        terminal::note("Your installed game needs valid EA activation. If a sign-in window");
        terminal::note(
            "appears, complete it there. Afterhours waits and never sees your password.",
        );
    }
    let cli = match Cli::try_parse_from(arguments(executable, supplied)) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() {
                Exit::Usage.into()
            } else {
                Exit::Success.into()
            };
        }
    };
    match nfs_afterhours::commands::run_bundled(cli, executable.to_path_buf()) {
        Ok(()) => Exit::Success.into(),
        Err(failure) => {
            terminal::error(&failure);
            failure.exit.into()
        }
    }
}

fn print_version() {
    println!(
        "Afterhours {} ({} / {})",
        env!("CARGO_PKG_VERSION"),
        option_env!("AFTERHOURS_RELEASE").unwrap_or("development"),
        option_env!("AFTERHOURS_REVISION").unwrap_or("local build")
    );
}

fn main() -> std::process::ExitCode {
    let mut supplied: Vec<_> = std::env::args_os().skip(1).collect();
    // Every server child, including the verified SDK compatibility copy,
    // explicitly selects this role. File names never determine behavior.
    if supplied
        .first()
        .is_some_and(|arg| arg == nfs_afterhours::commands::BUNDLED_SERVER_FLAG)
    {
        supplied.remove(0);
        return nfs_server::cli::main_from(supplied.into_iter());
    }
    let automatic = supplied.is_empty();
    let result = match std::env::current_exe() {
        Ok(executable) => run(&executable, supplied, automatic),
        Err(error) => {
            terminal::error(format_args!("Cannot locate Afterhours: {error}"));
            Exit::Internal.into()
        }
    };
    // Explorer's terminal would otherwise disappear with the error or outcome.
    // Explicit commands and redirected stdin remain noninteractive for scripts.
    if automatic && std::io::stdin().is_terminal() {
        terminal::note("Press Enter to close this window.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_start_uses_the_executable_directory_not_the_working_directory() {
        let path = Path::new("game with spaces/Afterhours.exe");
        assert_eq!(
            arguments(path, vec![]),
            vec![
                path.as_os_str().to_owned(),
                "start".into(),
                "--game-dir".into(),
                "game with spaces".into()
            ]
        );
        let explicit = vec!["status".into(), "--json".into()];
        assert_eq!(&arguments(path, explicit.clone())[1..], &explicit);
    }
}
