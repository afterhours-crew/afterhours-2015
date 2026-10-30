// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Kind-6 creations referencing GameObjectData directly, rather than a
//! Blueprint. Catalog indices are flattened in native class/registry order.
//! Callers supply the complete family count and only established compositions;
//! an in-range but unmodeled entry remains unsupported.
use super::{Decoded, Error, Initial, Reader, Record, Update, Writer};
use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;
use std::{collections::BTreeMap, sync::Arc};

pub use super::entity::creation::Parent;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Asset {
    pub bundle: u16,
    pub index: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Composition {
    RaceSpawner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    bundle: u16,
    count: u16,
    entries: BTreeMap<u16, Composition>,
}
impl Catalog {
    pub fn new(bundle: u16, count: u16, entries: &[(u16, Composition)]) -> Result<Self, Error> {
        if bundle >= 2048 || bundle == 2003 {
            return Err(Error::Unsupported);
        }
        if count == 0 || entries.len() > super::entity::creation::MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut result = Self {
            bundle,
            count,
            entries: BTreeMap::new(),
        };
        for &(index, composition) in entries {
            if index >= count || result.entries.insert(index, composition).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(result)
    }
    fn width(&self) -> u8 {
        (16 - self.count.leading_zeros()) as u8
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Content {
    catalogs: BTreeMap<u16, Catalog>,
}
impl Content {
    pub fn new(catalogs: Vec<Catalog>) -> Result<Self, Error> {
        if catalogs.len() > super::entity::creation::MAX_CATALOGS {
            return Err(Error::Bound);
        }
        let mut result = Self::default();
        for catalog in catalogs {
            if result.catalogs.insert(catalog.bundle, catalog).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(result)
    }
    pub fn binding(&self, asset: Asset) -> Result<Binding, Error> {
        let catalog = self.catalogs.get(&asset.bundle).ok_or(Error::Unsupported)?;
        if asset.index >= catalog.count {
            return Err(Error::Shape);
        }
        Ok(Binding {
            asset,
            count: catalog.count,
            composition: *catalog
                .entries
                .get(&asset.index)
                .ok_or(Error::Unsupported)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    asset: Asset,
    count: u16,
    composition: Composition,
}
impl Binding {
    pub fn asset(&self) -> Asset {
        self.asset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefix {
    pub parent: Option<Parent>,
    pub blueprint: u16,
    pub sub_id: u32,
    pub asset: Asset,
}

/// One serializer with an empty initial payload. Each reference can be absent
/// (unchanged) or explicitly null (Some(0)); these are distinct wire states.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct References {
    pub first: Option<u16>,
    pub second: Option<u16>,
}
impl References {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            first: r.optional(Reader::reference)?,
            second: r.optional(Reader::reference)?,
        })
    }
    fn write(w: &mut Writer, value: &Self) -> Result<(), Error> {
        w.optional(&value.first, Writer::reference)?;
        w.optional(&value.second, Writer::reference)
    }
}

impl Record {
    pub fn from_direct_entity(
        id: u16,
        prefix: Prefix,
        fields: References,
        content: &Content,
    ) -> Result<Self, Error> {
        let result = Self {
            id,
            update: Update::DirectEntity {
                binding: Arc::new(content.binding(prefix.asset)?),
                fields,
            },
            initial: Some(Initial::DirectEntity(prefix)),
        };
        result.encode()?;
        Ok(result)
    }
    pub fn direct_entity_binding(&self) -> Option<&Arc<Binding>> {
        match &self.update {
            Update::DirectEntity { binding, .. } => Some(binding),
            _ => None,
        }
    }
    pub(crate) fn direct_entity_wire(&self) -> Result<BitWriter, Error> {
        let Update::DirectEntity { binding, fields } = &self.update else {
            return Err(Error::TypeMismatch);
        };
        let mut w = Writer(BitWriter::new());
        if let Some(initial) = &self.initial {
            let Initial::DirectEntity(prefix) = initial else {
                return Err(Error::TypeMismatch);
            };
            if prefix.asset != binding.asset {
                return Err(Error::TypeMismatch);
            }
            w.bit(false)?;
            w.bit(prefix.parent.is_some())?;
            if let Some(parent) = &prefix.parent {
                w.optional(&parent.reference, Writer::reference)?;
                w.bit(false)?; // Parent transform is not supported.
            }
            w.reference(&prefix.blueprint)?;
            w.put(u64::from(prefix.sub_id), 32)?;
            w.put(2, 2)?;
            w.put(u64::from(prefix.asset.bundle), 11)?;
            w.put(
                u64::from(prefix.asset.index),
                (16 - binding.count.leading_zeros()) as usize,
            )?;
        }
        match binding.composition {
            // The native single-serializer path omits the outer presence mask.
            Composition::RaceSpawner => References::write(&mut w, fields)?,
        }
        Ok(w.0)
    }
    pub(crate) fn decode_direct_entity(
        input: BitSpan<'_>,
        id: u16,
        create: bool,
        content: Option<&Content>,
        binding: Option<&Arc<Binding>>,
    ) -> Result<Decoded, Error> {
        let mut r = Reader {
            span: input,
            pos: if create { 19 } else { 14 },
        };
        let (initial, binding, initial_bits) = if create {
            if r.bit()? {
                return Err(Error::Unsupported);
            }
            let parent = if r.bit()? {
                let reference = r.optional(Reader::reference)?;
                if r.bit()? {
                    return Err(Error::Unsupported);
                }
                Some(Parent { reference })
            } else {
                None
            };
            let blueprint = r.reference()?;
            let sub_id = r.take(32)?;
            if r.take(2)? != 2 {
                return Err(Error::Unsupported);
            }
            let bundle = r.take(11)? as u16;
            let content = content.ok_or(Error::Unsupported)?;
            let catalog = content.catalogs.get(&bundle).ok_or(Error::Unsupported)?;
            let asset = Asset {
                bundle,
                index: r.take(catalog.width())? as u16,
            };
            let binding = Arc::new(content.binding(asset)?);
            (
                Some(Initial::DirectEntity(Prefix {
                    parent,
                    blueprint,
                    sub_id,
                    asset,
                })),
                binding,
                Some(r.pos),
            )
        } else {
            (None, Arc::clone(binding.ok_or(Error::UnknownObject)?), None)
        };
        let fields = match binding.composition {
            Composition::RaceSpawner => References::read(&mut r)?,
        };
        Ok(Decoded {
            record: Self {
                id,
                initial,
                update: Update::DirectEntity { binding, fields },
            },
            initial_bits,
            bits: r.pos,
        })
    }
}

#[cfg(test)]
mod tests;
