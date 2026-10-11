// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Profile};
use crate::bits::BitWriter;
use crate::replication::{Error, Reader, Writer, vehicle};
use nfs_protocol::world::BitSpan;
use std::collections::BTreeMap;

pub const MAX_CATALOGS: usize = 256;
pub const MAX_PROFILES: usize = 512;
pub const MAX_TYPES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Asset {
    pub bundle: u16,
    pub type_id: u16,
    pub local_index: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    bundle: u16,
    entries: Vec<(u16, u16)>,
    count: u16,
}
impl Catalog {
    pub fn new(bundle: u16, entries: &[(u16, u16)]) -> Result<Self, Error> {
        if bundle >= 2048 || bundle == 2003 {
            return Err(Error::Unsupported);
        }
        if entries.is_empty() || entries.len() > MAX_TYPES {
            return Err(Error::Bound);
        }
        let mut previous = 0;
        let mut count = 0u16;
        for &(kind, n) in entries {
            if kind <= previous {
                return Err(Error::Shape);
            }
            count = count.checked_add(n).ok_or(Error::Bound)?;
            previous = kind;
        }
        if count == 0 {
            return Err(Error::Shape);
        }
        Ok(Self {
            bundle,
            entries: entries.to_vec(),
            count,
        })
    }
    fn width(&self) -> u8 {
        (16 - self.count.leading_zeros()) as u8
    }
    fn resolve(&self, mut flat: u16) -> Result<Asset, Error> {
        if flat >= self.count {
            return Err(Error::Shape);
        }
        for &(type_id, count) in &self.entries {
            if flat < count {
                return Ok(Asset {
                    bundle: self.bundle,
                    type_id,
                    local_index: flat,
                });
            }
            flat -= count;
        }
        Err(Error::Shape)
    }
    fn flatten(&self, asset: Asset) -> Result<u16, Error> {
        if asset.bundle != self.bundle {
            return Err(Error::Shape);
        }
        let mut flat = 0;
        for &(kind, count) in &self.entries {
            if kind == asset.type_id {
                return if asset.local_index < count {
                    Ok(flat + asset.local_index)
                } else {
                    Err(Error::Shape)
                };
            }
            flat += count;
        }
        Err(Error::Unsupported)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Content {
    catalogs: BTreeMap<u16, Catalog>,
    profiles: BTreeMap<Asset, Composition>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Composition {
    Entity(Profile),
    Vehicle(vehicle::Profile),
}
impl Content {
    pub fn new(catalogs: Vec<Catalog>, profiles: Vec<(Asset, Profile)>) -> Result<Self, Error> {
        if catalogs.is_empty() || catalogs.len() > MAX_CATALOGS || profiles.len() > MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut result = Self {
            catalogs: BTreeMap::new(),
            profiles: BTreeMap::new(),
        };
        for c in catalogs {
            if result.catalogs.insert(c.bundle, c).is_some() {
                return Err(Error::Shape);
            }
        }
        for (asset, profile) in profiles {
            result.catalog(asset.bundle)?.flatten(asset)?;
            if result
                .profiles
                .insert(asset, Composition::Entity(profile))
                .is_some()
            {
                return Err(Error::Shape);
            }
        }
        Ok(result)
    }
    fn catalog(&self, bundle: u16) -> Result<&Catalog, Error> {
        self.catalogs.get(&bundle).ok_or(Error::Unsupported)
    }
    pub fn profile(&self, asset: Asset) -> Result<&Profile, Error> {
        match self.profiles.get(&asset).ok_or(Error::Unsupported)? {
            Composition::Entity(profile) => Ok(profile),
            Composition::Vehicle(_) => Err(Error::TypeMismatch),
        }
    }
    pub fn with_vehicles(
        mut self,
        profiles: Vec<(Asset, vehicle::Profile)>,
    ) -> Result<Self, Error> {
        if profiles.len() > MAX_PROFILES.saturating_sub(self.profiles.len()) {
            return Err(Error::Bound);
        }
        for (asset, profile) in profiles {
            self.catalog(asset.bundle)?.flatten(asset)?;
            if self
                .profiles
                .insert(asset, Composition::Vehicle(profile))
                .is_some()
            {
                return Err(Error::Shape);
            }
        }
        Ok(self)
    }
    pub fn vehicle_profile(&self, asset: Asset) -> Result<&vehicle::Profile, Error> {
        match self.profiles.get(&asset).ok_or(Error::Unsupported)? {
            Composition::Vehicle(profile) => Ok(profile),
            Composition::Entity(_) => Err(Error::TypeMismatch),
        }
    }
    pub fn binding(&self, asset: Asset) -> Result<Binding, Error> {
        let catalog = self.catalog(asset.bundle)?;
        catalog.flatten(asset)?;
        Ok(Binding {
            asset,
            catalog: catalog.clone(),
            profile: self.profiles.get(&asset).ok_or(Error::Unsupported)?.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    asset: Asset,
    catalog: Catalog,
    profile: Composition,
}
impl Binding {
    pub fn asset(&self) -> Asset {
        self.asset
    }
    pub fn profile(&self) -> Result<&Profile, Error> {
        match &self.profile {
            Composition::Entity(profile) => Ok(profile),
            Composition::Vehicle(_) => Err(Error::TypeMismatch),
        }
    }
    pub fn composition(&self) -> &Composition {
        &self.profile
    }
    pub fn vehicle_profile(&self) -> Result<&vehicle::Profile, Error> {
        match &self.profile {
            Composition::Vehicle(profile) => Ok(profile),
            Composition::Entity(_) => Err(Error::TypeMismatch),
        }
    }
    pub(crate) fn content(&self) -> Content {
        Content {
            catalogs: BTreeMap::from([(self.asset.bundle, self.catalog.clone())]),
            profiles: BTreeMap::from([(self.asset, self.profile.clone())]),
        }
    }
    pub(crate) fn prefix(&self, prefix: &Prefix) -> Result<BitWriter, Error> {
        if prefix.asset != self.asset {
            return Err(Error::TypeMismatch);
        }
        prefix.encode_catalog(&self.catalog)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parent {
    pub reference: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefix {
    pub parent: Option<Parent>,
    pub blueprint: u16,
    pub sub_id: u32,
    pub owner: Option<u16>,
    pub asset: Asset,
}
impl Prefix {
    pub fn decode(input: BitSpan<'_>, content: &Content) -> Result<(Self, usize), Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        if !r.bit()? {
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
        let owner = r.optional(Reader::reference)?;
        if r.bit()? || r.take(2)? != 2 {
            return Err(Error::Unsupported);
        }
        let catalog = content.catalog(r.take(11)? as u16)?;
        let asset = catalog.resolve(r.take(catalog.width())? as u16)?;
        Ok((
            Self {
                parent,
                blueprint,
                sub_id,
                owner,
                asset,
            },
            r.pos,
        ))
    }
    pub fn encode(&self, content: &Content) -> Result<BitWriter, Error> {
        let catalog = content.catalog(self.asset.bundle)?;
        self.encode_catalog(catalog)
    }
    fn encode_catalog(&self, catalog: &Catalog) -> Result<BitWriter, Error> {
        let flat = catalog.flatten(self.asset)?;
        let mut w = Writer(BitWriter::new());
        w.bit(true)?;
        w.bit(self.parent.is_some())?;
        if let Some(parent) = &self.parent {
            w.optional(&parent.reference, Writer::reference)?;
            w.bit(false)?;
        }
        w.reference(&self.blueprint)?;
        w.put(u64::from(self.sub_id), 32)?;
        w.optional(&self.owner, Writer::reference)?;
        w.bit(false)?;
        w.put(2, 2)?;
        w.put(u64::from(self.asset.bundle), 11)?;
        w.put(u64::from(flat), usize::from(catalog.width()))?;
        Ok(w.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Creation {
    pub prefix: Prefix,
    pub body: Body,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    pub creation: Creation,
    pub prefix_bits: usize,
    pub initial_bits: usize,
    pub bits: usize,
}
impl Creation {
    pub fn decode(input: BitSpan<'_>, content: &Content) -> Result<Decoded, Error> {
        let (prefix, prefix_bits) = Prefix::decode(input, content)?;
        let body = Body::decode(
            input.after(prefix_bits).map_err(|_| Error::Truncated)?,
            content.profile(prefix.asset)?,
            true,
        )?;
        let initial_bits = prefix_bits + body.initial_bits.ok_or(Error::Shape)?;
        let bits = prefix_bits + body.bits;
        if bits > crate::replication::MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        Ok(Decoded {
            creation: Self {
                prefix,
                body: body.body,
            },
            prefix_bits,
            initial_bits,
            bits,
        })
    }
    pub fn encode(&self, content: &Content) -> Result<BitWriter, Error> {
        if self.body.initial.is_none() {
            return Err(Error::Shape);
        }
        let mut w = self.prefix.encode(content)?;
        let body = self.body.encode(content.profile(self.prefix.asset)?)?;
        if w.len() + body.len() > crate::replication::MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        w.put_span(body.span());
        Ok(w)
    }
}

#[cfg(test)]
mod tests;
