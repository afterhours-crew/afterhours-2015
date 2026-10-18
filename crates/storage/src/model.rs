// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned inventory model for one local account.
//!
//! Owner is an eight-byte parent item reference, separate from account identity.
//! State and the class-specific body remain opaque to storage; the caller's
//! domain and protocol layers are responsible for their meaning and type.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Items per account, bounded by local policy.
pub const MAX_ITEMS: usize = 8192;
/// Nested references per list.
pub const MAX_NESTED: usize = 256;
/// Opaque derived bytes per item, bounded by local policy.
pub const MAX_DERIVED: usize = 4096;
/// Operations per batch.
pub const MAX_OPS: usize = 1024;
/// Applied batch ids retained for replay detection.
pub const MAX_BATCH_HISTORY: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Local configuration or stored layout is unusable. Never a wire value.
    Config,
    /// A policy bound was exceeded.
    Bounds,
    /// The batch is malformed: zero id, no operations, repeated targets,
    /// self references or duplicate references.
    Invalid,
    /// `expected_generation` differs from the stored generation.
    Conflict,
    UnknownItem,
    DuplicateItem,
    DanglingReference,
    /// The account store was never created.
    Absent,
    /// Unsupported schema version or application identity.
    Version,
    /// The store belongs to another account.
    Identity,
    Storage,
    Busy,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Stable local account key from owned configuration. Wire persona, user id
/// and correlation values never select a store.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId([u8; 16]);
impl AccountId {
    pub fn from_owned_config(bytes: [u8; 16]) -> Result<Self, Error> {
        if bytes == [0; 16] {
            return Err(Error::Config);
        }
        Ok(Self(bytes))
    }
    pub fn bytes(self) -> [u8; 16] {
        self.0
    }
    pub fn hex(self) -> String {
        use fmt::Write;
        let mut s = String::with_capacity(32);
        for b in self.0 {
            write!(s, "{b:02x}").expect("String formatting");
        }
        s
    }
}
impl fmt::Debug for AccountId {
    /// Logs show a short prefix only.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

/// Eight-byte item reference id. Local policy reserves zero for no owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(u64);
impl ItemId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub const fn to_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
    pub const fn from_bytes(bytes: [u8; 8]) -> Option<Self> {
        Self::new(u64::from_be_bytes(bytes))
    }
}

/// 16-byte definition GUID. The caller resolves the definition class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefinitionGuid(pub [u8; 16]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRecord {
    pub id: ItemId,
    pub definition: DefinitionGuid,
    /// Parent item in this account's graph; `None` is wire value zero.
    pub owner: Option<ItemId>,
    pub state: u8,
    pub buy_price: u32,
    pub sell_price: u32,
    pub default_items: Vec<ItemId>,
    pub sub_items: Vec<ItemId>,
    /// Class-specific serialized body, opaque to storage.
    pub derived: Vec<u8>,
}
impl ItemRecord {
    pub fn validate(&self) -> Result<(), Error> {
        if self.derived.len() > MAX_DERIVED
            || self.default_items.len() > MAX_NESTED
            || self.sub_items.len() > MAX_NESTED
        {
            return Err(Error::Bounds);
        }
        if self.owner == Some(self.id) {
            return Err(Error::Invalid);
        }
        for list in [&self.default_items, &self.sub_items] {
            let mut seen = BTreeSet::new();
            for id in list {
                if *id == self.id || !seen.insert(*id) {
                    return Err(Error::Invalid);
                }
            }
        }
        Ok(())
    }
    pub fn references(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.default_items
            .iter()
            .chain(&self.sub_items)
            .copied()
            .chain(self.owner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Insert(ItemRecord),
    Update(ItemRecord),
    Remove(ItemId),
    /// Replace the five ordered garage slots in the same transaction as items.
    SetGarage(GarageSlots),
    /// Replace one bounded table, including an intentionally empty table.
    SetTable(u32, crate::tables::Table),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Item(ItemId),
    Garage,
    Table(u32),
}
impl Op {
    pub fn target(&self) -> Target {
        match self {
            Self::Insert(r) | Self::Update(r) => Target::Item(r.id),
            Self::Remove(id) => Target::Item(*id),
            Self::SetGarage(_) => Target::Garage,
            Self::SetTable(id, _) => Target::Table(*id),
        }
    }
}

/// One transactional change. `id` is the caller's retry key: a batch id the
/// store has already applied is answered again without a second mutation.
/// `expected_generation` is the generation the caller based the change on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: u64,
    pub expected_generation: u64,
    pub ops: Vec<Op>,
}
impl Batch {
    pub fn validate(&self) -> Result<(), Error> {
        if self.id == 0 || self.ops.is_empty() {
            return Err(Error::Invalid);
        }
        if self.ops.len() > MAX_OPS {
            return Err(Error::Bounds);
        }
        let mut targets = BTreeSet::new();
        for op in &self.ops {
            if !targets.insert(op.target()) {
                return Err(Error::Invalid);
            }
            if let Op::Insert(r) | Op::Update(r) = op {
                r.validate()?;
            }
            if let Op::SetGarage(slots) = op {
                slots.validate()?;
            }
            if let Op::SetTable(_, table) = op {
                table.validate()?;
            }
        }
        Ok(())
    }
}

/// Milliseconds from an injected clock; must fit a signed 64-bit column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub updated_at: Timestamp,
    pub items: BTreeMap<ItemId, ItemRecord>,
    /// None is uninitialized (including migrated v1 profiles), distinct from
    /// an intentionally empty garage. Primary slot, then secondary slots 1..4.
    pub garage: Option<GarageSlots>,
    pub tables: BTreeMap<u32, crate::tables::Table>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GarageSlots(pub [Option<ItemId>; 5]);
impl GarageSlots {
    pub fn validate(self) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        for id in self.0.into_iter().flatten() {
            if !seen.insert(id) {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub fn validate_items(self, items: &BTreeMap<ItemId, ItemRecord>) -> Result<(), Error> {
        self.validate()?;
        for id in self.0.into_iter().flatten() {
            if !items.contains_key(&id) {
                return Err(Error::DanglingReference);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    pub generation: u64,
    /// The batch id had already been applied; nothing changed.
    pub replayed: bool,
}
