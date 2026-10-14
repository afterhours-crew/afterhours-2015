// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Items File payloads, not a captured-response player.
//! Every first reference carries an owned record; later references carry only
//! its id. The native cache inserts after the entire record is read/written.
//! Definition classes select the derived codec. Unknown classes cannot safely
//! be skipped because individual records have no length prefix.
use std::collections::{BTreeMap, BTreeSet};

mod derived;
mod dyno;
mod inventory;
mod purchase;
mod update;
pub use derived::{DefinitionClass, Derived, Layout};
pub use dyno::DynoStats;
pub use inventory::{
    Definition, DefinitionFlags, InitialInventory, InventoryCatalog, OwnershipLevel,
};
pub use purchase::{PurchaseReply, PurchaseRequest, PurchaseSelection};
pub use update::UpdateNotification;

/// Owned and Purchasable construction paths, respectively.
pub const OWNED: u8 = 2;
pub const PURCHASABLE: u8 = 4;
pub const LOAD_INVENTORY: u8 = 0x10;
pub const PURCHASE_ITEMS: u8 = 0x02;

mod handling;
pub use handling::HandlingChanges;

impl Derived {
    /// Exact typed suffix codec, also used at the persistence boundary.
    pub fn decode(layout: Layout, bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let derived = Self::read(layout, &mut reader)?;
        reader.finish()?;
        Ok(derived)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::default();
        self.write(&mut writer)?;
        Ok(writer.0)
    }
}

/// Local resource policies, not recovered native maxima.
pub const MAX_BYTES: usize = super::files::MAX_BYTES;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_COLLECTION: usize = 4096;
pub const MAX_REFERENCES: usize = 32768;
pub const MAX_DEPTH: usize = 32;
pub const MAX_WORDS: usize = 4096;
pub const MAX_DEFINITIONS: usize = 16384;
pub type Guid = [u8; 16];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Trailing,
    Bound,
    Shape,
    UnknownDefinition,
    MissingItem,
    Cycle,
    UnsupportedState,
    UnsupportedDefault,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Items {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct Catalog(BTreeMap<Guid, DefinitionClass>);
impl Catalog {
    pub fn new(
        definitions: impl IntoIterator<Item = (Guid, DefinitionClass)>,
    ) -> Result<Self, Error> {
        let mut entries = BTreeMap::new();
        for (guid, class) in definitions {
            if entries.len() >= MAX_DEFINITIONS {
                return Err(Error::Bound);
            }
            if entries.insert(guid, class).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(Self(entries))
    }
    pub fn class(&self, guid: &Guid) -> Result<DefinitionClass, Error> {
        self.0.get(guid).copied().ok_or(Error::UnknownDefinition)
    }
    pub fn definitions(&self) -> impl Iterator<Item = (&Guid, DefinitionClass)> {
        self.0.iter().map(|(guid, class)| (guid, *class))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field("definitions", &self.len())
            .finish()
    }
}

/// The common record. Ids are wire values, not accounts. Owner is zero for a
/// root in; nonzero values point at parent items. Storage applies its own
/// graph invariants. Numeric state/price/derived words retain their exact bits.
#[derive(Clone, Eq, PartialEq)]
pub struct Item {
    pub id: u64,
    pub definition: Guid,
    pub owner: u64,
    pub defaults: Vec<u64>,
    pub children: Vec<u64>,
    pub state: u8,
    pub buy_price: u32,
    pub sell_price: u32,
    pub derived: Derived,
}
impl Item {
    /// Library liveries are account-owned leaves. Installing one retains its
    /// ID and zero owner; ordinary vehicle parts must belong to their parent.
    pub fn is_account_livery(&self) -> bool {
        self.owner == 0
            && self.state == OWNED
            && self.defaults.is_empty()
            && self.children.is_empty()
            && matches!(self.derived, Derived::LiveryCustomization { .. })
    }

    pub fn belongs_under(&self, parent: u64) -> bool {
        self.owner == parent || (parent != 0 && self.is_account_livery())
    }
}
impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("layout", &self.derived.layout())
            .field("defaults", &self.defaults.len())
            .field("children", &self.children.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct Collection {
    pub roots: Vec<u64>,
    pub items: BTreeMap<u64, Item>,
}
impl std::fmt::Debug for Collection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collection")
            .field("roots", &self.roots.len())
            .field("items", &self.items.len())
            .finish()
    }
}

/// Five-byte request envelope. binds 0x10 to LoadInventoryOperation;
/// other operation bytes are preserved, not implemented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub sequence: u32,
    pub operation: u8,
}
impl Request {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let request = Self {
            sequence: reader.u32()?,
            operation: reader.u8()?,
        };
        reader.finish()?;
        Ok(request)
    }
    pub fn encode(self) -> [u8; 5] {
        let mut bytes = [0; 5];
        bytes[..4].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[4] = self.operation;
        bytes
    }
}

/// Length-delimited response header. Unknown response states retain their body
/// so callers can report them unsupported without inventing a schema.
#[derive(Clone, Eq, PartialEq)]
pub struct Envelope {
    pub sequence: u32,
    pub state: u8,
    pub body: Vec<u8>,
}
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemsEnvelope")
            .field("sequence", &self.sequence)
            .field("state", &self.state)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
impl Envelope {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let sequence = reader.u32()?;
        let state = reader.u8()?;
        let length = reader.u32()? as usize;
        let body = reader.take(length)?.to_vec();
        reader.finish()?;
        Ok(Self {
            sequence,
            state,
            body,
        })
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::default();
        writer.u32(self.sequence)?;
        writer.bytes(&[self.state])?;
        writer.u32(u32::try_from(self.body.len()).map_err(|_| Error::Bound)?)?;
        writer.bytes(&self.body)?;
        Ok(writer.0)
    }
    pub fn collection(&self, catalog: &Catalog) -> Result<Collection, Error> {
        if self.state != 2 {
            return Err(Error::UnsupportedState);
        }
        Collection::decode(&self.body, catalog)
    }
    /// Build state 2 from supplied owned data. This chooses no starting profile,
    /// state transition, sequence or inventory defaults on the caller's behalf.
    pub fn from_collection(
        sequence: u32,
        collection: &Collection,
        catalog: &Catalog,
    ) -> Result<Self, Error> {
        let body = collection.encode(catalog)?;
        if body.len() > MAX_BYTES - 9 {
            return Err(Error::Bound);
        }
        Ok(Self {
            sequence,
            state: 2,
            body,
        })
    }
}

impl Collection {
    /// Select complete reachable records for a notification, preserving root order.
    /// Encoding still validates cycles, definitions and all graph resource bounds.
    pub fn for_roots(&self, roots: Vec<u64>) -> Result<Self, Error> {
        if roots.len() > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let mut pending = roots.clone();
        let mut items = BTreeMap::new();
        let mut references = roots.len();
        while let Some(id) = pending.pop() {
            if items.contains_key(&id) {
                continue;
            }
            if items.len() >= MAX_ITEMS {
                return Err(Error::Bound);
            }
            let item = self.items.get(&id).ok_or(Error::MissingItem)?;
            references = references
                .checked_add(item.defaults.len())
                .and_then(|count| count.checked_add(item.children.len()))
                .filter(|&count| count <= MAX_REFERENCES)
                .ok_or(Error::Bound)?;
            if item.defaults.len() > MAX_COLLECTION || item.children.len() > MAX_COLLECTION {
                return Err(Error::Bound);
            }
            pending.extend(&item.defaults);
            pending.extend(&item.children);
            items.insert(id, item.clone());
        }
        Ok(Self { roots, items })
    }

    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        let mut decoder = Decoder {
            reader: Reader::new(bytes)?,
            catalog,
            items: BTreeMap::new(),
            active: BTreeSet::new(),
            references: 0,
        };
        let roots = decoder.collection(0)?;
        decoder.reader.finish()?;
        Ok(Self {
            roots,
            items: decoder.items,
        })
    }
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        if self.items.len() > MAX_ITEMS {
            return Err(Error::Bound);
        }
        let mut encoder = Encoder {
            writer: Writer::default(),
            catalog,
            items: &self.items,
            seen: BTreeSet::new(),
            active: BTreeSet::new(),
            references: 0,
        };
        encoder.collection(&self.roots, 0)?;
        if encoder.seen.len() != self.items.len() {
            return Err(Error::Shape);
        }
        Ok(encoder.writer.0)
    }
}

struct Decoder<'a> {
    reader: Reader<'a>,
    catalog: &'a Catalog,
    items: BTreeMap<u64, Item>,
    active: BTreeSet<u64>,
    references: usize,
}
impl Decoder<'_> {
    fn collection(&mut self, depth: usize) -> Result<Vec<u64>, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Bound);
        }
        let count = self.reader.u32()? as usize;
        if count > MAX_COLLECTION || count > self.reader.remaining() / 8 {
            return Err(Error::Bound);
        }
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            ids.push(self.item(depth)?);
        }
        Ok(ids)
    }
    fn item(&mut self, depth: usize) -> Result<u64, Error> {
        self.references += 1;
        if self.references > MAX_REFERENCES {
            return Err(Error::Bound);
        }
        let id = self.reader.u64()?;
        if self.items.contains_key(&id) {
            return Ok(id);
        }
        if self.active.contains(&id) {
            return Err(Error::Cycle);
        }
        if self.items.len() + self.active.len() >= MAX_ITEMS {
            return Err(Error::Bound);
        }
        self.active.insert(id);
        let definition = self.reader.fixed()?;
        let layout = self.catalog.class(&definition)?.layout();
        if self.reader.u64()? != id {
            return Err(Error::Shape);
        }
        let owner = self.reader.u64()?;
        let defaults = self.collection(depth + 1)?;
        let children = self.collection(depth + 1)?;
        let state = self.reader.u8()?;
        let buy_price = self.reader.u32()?;
        let sell_price = self.reader.u32()?;
        let derived = Derived::read(layout, &mut self.reader)?;
        self.items.insert(
            id,
            Item {
                id,
                definition,
                owner,
                defaults,
                children,
                state,
                buy_price,
                sell_price,
                derived,
            },
        );
        self.active.remove(&id);
        Ok(id)
    }
}

struct Encoder<'a> {
    writer: Writer,
    catalog: &'a Catalog,
    items: &'a BTreeMap<u64, Item>,
    seen: BTreeSet<u64>,
    active: BTreeSet<u64>,
    references: usize,
}
impl Encoder<'_> {
    fn collection(&mut self, ids: &[u64], depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH || ids.len() > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        self.writer.u32(ids.len() as u32)?;
        for &id in ids {
            self.item(id, depth)?;
        }
        Ok(())
    }
    fn item(&mut self, id: u64, depth: usize) -> Result<(), Error> {
        self.references += 1;
        if self.references > MAX_REFERENCES {
            return Err(Error::Bound);
        }
        self.writer.u64(id)?;
        if self.seen.contains(&id) {
            return Ok(());
        }
        if !self.active.insert(id) {
            return Err(Error::Cycle);
        }
        let item = self.items.get(&id).ok_or(Error::MissingItem)?;
        if item.id != id || self.catalog.class(&item.definition)?.layout() != item.derived.layout()
        {
            return Err(Error::Shape);
        }
        self.writer.bytes(&item.definition)?;
        self.writer.u64(id)?;
        self.writer.u64(item.owner)?;
        self.collection(&item.defaults, depth + 1)?;
        self.collection(&item.children, depth + 1)?;
        self.writer.bytes(&[item.state])?;
        self.writer.u32(item.buy_price)?;
        self.writer.u32(item.sell_price)?;
        item.derived.write(&mut self.writer)?;
        self.seen.insert(id);
        self.active.remove(&id);
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Bound);
        }
        Ok(Self { bytes, at: 0 })
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.remaining() {
            return Err(Error::Truncated);
        }
        let bytes = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(bytes)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().expect("fixed width"))
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.fixed::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }
    fn words(&mut self) -> Result<Vec<u32>, Error> {
        let count = self.u32()? as usize;
        if count > MAX_WORDS || count > self.remaining() / 4 {
            return Err(Error::Bound);
        }
        (0..count).map(|_| self.u32()).collect()
    }
    fn finish(&self) -> Result<(), Error> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(Error::Trailing)
        }
    }
}
#[derive(Default)]
struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(Error::Bound);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn u32(&mut self, value: u32) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    fn words(&mut self, values: &[u32]) -> Result<(), Error> {
        if values.len() > MAX_WORDS {
            return Err(Error::Bound);
        }
        self.u32(values.len() as u32)?;
        for &value in values {
            self.u32(value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
