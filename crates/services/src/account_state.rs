// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validated account-state import and reopening. These are owned domain values,
//! not protocol templates. Run all methods on a blocking worker at async edges.
use crate::{authentication::Identity, entitlements, kickback, speedwall, user_settings};
use nfs_storage::{
    AccountId,
    account::{Document, Kind, Repository},
};
use serde_json::Value;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Storage,
    Missing(Kind),
    Invalid(Kind),
    TooLarge(Kind),
}

/// Optional one-time seeds. Database values take precedence without opening a
/// seed file. Legacy settings SQLite takes precedence over legacy JSON/content.
#[derive(Clone, Default)]
pub struct Imports {
    pub identity: Option<PathBuf>,
    pub entitlements: Option<PathBuf>,
    pub kickback: Option<PathBuf>,
    pub speedwall: Option<PathBuf>,
    pub settings: Option<PathBuf>,
}
impl Imports {
    fn path(&self, kind: Kind) -> Option<&Path> {
        match kind {
            Kind::Identity => &self.identity,
            Kind::Entitlements => &self.entitlements,
            Kind::Kickback => &self.kickback,
            Kind::Speedwall => &self.speedwall,
            Kind::Settings => &self.settings,
        }
        .as_deref()
    }
}

pub struct Loaded {
    pub identity: Identity,
    pub entitlements: entitlements::State,
    pub kickback: kickback::State,
    pub speedwall: speedwall::State,
    pub settings: user_settings::Store,
}
fn json(kind: Kind, bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > kind.max_bytes() {
        return Err(Error::TooLarge(kind));
    }
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid(kind))
}
fn identity(value: &Value, account: AccountId) -> Result<Identity, Error> {
    let identity = Identity::from_json(value).map_err(|_| Error::Invalid(Kind::Identity))?;
    if identity.storage_account() != account {
        return Err(Error::Invalid(Kind::Identity));
    }
    Ok(identity)
}
pub(crate) fn validate(kind: Kind, value: &Value, account: AccountId) -> Result<(), Error> {
    let valid = match kind {
        Kind::Identity => identity(value, account).is_ok(),
        Kind::Entitlements => entitlements::State::from_json(value, account).is_ok(),
        Kind::Kickback => kickback::State::from_json(value, account).is_ok(),
        Kind::Speedwall => speedwall::State::from_json(value, account).is_ok(),
        Kind::Settings => user_settings::Settings::from_json(value).is_ok(),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid(kind))
    }
}
fn file(path: &Path, kind: Kind) -> Result<Document, Error> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| Error::Storage)?
        .take((kind.max_bytes() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > kind.max_bytes() {
        return Err(Error::TooLarge(kind));
    }
    Ok(Document { revision: 1, bytes })
}

/// Validate every candidate before importing any domain. A retry fills only
/// absent tables and never replaces existing state. Stop all older server
/// processes before importing: the legacy settings database remains untouched
/// for backup, but later writes by an old server cannot update the new store.
pub fn open(root: &Path, account: AccountId, imports: &Imports) -> Result<Loaded, Error> {
    let repo = Repository::open_owned_directory(root).map_err(|_| Error::Storage)?;
    repo.prepare(account).map_err(|_| Error::Storage)?;
    let legacy = nfs_storage::settings::Repository::open_owned_directory(root)
        .map_err(|_| Error::Storage)?;
    let mut documents = Vec::new();
    for kind in Kind::ALL {
        let document = match repo.read(account, kind).map_err(|_| Error::Storage)? {
            Some(document) => document,
            None => {
                let legacy_document = if kind == Kind::Settings {
                    legacy.read_legacy(account).map_err(|_| Error::Storage)?
                } else {
                    None
                };
                let document = match legacy_document {
                    Some(document) => document,
                    None => {
                        let legacy_json =
                            root.join(format!("user-settings-{}.json", account.hex()));
                        let path = if kind == Kind::Settings && legacy_json.exists() {
                            legacy_json.as_path()
                        } else {
                            imports.path(kind).ok_or(Error::Missing(kind))?
                        };
                        file(path, kind)?
                    }
                };
                documents.push((kind, document.clone()));
                document
            }
        };
        validate(kind, &json(kind, &document.bytes)?, account)?;
    }
    repo.initialize(account, &documents)
        .map_err(|_| Error::Storage)?;
    // Reload the winning values after any simultaneous initialization. The
    // database, not this process's seed, is the source of truth.
    let value = |kind| -> Result<Value, Error> {
        let document = repo
            .read(account, kind)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Missing(kind))?;
        json(kind, &document.bytes)
    };
    Ok(Loaded {
        identity: identity(&value(Kind::Identity)?, account)?,
        entitlements: entitlements::State::from_json(&value(Kind::Entitlements)?, account)
            .map_err(|_| Error::Invalid(Kind::Entitlements))?,
        kickback: kickback::State::from_json(&value(Kind::Kickback)?, account)
            .map_err(|_| Error::Invalid(Kind::Kickback))?,
        speedwall: speedwall::State::from_json(&value(Kind::Speedwall)?, account)
            .map_err(|_| Error::Invalid(Kind::Speedwall))?,
        settings: user_settings::Store::open(
            user_settings::Settings::default(),
            Some((root.to_path_buf(), account)),
        )
        .map_err(|_| Error::Storage)?,
    })
}
