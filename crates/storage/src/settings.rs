// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Settings live in the inventory account database. The legacy file is only
//! read during explicit import; it is never written or deleted by this adapter.
mod legacy;
pub use crate::account::Document;
use crate::{
    AccountId, Error,
    account::{Kind, Repository as AccountRepository},
};
use std::path::{Path, PathBuf};
pub const MAX_BYTES: usize = Kind::Settings.max_bytes();

#[derive(Clone, Debug)]
pub struct Repository {
    account: AccountRepository,
    root: PathBuf,
}
impl Repository {
    pub fn open_owned_directory(root: &Path) -> Result<Self, Error> {
        Ok(Self {
            account: AccountRepository::open_owned_directory(root)?,
            root: root.canonicalize().map_err(|_| Error::Storage)?,
        })
    }
    pub fn path(&self, account: AccountId) -> PathBuf {
        self.account.path(account)
    }
    pub fn prepare(&self, account: AccountId) -> Result<(), Error> {
        self.account.prepare(account)
    }
    pub fn read(&self, account: AccountId) -> Result<Option<Document>, Error> {
        self.account.read(account, Kind::Settings)
    }
    pub fn read_legacy(&self, account: AccountId) -> Result<Option<Document>, Error> {
        legacy::read(&self.root, account)
    }
    /// Caller validates the domain document before importing it. Existing state
    /// wins over the legacy file or any seed on every later start.
    pub fn initialize(&self, account: AccountId, document: Document) -> Result<(), Error> {
        self.account
            .initialize(account, &[(Kind::Settings, document)])
    }
    pub fn compare_exchange(
        &self,
        account: AccountId,
        expected: Option<u64>,
        bytes: &[u8],
    ) -> Result<bool, Error> {
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::Bounds);
        }
        self.prepare(account)?;
        self.account
            .compare_exchange(account, Kind::Settings, expected, bytes)
    }
}
