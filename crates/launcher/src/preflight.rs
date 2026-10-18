// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only checks before anything is written or started.
//!
//! Preflight establishes everything a session relies on: the caller is an
//! ordinary user, the installation is the supported build, the data
//! directory is a safe place to write, the server executable is present and
//! digested, and on Windows the client registration exists and no game or
//! launcher process or port conflict is in the way. It changes nothing, so
//! `check` can run it on its own.

use crate::{
    compat, digest,
    exit::{Failure, Result},
    paths,
    platform::{self, Platform},
    records::{self, Preflight},
};
use std::path::Path;

/// What to check.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub game_directory: Option<&'a Path>,
    pub data_directory: &'a Path,
    pub server_path: &'a Path,
    /// Whether the session will start the installed game, which adds the
    /// registration, activation helper, process and port checks.
    pub for_game: bool,
}

/// SHA-256 of the supported `NFS16.exe`.
pub const GAME_SHA256: &str = "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df";
/// SHA-256 of the supported `Core/ActivationUI.exe`.
pub const ACTIVATION_SHA256: &str =
    "5901b0ad269e12deecc2bcd2eaa56f8964538a8d6b1a265651d56dbd636daefd";

fn require_hash(game: &Path, relative: &str, expected: &str) -> Result<()> {
    let path = game.join(relative);
    if !path.is_file() {
        return Err(Failure::refused(format!(
            "Installed file missing: {relative}. Pass the directory containing {}.",
            platform::GAME_EXECUTABLE
        )));
    }
    if digest::sha256_file(&path)? != expected {
        return Err(Failure::refused(format!(
            "Unsupported installed file: {relative}. Nothing was changed."
        )));
    }
    Ok(())
}

/// Run every check and describe the result.
pub fn run(platform: &dyn Platform, input: Input<'_>) -> Result<Preflight> {
    eprintln!("[check] Checking user permissions, installation and save paths...");
    platform.refuse_privileged()?;
    let game = match input.game_directory {
        Some(game) => Some(platform::existing_absolute(game, "Game directory")?),
        None => None,
    };
    let data = paths::checked_data_tree(input.data_directory, game.as_deref())?;
    let server = platform::existing_absolute(input.server_path, "Server executable")?;
    if cfg!(windows)
        && server
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("exe"))
    {
        return Err(Failure::refused(
            "Select the built Afterhours server executable.",
        ));
    }
    let mut game_sha256 = None;
    let mut activation_sha256 = None;
    if let Some(game) = &game {
        eprintln!("[check] Verifying NFS16.exe against the supported game build...");
        require_hash(game, platform::GAME_EXECUTABLE, GAME_SHA256)?;
        eprintln!("[check] Supported game build verified.");
        game_sha256 = Some(GAME_SHA256.to_owned());
        if input.for_game {
            eprintln!("[check] Verifying the installed activation helper...");
            require_hash(game, platform::ACTIVATION_HELPER, ACTIVATION_SHA256)?;
            activation_sha256 = Some(ACTIVATION_SHA256.to_owned());
        }
    }
    let mut registered_client_path = None;
    let mut compatibility_name = None;
    if input.for_game {
        eprintln!(
            "[check] Checking existing EA registration, running programs and local port 3216..."
        );
        let registration = platform.registered_client_path()?.ok_or_else(|| {
            Failure::refused(
                "Existing game launcher registration is missing or unsupported. Afterhours does not install or modify it.",
            )
        })?;
        compatibility_name = Some(compat::compatibility_name(&registration)?.to_owned());
        registered_client_path = Some(registration);
        let running = platform.running_processes(platform::CONFLICTING_PROCESSES)?;
        if let Some(name) = running.first() {
            return Err(Failure::refused(format!(
                "Close the existing {name} process before starting Afterhours."
            )));
        }
        if !platform::loopback_port_is_free(platform::LSX_PORT) {
            return Err(Failure::refused(format!(
                "Local port {} is occupied; close the application using it and retry.",
                platform::LSX_PORT
            )));
        }
    }
    eprintln!("[check] Verifying the local server executable...");
    let server_sha256 = digest::sha256_file(&server)?;
    eprintln!(
        "[check] Checks complete. Activation availability is checked by the game when it starts."
    );
    Ok(Preflight {
        version: records::VERSION,
        game_directory: game,
        data_directory: data,
        server_sha256,
        server_path: server,
        game_sha256,
        activation_sha256,
        registered_client_path,
        compatibility_name,
        requires_administrator: false,
        machine_changes: Vec::new(),
        directory_write_access: "checked during setup".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{ChildProcess, SessionGuard, SpawnSpec};

    /// A platform with scripted answers and no processes.
    struct Scripted {
        privileged: bool,
        registration: Option<String>,
        running: Vec<String>,
    }

    impl Platform for Scripted {
        fn refuse_privileged(&self) -> Result<()> {
            if self.privileged {
                Err(Failure::refused("elevated"))
            } else {
                Ok(())
            }
        }
        fn registered_client_path(&self) -> Result<Option<String>> {
            Ok(self.registration.clone())
        }
        fn running_processes(&self, _: &[&str]) -> Result<Vec<String>> {
            Ok(self.running.clone())
        }
        fn process_alive(&self, _: u32) -> bool {
            false
        }
        fn attach_session_guard(&self) -> Result<Box<dyn SessionGuard>> {
            Err(Failure::internal("not in tests"))
        }
        fn spawn(&self, _: &SpawnSpec) -> Result<Box<dyn ChildProcess>> {
            Err(Failure::internal("not in tests"))
        }
    }

    fn ok_platform() -> Scripted {
        Scripted {
            privileged: false,
            registration: Some("C:\\EA\\EADesktop.exe".into()),
            running: Vec::new(),
        }
    }

    /// A fake installation whose NFS16.exe has the supported digest only
    /// if `genuine`; we cannot ship the real file, so tests use the
    /// mismatch path and the missing-file path.
    fn install(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let game = base.join("Game");
        std::fs::create_dir_all(game.join("Core")).unwrap();
        std::fs::write(game.join(platform::GAME_EXECUTABLE), b"not the game").unwrap();
        std::fs::write(game.join(platform::ACTIVATION_HELPER), b"not the helper").unwrap();
        let server = base.join(format!("nfs-server{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&server, b"server bytes").unwrap();
        (game, server)
    }

    #[test]
    fn privilege_missing_game_and_wrong_hash_are_refused_in_that_order() {
        let base = crate::testing::scratch("preflight");
        let (game, server) = install(&base);
        let data = base.join("data");
        let mut platform = ok_platform();
        platform.privileged = true;
        let input = Input {
            game_directory: Some(&game),
            data_directory: &data,
            server_path: &server,
            for_game: false,
        };
        assert_eq!(run(&platform, input).unwrap_err().message, "elevated");
        platform.privileged = false;
        let absent = base.join("absent");
        let missing = Input {
            game_directory: Some(&absent),
            ..input
        };
        assert!(
            run(&platform, missing)
                .unwrap_err()
                .message
                .contains("not found")
        );
        let error = run(&platform, input).unwrap_err();
        assert!(
            error
                .message
                .contains("Unsupported installed file: NFS16.exe"),
            "{error}"
        );
        assert!(!data.exists(), "preflight writes nothing");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn server_only_preflight_without_a_game_reports_the_server_digest() {
        let base = crate::testing::scratch("preflight-server");
        let (_game, server) = install(&base);
        let data = base.join("data");
        let platform = ok_platform();
        let report = run(
            &platform,
            Input {
                game_directory: None,
                data_directory: &data,
                server_path: &server,
                for_game: false,
            },
        )
        .unwrap();
        assert_eq!(report.server_sha256, digest::sha256_bytes(b"server bytes"));
        assert_eq!(report.game_sha256, None);
        assert_eq!(report.registered_client_path, None);
        assert!(report.machine_changes.is_empty());
        assert!(!report.requires_administrator);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn game_sessions_need_registration_and_no_conflicts() {
        // Use a game directory with the real digest files absent to stop
        // before the hash check, so registration and conflicts are what
        // the error is about: the order is hash, registration, processes,
        // port. Here we exercise the registration and process branches by
        // skipping the installation entirely.
        let base = crate::testing::scratch("preflight-game");
        let (_game, server) = install(&base);
        let data = base.join("data");
        let mut platform = ok_platform();
        platform.registration = None;
        let input = Input {
            game_directory: None,
            data_directory: &data,
            server_path: &server,
            for_game: true,
        };
        assert!(
            run(&platform, input)
                .unwrap_err()
                .message
                .contains("registration is missing")
        );
        platform.registration = Some("C:\\x\\cmd.exe".into());
        assert!(
            run(&platform, input)
                .unwrap_err()
                .message
                .contains("Unrecognized launcher registration")
        );
        platform.registration = Some("C:\\EA\\EADesktop.exe".into());
        platform.running = vec!["NFS16".into()];
        assert!(
            run(&platform, input)
                .unwrap_err()
                .message
                .contains("Close the existing NFS16 process")
        );
        platform.running.clear();
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, platform::LSX_PORT));
        if let Ok(_held) = listener {
            assert!(
                run(&platform, input)
                    .unwrap_err()
                    .message
                    .contains("port 3216 is occupied")
            );
        }
        std::fs::remove_dir_all(base).unwrap();
    }
}
