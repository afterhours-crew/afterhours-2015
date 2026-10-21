// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session retention: when a session starts, sessions beyond the newest
//! [`KEPT_SESSIONS`] are removed, so session logs and recordings stop
//! growing without bound. A removed session loses its folder
//! `artifacts/launcher/<id>/` and its recording folder `artifacts/runs/<id>/`.
//!
//! Sessions are ordered as the logs view lists them, newest first, and as
//! many are kept as that view lists by default. The new session and the one
//! `current-session.json` names are always kept. Only folders named by a
//! session id are removed, never a link or what it points to; saves, the
//! account and settings live elsewhere in the data directory and are never
//! touched. A folder that cannot be removed is reported and left in place
//! for the next launch: retention never fails a launch.

use crate::{paths, sessions};
use std::path::Path;

/// Sessions kept, the new one included: as many as the logs view lists.
pub const KEPT_SESSIONS: usize = sessions::DEFAULT_LIMIT;

/// What one pass removed.
#[derive(Debug, Default)]
pub struct Pruned {
    /// Ids of the removed sessions, newest first.
    pub removed: Vec<String>,
    /// Folders left in place, and records that could not be read.
    pub warnings: Vec<String>,
}

/// Remove all but the newest `keep` sessions of `data`, never `new` nor
/// the session `current-session.json` names. Call it holding the data
/// directory's lock, so no other launch adds or reads a session meanwhile.
pub fn prune(data: &Path, keep: usize, new: &str) -> Pruned {
    let mut pruned = Pruned::default();
    let current = sessions::current_pointer(data, &mut pruned.warnings);
    let artifacts = data.join("artifacts");
    let launcher = artifacts.join("launcher");
    let (mut found, _) = sessions::discover(&launcher, &mut pruned.warnings);
    sessions::newest_first(&mut found);
    for candidate in found.into_iter().skip(keep) {
        let id = candidate.id;
        if id == new || current.as_deref() == Some(id.as_str()) {
            continue;
        }
        if let Err(error) = remove_folder(&launcher.join(&id)) {
            pruned
                .warnings
                .push(format!("Session {id} could not be removed: {error}"));
            continue;
        }
        if let Err(error) = remove_folder(&artifacts.join("runs").join(&id)) {
            pruned.warnings.push(format!(
                "The recording of session {id} could not be removed: {error}"
            ));
        }
        pruned.removed.push(id);
    }
    pruned
}

/// Remove a folder and its contents. Absent is done; a link or a file is
/// left in place.
fn remove_folder(path: &Path) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if paths::is_link(&meta) || !meta.is_dir() {
        return Err(format!("{} is not a folder.", path.display()));
    }
    std::fs::remove_dir_all(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{session, testing::scratch};
    use std::{
        path::PathBuf,
        time::{Duration, UNIX_EPOCH},
    };

    fn id(n: usize) -> String {
        format!("{n:032x}")
    }

    /// A session requested `n` minutes after 2026-10-18T08:00:00Z, with a
    /// log and a recording folder.
    fn session(data: &Path, n: usize) -> PathBuf {
        let dir = paths::session_dir(data, &id(n));
        std::fs::create_dir_all(&dir).unwrap();
        let request = dir.join("request.json");
        std::fs::write(&request, b"{}").unwrap();
        let time = UNIX_EPOCH + Duration::from_secs(1_792_310_400 + 60 * n as u64);
        std::fs::File::options()
            .write(true)
            .open(&request)
            .unwrap()
            .set_modified(time)
            .unwrap();
        std::fs::write(dir.join("server.stderr.log"), b"a line\n").unwrap();
        let runs = runs(data, n);
        std::fs::create_dir_all(runs.join("world")).unwrap();
        std::fs::write(runs.join("world").join("frames.bin"), b"\0\x01").unwrap();
        dir
    }

    fn runs(data: &Path, n: usize) -> PathBuf {
        data.join("artifacts").join("runs").join(id(n))
    }

    fn remaining(data: &Path) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(data.join("artifacts").join("launcher"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        ids.sort();
        ids
    }

    fn point_at(data: &Path, n: usize) {
        session::point_current_session(data, &id(n)).unwrap();
    }

    #[test]
    fn keeps_the_newest_and_removes_older_sessions_with_their_recordings() {
        let data = scratch("retention-newest");
        for n in 1..=5 {
            session(&data, n);
        }
        let pruned = prune(&data, 3, &id(5));
        assert_eq!(pruned.removed, [id(2), id(1)]);
        assert!(pruned.warnings.is_empty(), "{:?}", pruned.warnings);
        assert_eq!(remaining(&data), [id(3), id(4), id(5)]);
        assert!(!runs(&data, 1).exists() && !runs(&data, 2).exists());
        assert!(runs(&data, 3).join("world").join("frames.bin").is_file());

        let again = prune(&data, 3, &id(5));
        assert!(again.removed.is_empty() && again.warnings.is_empty());
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn up_to_the_limit_or_without_sessions_nothing_is_removed() {
        let data = scratch("retention-few");
        assert!(prune(&data.join("missing"), 3, &id(1)).removed.is_empty());
        assert!(!data.join("missing").exists(), "creates nothing");
        let empty = prune(&data, 3, &id(1));
        assert!(empty.removed.is_empty() && empty.warnings.is_empty());
        for n in 1..=3 {
            session(&data, n);
        }
        assert!(prune(&data, 3, &id(3)).removed.is_empty());
        assert_eq!(remaining(&data).len(), 3);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_new_and_the_current_session_are_always_kept() {
        let data = scratch("retention-protected");
        for n in 1..=4 {
            session(&data, n);
        }
        // An old session named as current, and a new one whose request
        // reads older than the rest: neither is removed.
        point_at(&data, 1);
        let pruned = prune(&data, 1, &id(2));
        assert_eq!(pruned.removed, [id(3)]);
        assert_eq!(remaining(&data), [id(1), id(2), id(4)]);
        assert!(runs(&data, 1).exists() && runs(&data, 2).exists());
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn only_session_folders_are_removed() {
        let data = scratch("retention-others");
        for n in 1..=3 {
            session(&data, n);
        }
        let launcher = data.join("artifacts").join("launcher");
        std::fs::create_dir(launcher.join("notes")).unwrap();
        std::fs::write(launcher.join(id(9)), b"a file, not a session").unwrap();
        // A recording without a session, and a removed session's recording
        // that is a file: both stay.
        std::fs::create_dir_all(runs(&data, 8)).unwrap();
        std::fs::remove_dir_all(runs(&data, 1)).unwrap();
        std::fs::write(runs(&data, 1), b"not a folder").unwrap();
        let account = data.join("artifacts").join("account");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("save.db"), b"save").unwrap();

        let pruned = prune(&data, 1, &id(3));
        assert_eq!(pruned.removed, [id(2), id(1)]);
        assert_eq!(pruned.warnings.len(), 1, "{:?}", pruned.warnings);
        assert!(pruned.warnings[0].contains(&id(1)));
        assert!(launcher.join("notes").is_dir() && launcher.join(id(9)).is_file());
        assert!(runs(&data, 8).is_dir() && runs(&data, 1).is_file());
        assert_eq!(std::fs::read(account.join("save.db")).unwrap(), b"save");
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn an_unreadable_pointer_is_reported_and_retention_continues() {
        let data = scratch("retention-pointer");
        for n in 1..=3 {
            session(&data, n);
        }
        std::fs::write(data.join(session::POINTER_FILE), b"not json").unwrap();
        let pruned = prune(&data, 2, &id(3));
        assert_eq!(pruned.removed, [id(1)]);
        assert_eq!(pruned.warnings.len(), 1, "{:?}", pruned.warnings);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_session_folder_and_its_target_are_left_alone() {
        let data = scratch("retention-link");
        let outside = scratch("retention-link-target");
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        for n in 2..=3 {
            session(&data, n);
        }
        let launcher = data.join("artifacts").join("launcher");
        std::os::unix::fs::symlink(&outside, launcher.join(id(1))).unwrap();
        let pruned = prune(&data, 1, &id(3));
        assert_eq!(pruned.removed, [id(2)]);
        assert!(outside.join("keep.txt").is_file());
        assert!(std::fs::symlink_metadata(launcher.join(id(1))).is_ok());
        std::fs::remove_dir_all(data).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
}
