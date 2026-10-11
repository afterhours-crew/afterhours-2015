// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, MAX_RECORD_BITS, Reader, Writer};
use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;
use std::collections::BTreeMap;

pub use super::entity::Rpc;
pub const MAX_SERIALIZERS: usize = 512;
pub const MAX_PROFILES: usize = 256;
pub const MAX_REFERENCES: usize = 1024;
mod fields;
pub mod gameplay;
pub mod genesis;
pub mod ordinary;
pub mod passive;
pub mod root;
pub mod state;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Noop,
    Rpc,
    LevelReference,
    BoolProperty,
    FloatProperty,
    I32Property,
    RpcReferences,
    RpcPairs,
    RpcTagged,
    RpcOptional64,
    RpcGuid,
    RpcBool,
    RpcMap,
    RpcFourReferences,
    RpcSet64,
    RpcPursuitMaps,
    RpcSpawn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    root: bool,
    kinds: Vec<Kind>,
}
impl Profile {
    pub fn new(root: bool, kinds: &[Kind]) -> Result<Self, Error> {
        if kinds.is_empty() || kinds.len() > MAX_SERIALIZERS {
            return Err(Error::Bound);
        }
        if kinds[0] != Kind::Noop {
            return Err(Error::Unsupported);
        }
        Ok(Self {
            root,
            kinds: kinds.to_vec(),
        })
    }
    pub fn is_root(&self) -> bool {
        self.root
    }
    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Content(BTreeMap<u32, Profile>);
impl Content {
    pub fn new(profiles: Vec<(u32, Profile)>) -> Result<Self, Error> {
        if profiles.is_empty() || profiles.len() > MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut values = BTreeMap::new();
        for (key, profile) in profiles {
            if values.insert(key, profile).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(Self(values))
    }
    pub fn profile(&self, key: u32) -> Result<&Profile, Error> {
        self.0.get(&key).ok_or(Error::Unsupported)
    }
    pub fn profiles(&self) -> impl Iterator<Item = (u32, &Profile)> {
        self.0.iter().map(|(key, profile)| (*key, profile))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Initial {
    Empty,
    Rpc(Rpc),
    BoolProperty(Option<bool>),
    FloatProperty(Option<u32>),
    I32Property(Option<i32>),
    RpcReferences { rpc: Rpc, value: Vec<u16> },
    RpcPairs { rpc: Rpc, value: Vec<(u16, u16)> },
    RpcTagged { rpc: Rpc, value: Vec<(u16, i8)> },
    RpcOptional64 { rpc: Rpc, value: Option<u64> },
    RpcGuid { rpc: Rpc, value: Option<[u8; 16]> },
    RpcBool { rpc: Rpc, value: bool },
    RpcMap { rpc: Rpc, value: Vec<(u16, u32)> },
    RpcSpawnInactive { rpc: Rpc, references: Vec<u16> },
}
impl Initial {
    pub fn rpc(&self) -> Option<&Rpc> {
        match self {
            Self::Rpc(rpc)
            | Self::RpcReferences { rpc, .. }
            | Self::RpcPairs { rpc, .. }
            | Self::RpcTagged { rpc, .. }
            | Self::RpcOptional64 { rpc, .. }
            | Self::RpcGuid { rpc, .. }
            | Self::RpcBool { rpc, .. }
            | Self::RpcMap { rpc, .. }
            | Self::RpcSpawnInactive { rpc, .. } => Some(rpc),
            Self::Empty | Self::BoolProperty(_) | Self::FloatProperty(_) | Self::I32Property(_) => {
                None
            }
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Set64 {
    pub entries: Option<Vec<u64>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Noop,
    LevelReference(Option<u16>),
    BoolProperty(Option<bool>),
    FloatProperty(Option<u32>),
    I32Property(Option<i32>),
    ReferenceMap(Option<Vec<(u16, u32)>>),
    FourReferences([Option<u16>; 4]),
    Set64(Option<Set64>),
    PursuitMaps([Option<Vec<(u32, u16)>>; 2]),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Body {
    pub initial: Option<Vec<Initial>>,
    pub updates: Vec<Option<Update>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedBody {
    pub body: Body,
    pub initial_bits: Option<usize>,
    pub bits: usize,
}
impl Body {
    pub fn decode(
        input: BitSpan<'_>,
        profile: &Profile,
        creation: bool,
    ) -> Result<DecodedBody, Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let initial = if creation {
            let mut values = Vec::with_capacity(profile.kinds.len());
            for kind in &profile.kinds {
                values.push(fields::read_initial(&mut r, *kind)?);
            }
            Some(values)
        } else {
            None
        };
        let initial_bits = creation.then_some(r.pos);
        let mut updates = Vec::with_capacity(profile.kinds.len());
        for kind in &profile.kinds {
            let present = profile.kinds.len() == 1 || r.bit()?;
            updates.push(if present {
                Some(fields::read_update(&mut r, *kind)?)
            } else {
                None
            });
        }
        Ok(DecodedBody {
            body: Body { initial, updates },
            initial_bits,
            bits: r.pos,
        })
    }
    pub fn encode(&self, profile: &Profile) -> Result<BitWriter, Error> {
        if self.updates.len() != profile.kinds.len()
            || self
                .initial
                .as_ref()
                .is_some_and(|v| v.len() != profile.kinds.len())
        {
            return Err(Error::Shape);
        }
        let mut w = Writer(BitWriter::new());
        if let Some(initial) = &self.initial {
            for (value, kind) in initial.iter().zip(&profile.kinds) {
                fields::write_initial(&mut w, *kind, value)?;
            }
        }
        for (value, kind) in self.updates.iter().zip(&profile.kinds) {
            if profile.kinds.len() > 1 {
                w.bit(value.is_some())?;
            } else if value.is_none() {
                return Err(Error::Shape);
            }
            if let Some(value) = value {
                fields::write_update(&mut w, *kind, value)?;
            }
        }
        Ok(w.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint {
    pub ghost: u16,
    pub sub_id: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefix {
    pub level_id: u16,
    pub content_key: u32,
    pub blueprint: Option<Blueprint>,
    pub word_a: u32,
    pub word_b: u32,
    pub constructor_word: Option<u32>,
}
impl Prefix {
    pub fn decode(input: BitSpan<'_>) -> Result<(Self, usize), Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let level_id = r.take(16)? as u16;
        let content_key = r.take(32)?;
        let blueprint = r.optional(|r| {
            Ok(Blueprint {
                ghost: r.reference()?,
                sub_id: r.take(32)?,
            })
        })?;
        let word_a = r.take(32)?;
        let word_b = r.take(32)?;
        let constructor_word = if level_id == 0 {
            None
        } else {
            r.optional(|r| r.take(32))?
        };
        Ok((
            Self {
                level_id,
                content_key,
                blueprint,
                word_a,
                word_b,
                constructor_word,
            },
            r.pos,
        ))
    }
    pub fn encode(&self) -> Result<BitWriter, Error> {
        if self.level_id == 0 && self.constructor_word.is_some() {
            return Err(Error::Shape);
        }
        let mut w = Writer(BitWriter::new());
        w.put(u64::from(self.level_id), 16)?;
        w.put(u64::from(self.content_key), 32)?;
        w.optional(&self.blueprint, |w, v| {
            w.reference(&v.ghost)?;
            w.put(u64::from(v.sub_id), 32)
        })?;
        w.put(u64::from(self.word_a), 32)?;
        w.put(u64::from(self.word_b), 32)?;
        if self.level_id != 0 {
            w.optional(&self.constructor_word, |w, v| w.put(u64::from(*v), 32))?;
        }
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
        let (prefix, prefix_bits) = Prefix::decode(input)?;
        let profile = content.profile(prefix.content_key)?;
        if profile.is_root() != (prefix.level_id == 0) {
            return Err(Error::TypeMismatch);
        }
        let decoded = Body::decode(
            input.after(prefix_bits).map_err(|_| Error::Truncated)?,
            profile,
            true,
        )?;
        let bits = prefix_bits + decoded.bits;
        if bits > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        Ok(Decoded {
            creation: Self {
                prefix,
                body: decoded.body,
            },
            prefix_bits,
            initial_bits: prefix_bits + decoded.initial_bits.ok_or(Error::Shape)?,
            bits,
        })
    }
    pub fn encode(&self, content: &Content) -> Result<BitWriter, Error> {
        if self.body.initial.is_none() {
            return Err(Error::Shape);
        }
        let profile = content.profile(self.prefix.content_key)?;
        if profile.is_root() != (self.prefix.level_id == 0) {
            return Err(Error::TypeMismatch);
        }
        let mut wire = self.prefix.encode()?;
        let body = self.body.encode(profile)?;
        if wire.len() + body.len() > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        wire.put_span(body.span());
        Ok(wire)
    }
}
