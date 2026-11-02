// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Repository port and the pure transition every adapter applies.
use crate::AccountId;
use crate::model::{Applied, Batch, Error, ItemId, ItemRecord, MAX_ITEMS, Op, Snapshot, Timestamp};
use std::collections::BTreeMap;

/// Durable per-account inventory. Implementations are synchronous; callers on
/// async workers must move calls off the executor. Every method is safe to
/// repeat: `open` is idempotent, `snapshot` is read-only and `apply` answers a
/// repeated batch id from history instead of mutating twice.
pub trait InventoryRepository: Send + Sync {
    /// Creates the account's empty store when absent, verifies ownership and
    /// schema when present, and returns the current state.
    fn open(&self, account: AccountId) -> Result<Snapshot, Error>;
    /// Current state; `Error::Absent` before `open`.
    fn snapshot(&self, account: AccountId) -> Result<Snapshot, Error>;
    /// Read a retained commit receipt before rebuilding a retry against newer
    /// state. No account is created or changed. `None` means unknown, including
    /// an evicted receipt; it is not proof that the operation never ran.
    /// `apply` still checks the retry key atomically with the commit.
    fn receipt(&self, account: AccountId, batch_id: u64) -> Result<Option<Applied>, Error>;
    /// Applies one batch atomically at `now`, or replays its recorded result.
    fn apply(&self, account: AccountId, batch: &Batch, now: Timestamp) -> Result<Applied, Error>;
}

/// What an adapter must do for a batch given the stored generation and the
/// generation recorded for this batch id, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Replay(Applied),
    Apply { next_generation: u64 },
}

/// Shared acceptance order: shape, timestamp range, replay, then generation.
pub fn decide(
    generation: u64,
    recorded: Option<u64>,
    batch: &Batch,
    now: Timestamp,
) -> Result<Decision, Error> {
    batch.validate()?;
    if i64::try_from(now.0).is_err() {
        return Err(Error::Bounds);
    }
    if let Some(generation) = recorded {
        return Ok(Decision::Replay(Applied {
            generation,
            replayed: true,
        }));
    }
    if batch.expected_generation != generation {
        return Err(Error::Conflict);
    }
    let next_generation = generation
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
        .ok_or(Error::Bounds)?;
    Ok(Decision::Apply { next_generation })
}

/// Applies the operations to a copy of `items` and checks the result:
/// inserts need a fresh id, updates and removals an existing one, the item
/// count stays within `MAX_ITEMS`, and every default/sub-item reference
/// resolves after the batch. Errors leave `items` untouched.
pub fn transition(
    items: &BTreeMap<ItemId, ItemRecord>,
    batch: &Batch,
) -> Result<BTreeMap<ItemId, ItemRecord>, Error> {
    batch.validate()?;
    let mut next = items.clone();
    for op in &batch.ops {
        match op {
            Op::Insert(record) => {
                if next.contains_key(&record.id) {
                    return Err(Error::DuplicateItem);
                }
                next.insert(record.id, record.clone());
            }
            Op::Update(record) => {
                if !next.contains_key(&record.id) {
                    return Err(Error::UnknownItem);
                }
                next.insert(record.id, record.clone());
            }
            Op::Remove(id) => {
                if next.remove(id).is_none() {
                    return Err(Error::UnknownItem);
                }
            }
            Op::SetGarage(_) | Op::SetTable(_, _) => {}
        }
    }
    if next.len() > MAX_ITEMS {
        return Err(Error::Bounds);
    }
    for record in next.values() {
        if record.references().any(|id| !next.contains_key(&id)) {
            return Err(Error::DanglingReference);
        }
    }
    Ok(next)
}

/// Check item and garage references against the final transaction state,
/// allowing an item deletion and slot clearing in either operation order.
pub fn transition_garage(
    previous: Option<crate::GarageSlots>,
    items: &BTreeMap<ItemId, ItemRecord>,
    batch: &Batch,
) -> Result<Option<crate::GarageSlots>, Error> {
    let mut garage = previous;
    for op in &batch.ops {
        if let Op::SetGarage(slots) = op {
            garage = Some(*slots);
        }
    }
    if let Some(slots) = garage {
        slots.validate_items(items)?;
    }
    Ok(garage)
}

/// Validates a stored record set as an adapter reads it back. Stored data
/// that fails the model's own rules is `Error::Config`, not a caller error.
pub fn check_stored(items: &BTreeMap<ItemId, ItemRecord>) -> Result<(), Error> {
    if items.len() > MAX_ITEMS {
        return Err(Error::Config);
    }
    for (id, record) in items {
        if *id != record.id || record.validate().is_err() {
            return Err(Error::Config);
        }
        if record.references().any(|r| !items.contains_key(&r)) {
            return Err(Error::Config);
        }
    }
    Ok(())
}

/// Convenience for adapters building a snapshot.
pub fn snapshot(
    generation: u64,
    updated_at: Timestamp,
    items: BTreeMap<ItemId, ItemRecord>,
) -> Snapshot {
    Snapshot {
        generation,
        updated_at,
        items,
        garage: None,
        tables: BTreeMap::new(),
    }
}
