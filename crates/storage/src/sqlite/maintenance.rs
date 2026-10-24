// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-account process ownership and standalone, generation-bound backups.
use super::SqliteRepository;
use crate::{AccountId, Error, Inspection};
use rusqlite::{
    Connection, OpenFlags, TransactionBehavior,
    backup::{Backup, StepResult},
};
use std::{fs::File, path::PathBuf};

fn ordinary_file(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.is_file() && meta.file_attributes() & 0x400 == 0
    }
    #[cfg(not(windows))]
    {
        meta.is_file() && !meta.file_type().is_symlink()
    }
}

/// Held by a running server or one offline editor, never by read-only viewers.
/// Different accounts remain independent. Closing the file releases ownership,
/// including on process exit; no stale PID or lock-file deletion is needed.
#[derive(Debug)]
pub struct WriterLease {
    _file: File,
    root: PathBuf,
    account: AccountId,
}

impl SqliteRepository {
    /// Observe ownership without creating a lock file or changing an account.
    pub fn writer_is_leased(&self, account: AccountId) -> Result<bool, Error> {
        let path = self.path(account).with_extension("writer.lock");
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !ordinary_file(&meta) => {
                return Err(Error::Config);
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(Error::Storage),
        }
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| Error::Storage)?;
        match file.try_lock() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(std::fs::TryLockError::Error(_)) => Err(Error::Storage),
        }
    }

    pub fn writer_lease(&self, account: AccountId) -> Result<WriterLease, Error> {
        let path = self.path(account).with_extension("writer.lock");
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !ordinary_file(&meta) => {
                return Err(Error::Config);
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::Storage),
        }
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|_| Error::Storage)?;
        let meta = std::fs::symlink_metadata(&path).map_err(|_| Error::Storage)?;
        if !ordinary_file(&meta) {
            return Err(Error::Config);
        }
        match file.try_lock() {
            Ok(()) => Ok(WriterLease {
                _file: file,
                root: self.root.clone(),
                account,
            }),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::Busy),
            Err(std::fs::TryLockError::Error(_)) => Err(Error::Storage),
        }
    }

    /// Copy one accepted account generation, including committed WAL pages,
    /// into an empty backup repository. The source is read-only; its read
    /// transaction fixes the generation for the complete copy. The resulting
    /// database uses a rollback journal and needs no sibling WAL to restore.
    ///
    /// Existing destinations are never overwritten. A failed copy removes only
    /// the newly reserved destination. The caller keeps the writer lease until
    /// its later edit commits, and records the backup location before reporting
    /// success. This synchronous operation belongs on an I/O worker or CLI edge.
    pub fn backup_into(
        &self,
        lease: &WriterLease,
        expected_generation: u64,
        destination: &Self,
    ) -> Result<Inspection, Error> {
        if lease.root != self.root || destination.root == self.root {
            return Err(Error::Config);
        }
        let account = lease.account;
        let (mut source, _) = self.connect_read_only(account)?;
        let tx = source.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let before = Self::snapshot_in(&tx, account)?;
        if before.generation != expected_generation {
            return Err(Error::Conflict);
        }
        let path = destination.path(account);
        let reserved = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Config
                } else {
                    Error::Storage
                }
            })?;
        drop(reserved);
        let copied = (|| {
            let mut target = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            Self::limit(&target)?;
            {
                let backup = Backup::new(&tx, &mut target)?;
                let mut done = false;
                // Even 512-byte source pages fit the 64 MiB adapter bound
                // within 1024 steps. Refuse unexpected non-progress instead
                // of allowing a tool operation to spin without a bound.
                for _ in 0..4096 {
                    match backup.step(128)? {
                        StepResult::Done => {
                            done = true;
                            break;
                        }
                        StepResult::More => (),
                        StepResult::Busy | StepResult::Locked => return Err(Error::Busy),
                        _ => return Err(Error::Storage),
                    }
                }
                if !done {
                    return Err(Error::Bounds);
                }
            }
            target.execute_batch("PRAGMA synchronous=EXTRA;")?;
            let mode: String = target.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
            if mode != "delete" {
                return Err(Error::Storage);
            }
            Self::check(&target, account)?;
            drop(target);
            File::options()
                .write(true)
                .open(&path)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Storage)?;
            let inspection = destination.inspect(account)?;
            if inspection.snapshot != before {
                return Err(Error::Conflict);
            }
            Ok(inspection)
        })();
        if tx.commit().is_err() {
            let _ = std::fs::remove_file(&path);
            return Err(Error::Storage);
        }
        if copied.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        copied
    }
}
