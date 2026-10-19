// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live traffic and rival amounts. The server checks the player's
//! `settings.json` once a second by metadata only (modified time and
//! length, which does not open the file for reading) and reads and
//! validates it only when either differs from the last check. A missing,
//! oversized or invalid file keeps the current settings and is logged once.
use crate::ai_settings::{AiSettings, Error};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tracing::{info, warn};

/// How often the file's metadata is checked.
pub const POLL: Duration = Duration::from_secs(1);
/// Largest settings file read; the launcher writes about 45 bytes.
pub const MAX_BYTES: u64 = 4096;

/// What identifies one version of the file without reading it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stamp {
    pub modified: SystemTime,
    pub len: u64,
}

/// File access, so the decisions below can be tested without a disk.
pub trait Files {
    /// Metadata only. A symbolic link or anything but a regular file is an
    /// error.
    fn stamp(&self, path: &Path) -> io::Result<Stamp>;
    /// At most `max` bytes; longer files are an error.
    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>>;
}

/// The operating system's files.
pub struct OsFiles;

impl Files for OsFiles {
    fn stamp(&self, path: &Path) -> io::Result<Stamp> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        Ok(Stamp {
            modified: metadata.modified()?,
            len: metadata.len(),
        })
    }

    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(max + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Same metadata as last time, or the same settings: nothing read or
    /// nothing changed.
    Unchanged,
    /// New settings to apply.
    Applied { old: AiSettings, new: AiSettings },
    /// The file disappeared; the current settings stay.
    Missing,
    /// The file changed but is too large or invalid; the current settings
    /// stay until it changes again.
    Invalid(Option<Error>),
    /// The file changed but could not be read (for example while it is
    /// being replaced); retried on the next check.
    Unreadable,
}

pub struct Watcher {
    path: PathBuf,
    current: AiSettings,
    /// Metadata of the version last read, valid or not.
    seen: Option<Stamp>,
    missing: bool,
}

impl Watcher {
    /// `current` is what the server started with. The first check reads
    /// the file once, which also catches a change made before the start.
    pub fn new(path: PathBuf, current: AiSettings) -> Self {
        Self {
            path,
            current,
            seen: None,
            missing: false,
        }
    }

    pub fn current(&self) -> AiSettings {
        self.current
    }

    pub fn poll(&mut self, files: &impl Files) -> Outcome {
        let Ok(stamp) = files.stamp(&self.path) else {
            self.seen = None;
            let first = !self.missing;
            self.missing = true;
            return if first {
                Outcome::Missing
            } else {
                Outcome::Unchanged
            };
        };
        self.missing = false;
        if self.seen == Some(stamp) {
            return Outcome::Unchanged;
        }
        if stamp.len > MAX_BYTES {
            self.seen = Some(stamp);
            return Outcome::Invalid(None);
        }
        let Ok(bytes) = files.read(&self.path, MAX_BYTES) else {
            return Outcome::Unreadable;
        };
        self.seen = Some(stamp);
        match AiSettings::from_json(&bytes) {
            Ok(new) if new == self.current => Outcome::Unchanged,
            Ok(new) => {
                let old = std::mem::replace(&mut self.current, new);
                Outcome::Applied { old, new }
            }
            Err(error) => Outcome::Invalid(Some(error)),
        }
    }
}

/// Check `path` every `period` until shutdown and publish each valid change.
/// File access runs off the async workers.
pub async fn watch(
    path: PathBuf,
    initial: AiSettings,
    sender: tokio::sync::watch::Sender<AiSettings>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    period: Duration,
) {
    let mut watcher = Some(Watcher::new(path, initial));
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = interval.tick() => {}
        }
        let Some(mut current) = watcher.take() else {
            return;
        };
        let Ok((checked, outcome)) = tokio::task::spawn_blocking(move || {
            let outcome = current.poll(&OsFiles);
            (current, outcome)
        })
        .await
        else {
            warn!("settings file check failed; live traffic and rival changes stop");
            return;
        };
        let path = checked.path.display().to_string();
        watcher = Some(checked);
        match outcome {
            Outcome::Unchanged | Outcome::Unreadable => {}
            Outcome::Applied { old, new } => {
                info!(
                    file = %path,
                    old_traffic_percent = old.traffic_percent(),
                    new_traffic_percent = new.traffic_percent(),
                    old_rival_limit = ?old.rival_limit(),
                    new_rival_limit = ?new.rival_limit(),
                    "traffic and rival settings changed; running sessions apply them now"
                );
                sender.send_replace(new);
            }
            Outcome::Missing => {
                info!(file = %path, "settings file absent; current traffic and rival settings kept")
            }
            Outcome::Invalid(error) => warn!(
                file = %path,
                error = ?error,
                "settings file ignored; current traffic and rival settings kept"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        file: RefCell<Option<(Stamp, Vec<u8>)>>,
        reads: Cell<usize>,
        fail_read: Cell<bool>,
    }

    impl Fake {
        fn write(&self, seconds: u64, text: &str) {
            *self.file.borrow_mut() = Some((
                Stamp {
                    modified: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                    len: text.len() as u64,
                },
                text.as_bytes().to_vec(),
            ));
        }
    }

    impl Files for Fake {
        fn stamp(&self, _: &Path) -> io::Result<Stamp> {
            self.file
                .borrow()
                .as_ref()
                .map(|(stamp, _)| *stamp)
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }
        fn read(&self, _: &Path, max: u64) -> io::Result<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            if self.fail_read.get() {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let bytes = self.file.borrow().as_ref().unwrap().1.clone();
            assert!(bytes.len() as u64 <= max);
            Ok(bytes)
        }
    }

    fn settings(traffic: u16, rivals: Option<u16>) -> AiSettings {
        AiSettings::new(traffic, rivals).unwrap()
    }

    #[test]
    fn an_unchanged_file_is_read_once_and_each_change_once() {
        let files = Fake::default();
        files.write(1, r#"{"traffic_percent": 100, "rival_limit": null}"#);
        let mut watcher = Watcher::new("settings.json".into(), AiSettings::default());
        // The first check reads it; the same settings change nothing.
        assert_eq!(watcher.poll(&files), Outcome::Unchanged);
        for _ in 0..5 {
            assert_eq!(watcher.poll(&files), Outcome::Unchanged);
        }
        assert_eq!(files.reads.get(), 1, "metadata only while unchanged");
        // Up, down, off and back, each read once.
        let mut reads = 1;
        for (second, text, expected) in [
            (
                2,
                r#"{"traffic_percent": 500, "rival_limit": 15}"#,
                settings(500, Some(15)),
            ),
            (
                3,
                r#"{"traffic_percent": 50, "rival_limit": 2}"#,
                settings(50, Some(2)),
            ),
            (
                4,
                r#"{"traffic_percent": 0, "rival_limit": 0}"#,
                settings(0, Some(0)),
            ),
            (
                5,
                r#"{"traffic_percent": 100, "rival_limit": null}"#,
                AiSettings::default(),
            ),
        ] {
            let old = watcher.current();
            files.write(second, text);
            assert_eq!(
                watcher.poll(&files),
                Outcome::Applied { old, new: expected }
            );
            assert_eq!(watcher.poll(&files), Outcome::Unchanged);
            reads += 1;
            assert_eq!(files.reads.get(), reads);
        }
        // Rewritten with the same length at a new time (an atomic replace):
        // read, but the same settings change nothing.
        files.write(6, r#"{"traffic_percent": 100, "rival_limit": null}"#);
        assert_eq!(watcher.poll(&files), Outcome::Unchanged);
        assert_eq!(files.reads.get(), reads + 1);
    }

    #[test]
    fn missing_invalid_oversized_and_unreadable_files_keep_the_settings() {
        let files = Fake::default();
        let start = settings(300, Some(4));
        let mut watcher = Watcher::new("settings.json".into(), start);
        assert_eq!(watcher.poll(&files), Outcome::Missing);
        assert_eq!(watcher.poll(&files), Outcome::Unchanged, "reported once");
        files.write(1, r#"{"traffic_percent": 900}"#);
        assert_eq!(
            watcher.poll(&files),
            Outcome::Invalid(Some(Error::TrafficPercent))
        );
        assert_eq!(watcher.poll(&files), Outcome::Unchanged, "not read again");
        files.write(2, "not json");
        assert_eq!(watcher.poll(&files), Outcome::Invalid(Some(Error::File)));
        let reads = files.reads.get();
        *files.file.borrow_mut() = Some((
            Stamp {
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(3),
                len: MAX_BYTES + 1,
            },
            Vec::new(),
        ));
        assert_eq!(watcher.poll(&files), Outcome::Invalid(None));
        assert_eq!(files.reads.get(), reads, "an oversized file is never read");
        // A failed read is retried on the next check.
        files.write(4, r#"{"traffic_percent": 200}"#);
        files.fail_read.set(true);
        assert_eq!(watcher.poll(&files), Outcome::Unreadable);
        files.fail_read.set(false);
        assert_eq!(
            watcher.poll(&files),
            Outcome::Applied {
                old: start,
                new: settings(200, None)
            }
        );
        // Deleted, then restored with the same metadata: read again.
        let saved = files.file.borrow_mut().take();
        assert_eq!(watcher.poll(&files), Outcome::Missing);
        *files.file.borrow_mut() = saved;
        assert_eq!(watcher.poll(&files), Outcome::Unchanged);
        assert_eq!(watcher.current(), settings(200, None));
    }

    #[test]
    fn os_files_refuse_links_directories_and_oversized_files() {
        let dir = std::env::temp_dir().join(format!(
            "nfs-settings-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{"traffic_percent": 400}"#).unwrap();
        let stamp = OsFiles.stamp(&path).unwrap();
        assert_eq!(stamp.len, 24);
        assert_eq!(OsFiles.read(&path, MAX_BYTES).unwrap().len(), 24);
        assert!(OsFiles.read(&path, 10).is_err());
        assert!(OsFiles.stamp(&dir).is_err());
        assert!(OsFiles.stamp(&dir.join("missing.json")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_running_watch_publishes_saved_changes_and_stops_on_shutdown() {
        let dir = std::env::temp_dir().join(format!("nfs-settings-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{"traffic_percent": 100, "rival_limit": null}"#).unwrap();
        let (sender, mut live) = crate::ai_settings::Live::channel(AiSettings::default());
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(watch(
            path.clone(),
            AiSettings::default(),
            sender,
            shutdown,
            Duration::from_millis(10),
        ));
        // Saved the way the launcher saves: a new file renamed over the old.
        let temporary = dir.join("settings.json.1.tmp");
        std::fs::write(
            &temporary,
            br#"{"traffic_percent": 500, "rival_limit": 15}"#,
        )
        .unwrap();
        std::fs::rename(&temporary, &path).unwrap();
        let changed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(settings) = live.changed() {
                    return settings;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(changed, settings(500, Some(15)));
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
