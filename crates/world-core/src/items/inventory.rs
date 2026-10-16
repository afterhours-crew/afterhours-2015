// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static definitions and the native local-backend initial inventory recipe
//!. Captured item instances are never used as initialization data.
use super::{
    Catalog, Collection, DefinitionClass, Derived, Error, Guid, Item, MAX_COLLECTION,
    MAX_DEFINITIONS, MAX_DEPTH, MAX_ITEMS, MAX_REFERENCES, OWNED,
};
use std::collections::{BTreeMap, BTreeSet};

/// Asset metadata, a separate domain from the wire item-state byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipLevel {
    Claimable,
    Owned,
    Purchasable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefinitionFlags {
    pub deprecated: bool,
    pub requires_owner: bool,
    pub uses_scope: bool,
    pub purchasable: bool,
    pub optional: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    pub class: DefinitionClass,
    pub buy_price: u32,
    pub sell_price: u32,
    pub quantity: u32,
    pub ownership_level: OwnershipLevel,
    pub flags: DefinitionFlags,
    pub sub_items: Vec<Guid>,
    /// Retained static metadata. Native recursive construction uses SubItems;
    /// AdditionalItems is not silently turned into another owned child list.
    pub additional_items: Vec<Guid>,
    /// None means construction is unresolved, not an all-zero default.
    pub default_derived: Option<Derived>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct InventoryCatalog {
    bindings: Catalog,
    definitions: BTreeMap<Guid, Definition>,
    initial: Vec<Guid>,
}
impl std::fmt::Debug for InventoryCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InventoryCatalog")
            .field("definitions", &self.definitions.len())
            .field("initial", &self.initial.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialInventory {
    pub collection: Collection,
    /// Exclusive end of the allocated range; reserve transactionally with the
    /// collection. Allocation does not mutate a shared counter on failure.
    pub next_id: u64,
}

impl InventoryCatalog {
    pub fn new(
        definitions: impl IntoIterator<Item = (Guid, Definition)>,
        initial: Vec<Guid>,
    ) -> Result<Self, Error> {
        if initial.len() > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let mut entries = BTreeMap::new();
        let mut edges = 0usize;
        for (guid, definition) in definitions {
            if entries.len() >= MAX_DEFINITIONS
                || definition.sub_items.len() > MAX_COLLECTION
                || definition.additional_items.len() > MAX_COLLECTION
            {
                return Err(Error::Bound);
            }
            edges += definition.sub_items.len() + definition.additional_items.len();
            if edges > MAX_REFERENCES {
                return Err(Error::Bound);
            }
            if let Some(derived) = &definition.default_derived {
                if derived.layout() != definition.class.layout() {
                    return Err(Error::Shape);
                }
                derived.encode()?;
            }
            if entries.insert(guid, definition).is_some() {
                return Err(Error::Shape);
            }
        }
        let bindings = Catalog::new(entries.iter().map(|(guid, d)| (*guid, d.class)))?;
        for guid in initial.iter().chain(
            entries
                .values()
                .flat_map(|d| d.sub_items.iter().chain(&d.additional_items)),
        ) {
            if !entries.contains_key(guid) {
                return Err(Error::UnknownDefinition);
            }
        }
        let catalog = Self {
            bindings,
            definitions: entries,
            initial,
        };
        // Memoized heights make validation linear in the static graph, even
        // when thousands of definitions share the same defaults.
        let mut heights = BTreeMap::new();
        for guid in catalog.definitions.keys() {
            catalog.height(*guid, 0, &mut BTreeSet::new(), &mut heights)?;
        }
        Ok(catalog)
    }
    fn height(
        &self,
        guid: Guid,
        depth: usize,
        visiting: &mut BTreeSet<Guid>,
        heights: &mut BTreeMap<Guid, usize>,
    ) -> Result<usize, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        if let Some(height) = heights.get(&guid) {
            return Ok(*height);
        }
        if !visiting.insert(guid) {
            return Err(Error::Cycle);
        }
        let mut height = 1;
        for child in &self.definitions[&guid].sub_items {
            height = height.max(1 + self.height(*child, depth + 1, visiting, heights)?);
        }
        if height > MAX_DEPTH {
            return Err(Error::Bound);
        }
        visiting.remove(&guid);
        heights.insert(guid, height);
        Ok(height)
    }
    pub fn bindings(&self) -> &Catalog {
        &self.bindings
    }
    pub fn definitions(&self) -> impl Iterator<Item = (&Guid, &Definition)> {
        self.definitions.iter()
    }
    pub fn definition(&self, guid: &Guid) -> Result<&Definition, Error> {
        self.definitions.get(guid).ok_or(Error::UnknownDefinition)
    }
    pub fn initial_definitions(&self) -> &[Guid] {
        &self.initial
    }

    /// local fallback recipe. This is an explicit server initialization
    /// policy, not a claim about the official fresh-account inventory.
    pub fn instantiate_initial(&self, first_id: u64) -> Result<InitialInventory, Error> {
        if first_id == 0 {
            return Err(Error::Shape);
        }
        let mut result = InitialInventory {
            collection: Collection::default(),
            next_id: first_id,
        };
        for guid in &self.initial {
            self.instantiate(*guid, 0, 0, &mut result)?;
        }
        // The native inventory contains every record in an ID-ordered tree.
        // The wire reference cache handles nested records encountered earlier.
        result.collection.roots = result.collection.items.keys().copied().collect();
        result.collection.encode(&self.bindings)?;
        Ok(result)
    }
    /// Construct one owned subtree from static defaults, retaining parent owners
    /// and shared default/installed references. Every record is an explicit
    /// collection entry so the receiver indexes descendants as well as parents.
    /// The wire reference cache avoids duplicate bodies. No account is mutated.
    /// The monotonic allocator must start after the existing owner's identity.
    pub fn instantiate_owned(
        &self,
        guid: Guid,
        owner: u64,
        first_id: u64,
    ) -> Result<InitialInventory, Error> {
        if owner == 0 || first_id == 0 || owner >= first_id {
            return Err(Error::Shape);
        }
        let mut result = InitialInventory {
            collection: Collection::default(),
            next_id: first_id,
        };
        self.instantiate(guid, owner, 0, &mut result)?;
        result.collection.roots = result.collection.items.keys().copied().collect();
        result.collection.encode(&self.bindings)?;
        Ok(result)
    }
    fn instantiate(
        &self,
        guid: Guid,
        owner: u64,
        depth: usize,
        result: &mut InitialInventory,
    ) -> Result<u64, Error> {
        if depth >= MAX_DEPTH || result.collection.items.len() >= MAX_ITEMS {
            return Err(Error::Bound);
        }
        let definition = self.definition(&guid)?;
        let derived = definition
            .default_derived
            .clone()
            .ok_or(Error::UnsupportedDefault)?;
        let id = result.next_id;
        result.next_id = id.checked_add(1).ok_or(Error::Bound)?;
        result.collection.items.insert(
            id,
            Item {
                id,
                definition: guid,
                owner,
                defaults: Vec::new(),
                children: Vec::new(),
                state: OWNED,
                buy_price: definition.buy_price,
                sell_price: definition.sell_price,
                derived,
            },
        );
        let mut children = Vec::with_capacity(definition.sub_items.len());
        for guid in &definition.sub_items {
            children.push(self.instantiate(*guid, id, depth + 1, result)?);
        }
        let item = result
            .collection
            .items
            .get_mut(&id)
            .ok_or(Error::MissingItem)?;
        item.defaults = children.clone();
        item.children = children;
        Ok(id)
    }
}

#[cfg(test)]
mod tests;
