// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Durable intents derived from actual REP-grant operations. These operations
//! join the reward transaction; preparing or retrying a reward cannot publish a
//! message. Timing and wire attachment encoding belong to the world adapter.
use crate::{inventory, persistent::key};
use nfs_storage::{
    Batch, Error, Op, Snapshot,
    tables::{Table, Value},
};
use nfs_world_core::items::{self, Derived, InventoryCatalog};
use std::collections::{BTreeMap, BTreeSet};

pub const TABLE: &str = "AfterhoursUnlockMessages";
const MAX_ROWS: usize = 64;
type Guid = [u8; 16];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Trigger {
    Categories { mask: u32 },
    Part { include: bool, ids: BTreeSet<u32> },
}

#[derive(Clone, Debug)]
pub struct Policy {
    controller: Option<Guid>,
    parts: BTreeMap<Guid, u32>,
    rules: BTreeMap<u32, Trigger>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pending {
    pub hash: u32,
    /// Committed reward generation that last started/replaced this delay.
    /// Property-only changes retain this value, so they do not reset the timer.
    pub signal: u64,
    pub part: Option<u32>,
}

impl Policy {
    /// Bind the projection to the policies that actually produce REP grants.
    /// Missing attachment identities must fail startup, not silently omit a toast.
    pub fn validate_grants(
        &self,
        categories: Option<&crate::category_unlocks::Policy>,
        parts: Option<&crate::item_unlocks::Policy>,
    ) -> Result<(), Error> {
        if self.controller != categories.map(|p| p.controller)
            || parts.is_none_or(|p| p.requirements.keys().any(|g| !self.parts.contains_key(g)))
        {
            return Err(Error::Config);
        }
        Ok(())
    }

    pub fn new(
        controller: Option<Guid>,
        parts: BTreeMap<Guid, u32>,
        rules: BTreeMap<u32, Trigger>,
    ) -> Result<Self, Error> {
        if rules.is_empty()
            || rules.len() > MAX_ROWS
            || rules.contains_key(&0)
            || parts.len() > 4096
            || parts.keys().any(|g| *g == [0; 16])
            || parts.values().any(|id| *id == 0)
            || parts.values().collect::<BTreeSet<_>>().len() != parts.len()
            || controller == Some([0; 16])
        {
            return Err(Error::Config);
        }
        for rule in rules.values() {
            match rule {
                Trigger::Categories { mask } if *mask == 0 || controller.is_none() => {
                    return Err(Error::Config);
                }
                Trigger::Part { ids, .. }
                    if parts.is_empty() || ids.len() > 4096 || ids.contains(&0) =>
                {
                    return Err(Error::Config);
                }
                _ => (),
            }
        }
        Ok(Self {
            controller,
            parts,
            rules,
        })
    }

    pub fn pending(&self, table: Option<&Table>) -> Result<Vec<Pending>, Error> {
        let pending = pending(table)?;
        for row in &pending {
            if !matches!(
                (self.rules.get(&row.hash), row.part),
                (Some(Trigger::Categories { .. }), None) | (Some(Trigger::Part { .. }), Some(_))
            ) || row
                .part
                .is_some_and(|id| !self.parts.values().any(|v| *v == id))
            {
                return Err(Error::Config);
            }
        }
        Ok(pending)
    }

    /// Evaluate only changes actually proposed by the authoritative grant policy.
    /// The order is the item-operation order, preserving the listener's property
    /// updates even when its count gate or the sender's filter suppresses an event.
    pub fn operations(
        &self,
        snapshot: &Snapshot,
        catalog: &InventoryCatalog,
        grants: &Batch,
    ) -> Result<Vec<Op>, inventory::Error> {
        if grants.expected_generation != snapshot.generation {
            return Err(Error::Conflict.into());
        }
        let old = snapshot.tables.get(&key(TABLE));
        let mut rows: BTreeMap<_, _> = self
            .pending(old)?
            .into_iter()
            .map(|p| (p.hash, p))
            .collect();
        let signal = snapshot.generation.checked_add(1).ok_or(Error::Bounds)?;
        let mut after = snapshot.clone();
        after.items = nfs_storage::port::transition(&snapshot.items, grants)?;
        if self
            .rules
            .values()
            .any(|r| matches!(r, Trigger::Categories { .. }))
        {
            let before_mask = self.mask(snapshot, catalog)?;
            let after_mask = self.mask(&after, catalog)?;
            for (&hash, rule) in &self.rules {
                if let Trigger::Categories { mask } = rule
                    && before_mask & mask != *mask
                    && after_mask & mask == *mask
                {
                    rows.insert(
                        hash,
                        Pending {
                            hash,
                            signal,
                            part: None,
                        },
                    );
                }
            }
        }
        let mut items = snapshot.items.clone();
        for op in &grants.ops {
            let record = match op {
                Op::Insert(record) | Op::Update(record) => record,
                Op::Remove(id) => {
                    items.remove(id);
                    continue;
                }
                _ => continue,
            };
            let previous = items.insert(record.id, record.clone());
            if record.owner.is_some()
                || record.state != items::PURCHASABLE
                || previous.is_some_and(|p| {
                    p.definition == record.definition
                        && p.owner.is_none()
                        && p.state == items::PURCHASABLE
                })
            {
                continue;
            }
            let Some(&id) = self.parts.get(&record.definition.0) else {
                continue;
            };
            let first = items
                .values()
                .filter(|p| {
                    p.definition == record.definition
                        && matches!(p.state, items::OWNED | items::PURCHASABLE)
                })
                .count()
                == 1;
            for (&hash, rule) in &self.rules {
                if let Trigger::Part { include, ids } = rule {
                    // Input properties update before the listener's first-item gate.
                    if let Some(pending) = rows.get_mut(&hash) {
                        pending.part = Some(id);
                    }
                    if first && ids.contains(&id) == *include {
                        rows.insert(
                            hash,
                            Pending {
                                hash,
                                signal,
                                part: Some(id),
                            },
                        );
                    }
                }
            }
        }
        let next = store(rows.values().copied())?;
        if old == Some(&next) || (old.is_none() && next.rows.is_empty()) {
            Ok(vec![])
        } else {
            Ok(vec![Op::SetTable(key(TABLE), next)])
        }
    }

    fn mask(
        &self,
        snapshot: &Snapshot,
        catalog: &InventoryCatalog,
    ) -> Result<u32, inventory::Error> {
        let collection = inventory::collection(snapshot, catalog.bindings())?;
        let mut matching = collection
            .items
            .values()
            .filter(|i| Some(i.definition) == self.controller);
        let item = matching.next().ok_or(Error::Config)?;
        if matching.next().is_some()
            || item.owner != 0
            || item.state != items::OWNED
            || !item.children.is_empty()
        {
            return Err(Error::Config.into());
        }
        match item.derived {
            Derived::CategoryUnlockController { unlock_mask } => Ok(unlock_mask),
            _ => Err(Error::Config.into()),
        }
    }
}

/// Decode the private versioned table; no pending row is silently discarded.
pub fn pending(table: Option<&Table>) -> Result<Vec<Pending>, Error> {
    let Some(table) = table else {
        return Ok(vec![]);
    };
    table.validate()?;
    if table.rows.len() > MAX_ROWS {
        return Err(Error::Bounds);
    }
    table
        .rows
        .iter()
        .map(|(&hash, row)| {
            let hash = u32::try_from(hash).map_err(|_| Error::Invalid)?;
            let (
                Some(Value::Int(1)),
                Some(Value::String(signal)),
                Some(Value::Int(kind)),
                Some(Value::Int(part)),
            ) = (
                row.get(&key("Version")),
                row.get(&key("Signal")),
                row.get(&key("Kind")),
                row.get(&key("Part")),
            )
            else {
                return Err(Error::Invalid);
            };
            let number: u64 = signal.parse().map_err(|_| Error::Invalid)?;
            if hash == 0 || row.len() != 4 || number == 0 || number.to_string() != *signal {
                return Err(Error::Invalid);
            }
            let part = match (*kind, *part) {
                (0, 0) => None,
                (1, p) if p != 0 => Some(p as u32),
                _ => return Err(Error::Invalid),
            };
            Ok(Pending {
                hash,
                signal: number,
                part,
            })
        })
        .collect()
}

fn store(rows: impl IntoIterator<Item = Pending>) -> Result<Table, Error> {
    let table = Table {
        rows: rows
            .into_iter()
            .map(|p| {
                (
                    u64::from(p.hash),
                    [
                        (key("Version"), Value::Int(1)),
                        (key("Signal"), Value::String(p.signal.to_string())),
                        (key("Kind"), Value::Int(i32::from(p.part.is_some()))),
                        (key("Part"), Value::Int(p.part.unwrap_or(0) as i32)),
                    ]
                    .into(),
                )
            })
            .collect(),
    };
    pending(Some(&table))?;
    Ok(table)
}

/// Atomically move one still-current intent into the inbox. A stale worker may
/// not erase a replacement or commit an attachment sampled from an older state.
/// Callers encode the current part with the canonical world codec and re-read
/// the snapshot after any conflict/replayed transaction before publishing.
pub fn deliver(
    snapshot: &Snapshot,
    expected: Pending,
    static_table: &str,
    capacity: u16,
    attachment: &[u8],
) -> Result<Option<Batch>, Error> {
    let mut rows = pending(snapshot.tables.get(&key(TABLE)))?;
    let Some(index) = rows.iter().position(|p| *p == expected) else {
        return Ok(None);
    };
    if ![super::STATIC, super::UNDERGROUND].contains(&static_table) {
        return Err(Error::Config);
    }
    let mut ops = match expected.part {
        Some(_) => super::dynamic::append_owned(snapshot, capacity, expected.hash, attachment)?.0,
        None => {
            if !attachment.is_empty() {
                return Err(Error::Invalid);
            }
            super::append(snapshot, static_table, expected.hash)?.map_or_else(Vec::new, |b| b.ops)
        }
    };
    rows.remove(index);
    ops.push(Op::SetTable(key(TABLE), store(rows)?));
    if snapshot.generation >= super::NAMESPACE {
        return Err(Error::Conflict);
    }
    let batch = Batch {
        id: super::NAMESPACE | snapshot.generation,
        expected_generation: snapshot.generation,
        ops,
    };
    batch.validate()?;
    Ok(Some(batch))
}

#[cfg(test)]
mod tests;
