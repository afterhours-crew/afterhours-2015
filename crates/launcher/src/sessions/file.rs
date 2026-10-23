// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One file of one session folder, by name, for a log viewer that reaches
//! the server only through its API (`GET /v1/sessions/<id>/files/<name>`).
//! Only the files a session writes are named, links are not followed, and
//! at most [`MAX_LOG_BYTES`] are read, the bound the session writes to.

use crate::{
    exit::{Failure, Result},
    paths,
    platform::child::MAX_LOG_BYTES,
    records,
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{io::Read, path::Path};

/// Version of [`SessionFileText`].
pub const FORMAT_VERSION: u32 = 1;

/// Files of a session folder that can be read: its records, the stop
/// requests a stop leaves, and each child's output.
pub const NAMES: [&str; 12] = [
    "status.json",
    "request.json",
    "stop",
    "stop-server",
    "prepare.stdout.json",
    "prepare.stderr.log",
    "create-account.stdout.json",
    "create-account.stderr.log",
    "server.stdout.jsonl",
    "server.stderr.log",
    "game.stdout.log",
    "game.stderr.log",
];

/// Whether `name` is a file a session writes: one of [`NAMES`], or the
/// logs of a later game in the same session (`game-2.stdout.log`...).
pub fn is_session_file(name: &str) -> bool {
    if NAMES.contains(&name) {
        return true;
    }
    let Some(rest) = name.strip_prefix("game-") else {
        return false;
    };
    let Some((number, stream)) = rest.split_once('.') else {
        return false;
    };
    !number.is_empty()
        && number.len() <= 4
        && number.bytes().all(|b| b.is_ascii_digit())
        && matches!(stream, "stdout.log" | "stderr.log")
}

/// `GET /v1/sessions/<id>/files/<name>`: the start of one session file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SessionFileText {
    pub version: u32,
    /// The session.
    pub id: String,
    /// The file's name in the session folder.
    pub name: String,
    /// The file's size.
    pub bytes: u64,
    /// The first bytes as text, at most the session's log bound; invalid
    /// UTF-8 is replaced.
    pub text: String,
    /// Whether the file is longer than what `text` holds.
    pub truncated: bool,
}

/// Read one session file. Refused (exit 1) for another name, an invalid
/// session ID or a link; exit 4 when the session or file does not exist.
pub fn read(data: &Path, id: &str, name: &str) -> Result<SessionFileText> {
    if !records::is_id(id) {
        return Err(Failure::refused(
            "A session ID is 32 lowercase hexadecimal characters.",
        ));
    }
    if !is_session_file(name) {
        return Err(Failure::refused(format!(
            "{name} is not a file a session writes."
        )));
    }
    let data = paths::checked_data_path(data, None)?;
    let path = paths::session_dir(&data, id).join(name);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| Failure::no_session(format!("Session {id} has no {name}.")))?;
    if !meta.is_file() {
        return Err(Failure::refused(format!("{name} is not an ordinary file.")));
    }
    let mut kept = Vec::new();
    std::fs::File::open(&path)?
        .take(MAX_LOG_BYTES as u64 + 1)
        .read_to_end(&mut kept)?;
    let truncated = kept.len() > MAX_LOG_BYTES;
    kept.truncate(MAX_LOG_BYTES);
    Ok(SessionFileText {
        version: FORMAT_VERSION,
        id: id.to_owned(),
        name: name.to_owned(),
        bytes: meta.len(),
        text: String::from_utf8_lossy(&kept).into_owned(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit::Exit;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn only_session_files_are_read_whole_up_to_the_bound() {
        assert!(is_session_file("server.stderr.log"));
        assert!(is_session_file("game-2.stdout.log"));
        for other in [
            "../account.json",
            "api.json",
            "game-.stdout.log",
            "game-x.stderr.log",
            "game-12345.stdout.log",
            "game-2.log",
            "settings.json",
            "",
        ] {
            assert!(!is_session_file(other), "{other}");
        }
        let data = crate::testing::scratch("session-file");
        let run = paths::session_dir(&data, ID);
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("server.stderr.log"), b"line one\nline \xff two\n").unwrap();
        let file = read(&data, ID, "server.stderr.log").unwrap();
        assert_eq!(file.text, "line one\nline \u{fffd} two\n");
        assert_eq!((file.bytes, file.truncated), (20, false));
        crate::contract::testing::validate("session-file", &file);
        std::fs::write(run.join("game.stdout.log"), vec![b'x'; MAX_LOG_BYTES + 10]).unwrap();
        let long = read(&data, ID, "game.stdout.log").unwrap();
        assert!(long.truncated && long.text.len() == MAX_LOG_BYTES);
        assert_eq!(
            read(&data, ID, "status.json").unwrap_err().exit,
            Exit::NoSession
        );
        assert_eq!(read(&data, ID, "api.json").unwrap_err().exit, Exit::Refused);
        assert_eq!(
            read(&data, "nope", "status.json").unwrap_err().exit,
            Exit::Refused
        );
        std::fs::create_dir(run.join("game.stderr.log")).unwrap();
        assert_eq!(
            read(&data, ID, "game.stderr.log").unwrap_err().exit,
            Exit::Refused
        );
        std::fs::remove_dir_all(data).unwrap();
    }
}
