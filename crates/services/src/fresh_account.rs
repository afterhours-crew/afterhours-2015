// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! New local account state from injected entropy, clock and explicit content.
//! Initial item selection is local deployment policy, not the official prologue
//! award path. No profile, captured reply or remote identity is an input.
use crate::{
    SUPPORTED_BUILD_SHA256 as BUILD, account_state, authentication::Identity, entitlements,
    inventory, persistent, user_settings::Settings,
};
use nfs_storage::{
    AccountId, Batch, MemoryRepository, Op, Timestamp,
    account::{Document, Kind, Repository},
};
use nfs_world_core::items::InventoryCatalog;
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path};

pub const SEED_BYTES: usize = 40;
pub const MAX_POLICY_BYTES: usize = 512 * 1024;
// Garage, persistent tables, awards and challenges reserve MAX through MAX-3.
pub const INITIAL_BATCH: u64 = u64::MAX - 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Policy,
    Identity,
    Inventory,
    Progression,
    Bounds,
    DestinationExists,
    Storage,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fresh account {self:?}")
    }
}
impl std::error::Error for Error {}

/// Static local license definitions and screenshot limit. Grant definitions have
/// the existing named entitlement fields except `id` and `persona_id`; those are
/// assigned locally. Grant dates/counts/status are explicit operator policy.
pub struct Policy {
    entitlements: Value,
    screenshot_limit: u32,
}
fn fields(v: &Value, keys: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
}
impl Policy {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Error::Policy)?
            .take((MAX_POLICY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Policy)?;
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(Error::Bounds);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Error::Policy)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, Error> {
        if !fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "entitlements",
                "screenshot_count_max",
            ],
        ) || v["format"] != "nfs-fresh-account-policy"
            || v["version"] != 1
            || v["build_sha256"] != BUILD
            || !fields(&v["entitlements"], &["scopes", "grants"])
        {
            return Err(Error::Policy);
        }
        let policy = Self {
            entitlements: v["entitlements"].clone(),
            screenshot_limit: v["screenshot_count_max"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(Error::Policy)?,
        };
        policy.entitlements(
            AccountId::from_owned_config([1; 16]).map_err(|_| Error::Identity)?,
            1,
        )?;
        Ok(policy)
    }
    fn entitlements(&self, account: AccountId, first_id: u64) -> Result<Value, Error> {
        let definitions = self.entitlements["grants"]
            .as_array()
            .filter(|v| v.len() <= entitlements::MAX_GRANTS)
            .ok_or(Error::Policy)?;
        let mut grants = Vec::new();
        for (index, definition) in definitions.iter().enumerate() {
            let mut grant = definition
                .as_object()
                .filter(|v| v.len() == 14 && !v.contains_key("id") && !v.contains_key("persona_id"))
                .ok_or(Error::Policy)?
                .clone();
            grant.insert(
                "id".into(),
                json!(first_id.checked_add(index as u64).ok_or(Error::Bounds)?),
            );
            grant.insert("persona_id".into(), json!(0));
            grants.push(Value::Object(grant));
        }
        let document = json!({"format":"nfs-entitlement-state", "version":1, "build_sha256":BUILD, "account":account.hex(), "scopes":self.entitlements["scopes"], "grants":grants});
        entitlements::State::from_json(&document, account).map_err(|_| Error::Policy)?;
        Ok(document)
    }
}

pub struct Prepared {
    identity: Identity,
    documents: Vec<(Kind, Document)>,
    batch: Batch,
    now: Timestamp,
}
impl Prepared {
    /// Construct and validate all state in memory before touching the filesystem.
    /// Positive persona/account IDs use distinct local domains. Item IDs are
    /// account-scoped and come from the existing Items initialization policy.
    pub fn new(
        name: &str,
        seed: [u8; SEED_BYTES],
        now: Timestamp,
        policy: &Policy,
        items: &InventoryCatalog,
        tables: &persistent::Catalog,
    ) -> Result<Self, Error> {
        if name.len() > 32 {
            return Err(Error::Identity);
        }
        let storage = AccountId::from_owned_config(seed[..16].try_into().expect("fixed slice"))
            .map_err(|_| Error::Identity)?;
        let word =
            |start| u64::from_le_bytes(seed[start..start + 8].try_into().expect("fixed slice"));
        let persona = ((word(16) & 0x0fff_ffff_ffff_ffff) | 0x1000_0000_0000_0000) as i64;
        let wire_account = ((word(24) & 0x0fff_ffff_ffff_ffff) | 0x2000_0000_0000_0000) as i64;
        let first_grant = (word(32) & 0x3fff_ffff_ffff_ffff) + 1;
        let identity = Identity::new(storage, persona, wire_account, name.into())
            .map_err(|_| Error::Identity)?;
        let values = [
            (
                Kind::Identity,
                json!({"version":1,"storage_account":storage.hex(),"persona":persona,"account":wire_account,"name":name}),
            ),
            (
                Kind::Entitlements,
                policy.entitlements(storage, first_grant)?,
            ),
            (
                Kind::Kickback,
                json!({"format":"nfs-kickback-state","version":1,"build_sha256":BUILD,"account":storage.hex(),"screenshot_count":0,"screenshot_count_max":policy.screenshot_limit,"gallery":"empty","winner":null}),
            ),
            (
                Kind::Speedwall,
                json!({"format":"nfs-speedwall-state","version":1,"build_sha256":BUILD,"account":storage.hex(),"rows":[]}),
            ),
            (Kind::Settings, Settings::default().to_json()),
        ];
        let mut documents = Vec::new();
        for (kind, value) in values {
            account_state::validate(kind, &value, storage).map_err(|_| Error::Policy)?;
            let bytes = serde_json::to_vec(&value).map_err(|_| Error::Policy)?;
            if bytes.len() > kind.max_bytes() {
                return Err(Error::Bounds);
            }
            documents.push((kind, Document { revision: 1, bytes }));
        }
        let memory = MemoryRepository::new();
        let snapshot =
            inventory::load(&memory, storage, items, 1, now).map_err(|_| Error::Inventory)?;
        let (snapshot, _) = tables
            .ensure_loaded(&memory, storage, snapshot, now)
            .map_err(|_| Error::Progression)?;
        let snapshot = crate::awards::ensure_empty(&memory, storage, snapshot, now)
            .map_err(|_| Error::Progression)?;
        let snapshot = crate::challenges::ensure_empty(&memory, storage, snapshot, now)
            .map_err(|_| Error::Progression)?;
        let mut ops: Vec<_> = snapshot.items.into_values().map(Op::Insert).collect();
        ops.push(Op::SetGarage(snapshot.garage.ok_or(Error::Inventory)?));
        ops.extend(
            snapshot
                .tables
                .into_iter()
                .map(|(key, table)| Op::SetTable(key, table)),
        );
        let batch = Batch {
            id: INITIAL_BATCH,
            expected_generation: 0,
            ops,
        };
        batch.validate().map_err(|_| Error::Bounds)?;
        Ok(Self {
            identity,
            documents,
            batch,
            now,
        })
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    /// Require a new state directory. SQLite publishes account domains and
    /// inventory atomically; an allocated directory may remain after a failure.
    /// Existing directories and profiles are never reset, replaced or removed.
    pub fn publish(&self, root: &Path) -> Result<(), Error> {
        std::fs::create_dir(root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::DestinationExists
            } else {
                Error::Storage
            }
        })?;
        Repository::open_owned_directory(root)
            .map_err(|_| Error::Storage)?
            .initialize_fresh(
                self.identity.storage_account(),
                &self.documents,
                &self.batch,
                self.now,
            )
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
