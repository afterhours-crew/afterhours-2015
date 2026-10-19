// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The small JSON files the launcher reads and writes.
//!
//! Every record carries `version: 1` and is bounded to 64 KiB on read. Files
//! are created with create-new semantics or replaced atomically through a
//! temporary file and rename, so a reader never observes a partial record.
//! The formats are the same ones the PowerShell launcher wrote, so existing
//! data directories keep working. [`Status`] and [`Preflight`] are also
//! command output and part of the JSON contract in [`crate::contract`].

use crate::{
    exit::{Failure, Result},
    paths,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Largest record the launcher will read.
pub const MAX_RECORD_BYTES: u64 = 65_536;

/// Current record version for every file in this module.
pub const VERSION: u32 = 1;

/// Whether `text` is `len` lowercase hexadecimal characters.
pub fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Session and account identifiers: 32 lowercase hexadecimal characters.
pub fn is_id(text: &str) -> bool {
    is_lower_hex(text, 32)
}

/// Read a record file's bytes after checking it is an ordinary, bounded file.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)?;
    if paths::is_link(&meta) || !meta.is_file() || meta.len() > MAX_RECORD_BYTES {
        return Err(Failure::refused("Invalid or oversized launcher record."));
    }
    Ok(std::fs::read(path)?)
}

/// Read and parse a record.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_bounded(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Failure::refused(format!("Invalid launcher record {}: {e}", path.display())))
}

/// Write a record to a path that must not exist yet, then flush it to disk.
pub fn write_new_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}

/// Replace a record atomically: write a sibling temporary file, flush, rename.
pub fn replace_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| Failure::internal("record path has no file name"))?;
    let mut temporary = PathBuf::from(name);
    temporary
        .as_mut_os_string()
        .push(format!(".{}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary);
    write_new_json(&temporary, value)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// `current-session.json`: which session directory is current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPointer {
    pub version: u32,
    pub id: String,
}

/// `account.json`: which saved account this data directory uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountRecord {
    pub version: u32,
    /// The 32-character account ID; the save is `artifacts/account/<id>.sqlite`.
    pub local_account: String,
    /// Story progression the account was created with, `post-intro`.
    pub progression: String,
}

impl AccountRecord {
    pub fn post_intro(local_account: String) -> Self {
        Self {
            version: VERSION,
            local_account,
            progression: "post-intro".into(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION || !is_id(&self.local_account) {
            return Err(Failure::refused(
                "Existing account record is invalid; no state was replaced.",
            ));
        }
        Ok(())
    }
}

/// What a session was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Prepare content and the account; start nothing.
    Setup,
    /// Prepare, then run the server without a game.
    Serve,
    /// Prepare, run the server, start and route the game.
    Start,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Action::Setup => "setup",
            Action::Serve => "serve",
            Action::Start => "start",
        })
    }
}

/// `artifacts/launcher/<id>/request.json`: the supervisor's input, written
/// by the front-end process before the supervisor starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Select the server role of the single-executable distribution. Old
    /// requests continue to select a standalone server.
    #[serde(default)]
    pub bundled_server: bool,
    pub version: u32,
    pub id: String,
    pub action: Action,
    pub game_directory: Option<PathBuf>,
    pub data_directory: PathBuf,
    pub server_path: PathBuf,
    pub server_sha256: String,
    pub player_name: String,
    pub record_protocol: bool,
    /// Report the session as JSON lines on standard output (see
    /// [`crate::events`]) instead of human console text.
    #[serde(default)]
    pub events: bool,
    /// Start the server so the game can connect: under the compatibility
    /// name with the launcher-services port. Always so for `start`; for
    /// `serve` it lets a play request start the game later.
    #[serde(default)]
    pub game_support: bool,
    /// Traffic and rival amounts for this session; older requests use the
    /// original ones.
    #[serde(default)]
    pub settings: crate::settings::Settings,
}

impl Request {
    /// Whether the server is started for the game, as `start` does.
    pub fn serves_game(&self) -> bool {
        self.action == Action::Start || self.game_support
    }
    /// Server children select the bundled role explicitly, including setup
    /// tools and the SDK-named compatibility copy.
    pub fn server_arguments(&self, mut arguments: Vec<String>) -> Vec<String> {
        if self.bundled_server {
            arguments.insert(0, crate::commands::BUNDLED_SERVER_FLAG.into());
        }
        arguments
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION
            || !is_id(&self.id)
            || !crate::digest::is_sha256_hex(&self.server_sha256)
            || (self.action == Action::Start && self.game_directory.is_none())
        {
            return Err(Failure::refused("Invalid launch request."));
        }
        self.settings.validate()
    }
}

/// Session states, in the order a successful session passes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Reading the installation, preparing content and the account.
    Preparing,
    /// The server is ready and the game is starting (`start` only).
    Starting,
    /// The installed EA activation helper shows a window; no timeout.
    Activation,
    /// Routing applied (`start`) or the server is listening (`serve`).
    Ready,
    /// A stop was requested; closing the game, then the server.
    Stopping,
    /// The session ended normally.
    Finished,
    /// The session ended with an error; `message` says why.
    Failed,
    /// Derived on read, never written: a live state whose supervisor is gone.
    Interrupted,
}

impl State {
    /// Whether the session can still change.
    pub fn is_live(self) -> bool {
        !matches!(self, State::Finished | State::Failed | State::Interrupted)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
        f.write_str(text.as_str().unwrap_or("unknown"))
    }
}

/// `artifacts/launcher/<id>/status.json`: the supervisor's latest state.
/// Also the final record of `setup`, `serve` and `start` with `--json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    pub version: u32,
    /// Session ID: 32 lowercase hexadecimal characters.
    pub id: String,
    /// What the session was started as. `serve` hosts the server and may
    /// start the game on request; `null` only in records written before
    /// this field existed.
    #[serde(default)]
    pub action: Option<Action>,
    /// Whether the session's game process is running: from its start until
    /// it exits. With `action` `serve` this tells hosting from playing.
    /// Always `false` for an interrupted session.
    #[serde(default)]
    pub game_running: bool,
    /// When this state was recorded, UTC. Records written by the old
    /// PowerShell launcher use a longer ISO 8601 form with an offset.
    pub utc: String,
    pub state: State,
    /// What the session is doing or why it ended, for a person to read.
    pub message: String,
    /// Supervisor process ID when recorded. Process IDs may be reused after
    /// the process exits; verify identity before acting on one.
    pub worker_pid: u32,
    pub game_pid: Option<u32>,
    pub server_pid: Option<u32>,
}

/// `check` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Preflight {
    pub version: u32,
    pub game_directory: Option<PathBuf>,
    pub data_directory: PathBuf,
    pub server_path: PathBuf,
    /// SHA-256 of the server executable, lowercase hexadecimal.
    pub server_sha256: String,
    /// SHA-256 of the installed `NFS16.exe`; `null` when no game was checked.
    pub game_sha256: Option<String>,
    /// SHA-256 of the installed activation helper; `null` when not checked.
    pub activation_sha256: Option<String>,
    /// The existing EA client registration the game uses, read-only.
    pub registered_client_path: Option<String>,
    /// File name the server runs under so the game accepts it (`start` only).
    pub compatibility_name: Option<String>,
    /// Always `false`: the launcher never needs administrator rights.
    pub requires_administrator: bool,
    /// Always empty: the launcher changes no machine-wide setting.
    pub machine_changes: Vec<String>,
    /// `checked during setup`: `check` itself writes nothing.
    pub directory_write_access: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_written_by_the_powershell_launcher_still_parse() {
        let status: Status = serde_json::from_str(
            r#"{"version":1,"id":"0123456789abcdef0123456789abcdef","utc":"2026-10-14T10:00:00.0000000+00:00","state":"ready","message":"Local routing is ready.","worker_pid":4242,"game_pid":null,"server_pid":4243}"#,
        )
        .unwrap();
        assert_eq!(status.state, State::Ready);
        assert_eq!(status.game_pid, None);
        assert!(status.state.is_live());
        assert_eq!((status.action, status.game_running), (None, false));
        let account: AccountRecord = serde_json::from_str(
            r#"{"version":1,"local_account":"0123456789abcdef0123456789abcdef","progression":"post-intro"}"#,
        )
        .unwrap();
        account.validate().unwrap();
        let pointer: SessionPointer =
            serde_json::from_str(r#"{"version":1,"id":"0123456789abcdef0123456789abcdef"}"#)
                .unwrap();
        assert!(is_id(&pointer.id));
    }

    #[test]
    fn identifiers_must_be_lowercase_hex_of_the_right_length() {
        assert!(is_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!is_id("0123456789abcdef0123456789abcde"));
        assert!(!AccountRecord::post_intro("short".into()).validate().is_ok());
    }

    #[test]
    fn create_new_refuses_to_overwrite_and_replace_is_atomic() {
        let dir = crate::testing::scratch("records");
        let path = dir.join("record.json");
        let first = SessionPointer {
            version: 1,
            id: "a".repeat(32),
        };
        write_new_json(&path, &first).unwrap();
        assert!(
            write_new_json(
                &path,
                &SessionPointer {
                    version: 1,
                    id: "b".repeat(32)
                }
            )
            .is_err()
        );
        assert_eq!(read_json::<SessionPointer>(&path).unwrap(), first);
        let second = SessionPointer {
            version: 1,
            id: "c".repeat(32),
        };
        replace_json(&path, &second).unwrap();
        assert_eq!(read_json::<SessionPointer>(&path).unwrap(), second);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file remains"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_or_non_file_records_are_refused() {
        let dir = crate::testing::scratch("records");
        let big = dir.join("big.json");
        std::fs::write(&big, vec![b' '; MAX_RECORD_BYTES as usize + 1]).unwrap();
        assert!(read_bounded(&big).is_err());
        assert!(read_bounded(&dir).is_err());
        assert!(read_json::<Status>(&dir.join("absent.json")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn request_validation_requires_a_game_only_for_start() {
        let mut request = Request {
            bundled_server: false,
            version: 1,
            id: "d".repeat(32),
            action: Action::Serve,
            game_directory: None,
            data_directory: PathBuf::from("data"),
            server_path: PathBuf::from("nfs-server"),
            server_sha256: "e".repeat(64),
            player_name: "Driver".into(),
            record_protocol: false,
            events: false,
            game_support: false,
            settings: Default::default(),
        };
        request.validate().unwrap();
        let mut legacy = serde_json::to_value(&request).unwrap();
        legacy.as_object_mut().unwrap().remove("settings");
        assert_eq!(
            serde_json::from_value::<Request>(legacy).unwrap().settings,
            crate::settings::Settings::default()
        );
        request.settings.traffic_percent = 301;
        assert!(request.validate().is_err());
        request.settings.traffic_percent = 100;
        let plain = vec![
            "prepare-local".to_owned(),
            "--root".to_owned(),
            "data".to_owned(),
        ];
        assert_eq!(request.server_arguments(plain.clone()), plain);
        let mut legacy = serde_json::to_value(&request).unwrap();
        legacy.as_object_mut().unwrap().remove("bundled_server");
        legacy.as_object_mut().unwrap().remove("events");
        legacy.as_object_mut().unwrap().remove("game_support");
        let legacy = serde_json::from_value::<Request>(legacy).unwrap();
        assert!(!legacy.bundled_server && !legacy.events && !legacy.game_support);
        assert!(
            !legacy.serves_game(),
            "a serve request without the field serves no game"
        );
        request.bundled_server = true;
        let selected = request.server_arguments(plain.clone());
        assert_eq!(selected[0], crate::commands::BUNDLED_SERVER_FLAG);
        assert_eq!(selected[1..], plain);
        request.game_support = true;
        assert!(request.serves_game());
        request.game_support = false;
        request.action = Action::Start;
        assert!(request.serves_game(), "start always serves the game");
        assert!(request.validate().is_err());
        request.game_directory = Some(PathBuf::from("game"));
        request.validate().unwrap();
        request.server_sha256.pop();
        assert!(request.validate().is_err());
        assert_eq!(serde_json::to_string(&Action::Start).unwrap(), "\"start\"");
        assert_eq!(State::Activation.to_string(), "activation");
    }
}
