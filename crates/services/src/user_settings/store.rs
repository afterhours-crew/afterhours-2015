// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Change, Error, Settings};
use nfs_storage::{AccountId, settings::Repository};
use std::{fmt, path::PathBuf, sync::Mutex};

enum Backend {
    Memory(Mutex<Settings>),
    Durable(Repository, AccountId),
}
pub struct Store {
    backend: Backend,
}
impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("settings::Store")
            .field("persisted", &self.persisted())
            .finish()
    }
}
fn encode(settings: &Settings) -> Result<Vec<u8>, Error> {
    settings.validate()?;
    serde_json::to_vec(&settings.to_json()).map_err(|_| Error::Storage)
}
fn decode(bytes: &[u8]) -> Result<Settings, Error> {
    Settings::from_json(&serde_json::from_slice(bytes).map_err(|_| Error::Config)?)
}
impl Store {
    /// Initialize from bounded local content only when no durable state exists.
    /// All filesystem/SQLite calls must run away from async workers.
    pub fn open(content: Settings, durable: Option<(PathBuf, AccountId)>) -> Result<Self, Error> {
        content.validate()?;
        let backend = match durable {
            None => Backend::Memory(Mutex::new(content)),
            Some((root, account)) => {
                let repo = Repository::open_owned_directory(&root).map_err(|_| Error::Storage)?;
                repo.prepare(account).map_err(|_| Error::Storage)?;
                if repo.read(account).map_err(|_| Error::Storage)?.is_none() {
                    let document = match repo.read_legacy(account).map_err(|_| Error::Storage)? {
                        Some(document) => {
                            decode(&document.bytes)?;
                            document
                        }
                        None => nfs_storage::settings::Document {
                            revision: 1,
                            bytes: encode(&content)?,
                        },
                    };
                    repo.initialize(account, document)
                        .map_err(|_| Error::Storage)?;
                }
                decode(
                    &repo
                        .read(account)
                        .map_err(|_| Error::Storage)?
                        .ok_or(Error::Storage)?
                        .bytes,
                )?;
                Backend::Durable(repo, account)
            }
        };
        Ok(Self { backend })
    }
    pub fn persisted(&self) -> bool {
        matches!(self.backend, Backend::Durable(..))
    }
    pub fn current(&self) -> Result<Settings, Error> {
        match &self.backend {
            Backend::Memory(state) => Ok(state.lock().map_err(|_| Error::Storage)?.clone()),
            Backend::Durable(repo, account) => decode(
                &repo
                    .read(*account)
                    .map_err(|_| Error::Storage)?
                    .ok_or(Error::Storage)?
                    .bytes,
            ),
        }
    }
    /// Atomic per-key merge against the latest account state. A conflict reloads
    /// and retries at most eight times. No success is returned on a failed save.
    pub fn apply(&self, change: &Change) -> Result<Settings, Error> {
        match &self.backend {
            Backend::Memory(state) => {
                let mut state = state.lock().map_err(|_| Error::Storage)?;
                change.apply(&mut state)?;
                Ok(state.clone())
            }
            Backend::Durable(repo, account) => {
                for _ in 0..8 {
                    let current = repo
                        .read(*account)
                        .map_err(|_| Error::Storage)?
                        .ok_or(Error::Storage)?;
                    let mut next = decode(&current.bytes)?;
                    if !change.apply(&mut next)? {
                        return Ok(next);
                    }
                    if repo
                        .compare_exchange(*account, Some(current.revision), &encode(&next)?)
                        .map_err(|_| Error::Storage)?
                    {
                        return Ok(next);
                    }
                }
                Err(Error::Conflict)
            }
        }
    }
}
