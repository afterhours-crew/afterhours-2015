// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;

pub const MAX_RECORD_BITS: usize = 32768;
pub const MAX_OBJECTS: usize = 8191;
pub const MAX_NAME: usize = 1023;
pub const MAX_REFERENCES: usize = 255;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Bound,
    Shape,
    Unsupported,
    UnknownObject,
    DuplicateObject,
    TypeMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Actor,
    Entity,
    Participant,
    Player,
    SubLevel,
}
impl Kind {
    pub fn index(self) -> u8 {
        match self {
            Self::Actor => 0,
            Self::Entity => 6,
            Self::Participant => 11,
            Self::Player => 14,
            Self::SubLevel => 24,
        }
    }
    fn from_index(index: u8) -> Result<Self, Error> {
        match index {
            0 => Ok(Self::Actor),
            6 => Ok(Self::Entity),
            11 => Ok(Self::Participant),
            14 => Ok(Self::Player),
            24 => Ok(Self::SubLevel),
            _ => Err(Error::Unsupported),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Identity {
    pub persona: u64,
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Eq, PartialEq)]
pub struct NamedIdentity {
    pub identity: Identity,
    pub name: Vec<u8>,
}
impl std::fmt::Debug for NamedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedIdentity")
            .field("name_bytes", &self.name.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipantInit {
    pub selector: u16,
    pub serial: u16,
    pub identity: NamedIdentity,
    pub player: u16,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParticipantUpdate {
    pub player: Option<u16>,
    pub entity: Option<u16>,
    pub identity: Option<NamedIdentity>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActorInit {
    pub participant: u16,
    pub flag_a: bool,
    pub flag_b: bool,
    pub word_a: u16,
    pub word_b: u16,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActorUpdate {
    pub flag_a: Option<bool>,
    pub flag_b: Option<bool>,
    pub word_pair: Option<[u16; 2]>,
    pub word_c: Option<u32>,
    pub references_a: Option<Vec<u16>>,
    pub references_b: Option<Vec<u16>>,
}
#[derive(Clone, Eq, PartialEq)]
pub struct PlayerInit {
    pub value8: u8,
    pub name: Vec<u8>,
    pub value255: u8,
    pub runtime_index: u8,
    pub identity: Identity,
    pub flag_a: bool,
    pub flag_b: bool,
}
impl std::fmt::Debug for PlayerInit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerInit")
            .field("runtime_index", &self.runtime_index)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmptyAsset {
    Implicit,
    Sentinel { mode: u8 },
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlayerBaseUpdate {
    pub value16: Option<u8>,
    pub name_identity: Option<NamedIdentity>,
    pub identity_a: Option<Identity>,
    pub value8: Option<u8>,
    pub configured_pair: Option<(u8, u32)>,
    pub empty_assets: bool,
    pub empty_structured: bool,
    pub asset: Option<EmptyAsset>,
    /// Ninth base group, a counted collection like `empty_assets`. Official
    /// hosts send it present and empty only in the receiving client's own
    /// Player creation (E101 ghost 9, E742 ghost 148); remote players omit it
    /// (E132 width 1). Entries remain unsupported.
    pub empty_local: bool,
    pub identity_b: Option<Identity>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlayerUpdate {
    pub base: Option<PlayerBaseUpdate>,
    pub component_present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Initial {
    Actor(ActorInit),
    Entity {
        prefix: entity::creation::Prefix,
        fields: Vec<entity::Initial>,
    },
    Vehicle {
        prefix: entity::creation::Prefix,
        creation: vehicle::Creation,
    },
    Participant(ParticipantInit),
    Player(PlayerInit),
    SubLevel {
        prefix: sublevel::Prefix,
        fields: Vec<sublevel::Initial>,
    },
}
impl Initial {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Actor(_) => Kind::Actor,
            Self::Entity { .. } | Self::Vehicle { .. } => Kind::Entity,
            Self::Participant(_) => Kind::Participant,
            Self::Player(_) => Kind::Player,
            Self::SubLevel { .. } => Kind::SubLevel,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Actor(ActorUpdate),
    Entity {
        binding: std::sync::Arc<entity::creation::Binding>,
        fields: Vec<Option<entity::Update>>,
    },
    Vehicle {
        binding: std::sync::Arc<entity::creation::Binding>,
        fields: Vec<Option<vehicle::Update>>,
    },
    Participant(ParticipantUpdate),
    Player(PlayerUpdate),
    SubLevel {
        profile: sublevel::Profile,
        fields: Vec<Option<sublevel::Update>>,
    },
}
impl Update {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Actor(_) => Kind::Actor,
            Self::Entity { .. } | Self::Vehicle { .. } => Kind::Entity,
            Self::Participant(_) => Kind::Participant,
            Self::Player(_) => Kind::Player,
            Self::SubLevel { .. } => Kind::SubLevel,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub id: u16,
    pub initial: Option<Initial>,
    pub update: Update,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    pub record: Record,
    pub initial_bits: Option<usize>,
    pub bits: usize,
}

struct Reader<'a> {
    span: BitSpan<'a>,
    pos: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: u8) -> Result<u32, Error> {
        if self.pos + usize::from(n) > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        let v = self
            .span
            .read_u32(self.pos, n)
            .map_err(|_| Error::Truncated)?;
        self.pos += usize::from(n);
        Ok(v)
    }
    fn bit(&mut self) -> Result<bool, Error> {
        Ok(self.take(1)? != 0)
    }
    fn string(&mut self, width: u8, max: usize) -> Result<Vec<u8>, Error> {
        let n = self.take(width)? as usize;
        if n > max {
            return Err(Error::Bound);
        }
        if n > (self.span.len() - self.pos) / 8 {
            return Err(Error::Truncated);
        }
        (0..n).map(|_| self.take(8).map(|v| v as u8)).collect()
    }
    fn identity(&mut self) -> Result<Identity, Error> {
        let low = u64::from(self.take(32)?);
        let high = u64::from(self.take(32)?);
        Ok(Identity {
            persona: low | (high << 32),
            bytes: self.string(5, 16)?,
        })
    }
    fn named(&mut self) -> Result<NamedIdentity, Error> {
        Ok(NamedIdentity {
            identity: self.identity()?,
            name: self.string(10, MAX_NAME)?,
        })
    }
    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        if self.bit()? {
            read(self).map(Some)
        } else {
            Ok(None)
        }
    }
    fn reference(&mut self) -> Result<u16, Error> {
        Ok(self.take(13)? as u16)
    }
    fn references(&mut self) -> Result<Vec<u16>, Error> {
        let n = self.take(8)? as usize;
        if n > (self.span.len() - self.pos) / 13 {
            return Err(Error::Truncated);
        }
        (0..n).map(|_| self.reference()).collect()
    }
    fn empty_collection(&mut self) -> Result<bool, Error> {
        if !self.bit()? {
            return Ok(false);
        }
        match self.take(7)? {
            0 => Ok(true),
            1..=64 => Err(Error::Unsupported),
            _ => Err(Error::Shape),
        }
    }
    fn asset(&mut self) -> Result<EmptyAsset, Error> {
        let mode = self.take(2)? as u8;
        match mode {
            0 => Ok(EmptyAsset::Implicit),
            2 | 3 if self.take(11)? == 0x7d3 => Ok(EmptyAsset::Sentinel { mode }),
            _ => Err(Error::Unsupported),
        }
    }
    fn initial(&mut self, kind: Kind) -> Result<Initial, Error> {
        Ok(match kind {
            Kind::SubLevel | Kind::Entity => return Err(Error::Unsupported),
            Kind::Actor => Initial::Actor(ActorInit {
                participant: self.reference()?,
                flag_a: self.bit()?,
                flag_b: self.bit()?,
                word_a: self.take(16)? as u16,
                word_b: self.take(16)? as u16,
            }),
            Kind::Participant => Initial::Participant(ParticipantInit {
                selector: self.take(9)? as u16,
                serial: self.take(10)? as u16,
                identity: self.named()?,
                player: self.reference()?,
            }),
            Kind::Player => Initial::Player(PlayerInit {
                value8: self.take(8)? as u8,
                name: self.string(10, MAX_NAME)?,
                value255: self.take(8)? as u8,
                runtime_index: self.take(7)? as u8,
                identity: self.identity()?,
                flag_a: self.bit()?,
                flag_b: self.bit()?,
            }),
        })
    }
    fn update(&mut self, kind: Kind) -> Result<Update, Error> {
        Ok(match kind {
            Kind::SubLevel | Kind::Entity => return Err(Error::Unsupported),
            Kind::Participant => Update::Participant(ParticipantUpdate {
                player: self.optional(Self::reference)?,
                entity: self.optional(Self::reference)?,
                identity: self.optional(Self::named)?,
            }),
            Kind::Actor => Update::Actor(ActorUpdate {
                flag_a: self.optional(Self::bit)?,
                flag_b: self.optional(Self::bit)?,
                word_pair: self.optional(|r| Ok([r.take(16)? as u16, r.take(16)? as u16]))?,
                word_c: self.optional(|r| r.take(32))?,
                references_a: self.optional(Self::references)?,
                references_b: self.optional(Self::references)?,
            }),
            Kind::Player => {
                let base = self.optional(|r| {
                    let value16 = r.optional(|r| Ok(r.take(5)? as u8))?;
                    let name_identity = r.optional(|r| {
                        let name = r.string(10, MAX_NAME)?;
                        Ok(NamedIdentity {
                            identity: r.identity()?,
                            name,
                        })
                    })?;
                    let identity_a = r.optional(Self::identity)?;
                    let value8 = r.optional(|r| Ok(r.take(8)? as u8))?;
                    let configured_pair = r.optional(|r| Ok((r.take(5)? as u8, r.take(18)?)))?;
                    let empty_assets = r.empty_collection()?;
                    let empty_structured = r.empty_collection()?;
                    let asset = r.optional(Self::asset)?;
                    let empty_local = r.empty_collection()?;
                    let identity_b = r.optional(Self::identity)?;
                    Ok(PlayerBaseUpdate {
                        value16,
                        name_identity,
                        identity_a,
                        value8,
                        configured_pair,
                        empty_assets,
                        empty_structured,
                        asset,
                        empty_local,
                        identity_b,
                    })
                })?;
                Update::Player(PlayerUpdate {
                    base,
                    component_present: self.bit()?,
                })
            }
        })
    }
}

struct Writer(BitWriter);
impl Writer {
    fn put(&mut self, value: u64, width: usize) -> Result<(), Error> {
        if width > 32 || value >= (1u64 << width) {
            return Err(Error::Shape);
        }
        if self.0.len() + width > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        self.0.put(value, width);
        Ok(())
    }
    fn bit(&mut self, v: bool) -> Result<(), Error> {
        self.put(u64::from(v), 1)
    }
    fn string(&mut self, bytes: &[u8], width: usize, max: usize) -> Result<(), Error> {
        if bytes.len() > max {
            return Err(Error::Bound);
        }
        self.put(bytes.len() as u64, width)?;
        for b in bytes {
            self.put(u64::from(*b), 8)?
        }
        Ok(())
    }
    fn identity(&mut self, v: &Identity) -> Result<(), Error> {
        self.put(v.persona & 0xffffffff, 32)?;
        self.put(v.persona >> 32, 32)?;
        self.string(&v.bytes, 5, 16)
    }
    fn named(&mut self, v: &NamedIdentity) -> Result<(), Error> {
        self.identity(&v.identity)?;
        self.string(&v.name, 10, MAX_NAME)
    }
    fn optional<T>(
        &mut self,
        v: &Option<T>,
        write: impl FnOnce(&mut Self, &T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.bit(v.is_some())?;
        if let Some(v) = v {
            write(self, v)?
        }
        Ok(())
    }
    fn reference(&mut self, v: &u16) -> Result<(), Error> {
        self.put(u64::from(*v), 13)
    }
    fn references(&mut self, v: &[u16]) -> Result<(), Error> {
        if v.len() > MAX_REFERENCES {
            return Err(Error::Bound);
        }
        self.put(v.len() as u64, 8)?;
        for r in v {
            self.reference(r)?
        }
        Ok(())
    }
    fn initial(&mut self, v: &Initial) -> Result<(), Error> {
        match v {
            Initial::SubLevel { .. } | Initial::Entity { .. } | Initial::Vehicle { .. } => {
                return Err(Error::Unsupported);
            }
            Initial::Actor(v) => {
                self.reference(&v.participant)?;
                self.bit(v.flag_a)?;
                self.bit(v.flag_b)?;
                self.put(u64::from(v.word_a), 16)?;
                self.put(u64::from(v.word_b), 16)?;
            }
            Initial::Participant(v) => {
                self.put(u64::from(v.selector), 9)?;
                self.put(u64::from(v.serial), 10)?;
                self.named(&v.identity)?;
                self.reference(&v.player)?;
            }
            Initial::Player(v) => {
                self.put(u64::from(v.value8), 8)?;
                self.string(&v.name, 10, MAX_NAME)?;
                self.put(u64::from(v.value255), 8)?;
                self.put(u64::from(v.runtime_index), 7)?;
                self.identity(&v.identity)?;
                self.bit(v.flag_a)?;
                self.bit(v.flag_b)?;
            }
        }
        Ok(())
    }
    fn update(&mut self, v: &Update) -> Result<(), Error> {
        match v {
            Update::SubLevel { .. } | Update::Entity { .. } | Update::Vehicle { .. } => {
                return Err(Error::Unsupported);
            }
            Update::Participant(v) => {
                self.optional(&v.player, Self::reference)?;
                self.optional(&v.entity, Self::reference)?;
                self.optional(&v.identity, Self::named)?;
            }
            Update::Actor(v) => {
                self.optional(&v.flag_a, |w, v| w.bit(*v))?;
                self.optional(&v.flag_b, |w, v| w.bit(*v))?;
                self.optional(&v.word_pair, |w, v| {
                    w.put(u64::from(v[0]), 16)?;
                    w.put(u64::from(v[1]), 16)
                })?;
                self.optional(&v.word_c, |w, v| w.put(u64::from(*v), 32))?;
                self.optional(&v.references_a, |w, v| w.references(v))?;
                self.optional(&v.references_b, |w, v| w.references(v))?;
            }
            Update::Player(v) => {
                self.optional(&v.base, |w, v| {
                    w.optional(&v.value16, |w, v| w.put(u64::from(*v), 5))?;
                    w.optional(&v.name_identity, |w, v| {
                        w.string(&v.name, 10, MAX_NAME)?;
                        w.identity(&v.identity)
                    })?;
                    w.optional(&v.identity_a, Self::identity)?;
                    w.optional(&v.value8, |w, v| w.put(u64::from(*v), 8))?;
                    w.optional(&v.configured_pair, |w, v| {
                        w.put(u64::from(v.0), 5)?;
                        w.put(u64::from(v.1), 18)
                    })?;
                    w.bit(v.empty_assets)?;
                    if v.empty_assets {
                        w.put(0, 7)?
                    }
                    w.bit(v.empty_structured)?;
                    if v.empty_structured {
                        w.put(0, 7)?
                    }
                    w.optional(&v.asset, |w, v| match v {
                        EmptyAsset::Implicit => w.put(0, 2),
                        EmptyAsset::Sentinel { mode } if [2, 3].contains(mode) => {
                            w.put(u64::from(*mode), 2)?;
                            w.put(0x7d3, 11)
                        }
                        _ => Err(Error::Shape),
                    })?;
                    w.bit(v.empty_local)?;
                    if v.empty_local {
                        w.put(0, 7)?
                    }
                    w.optional(&v.identity_b, Self::identity)
                })?;
                self.bit(v.component_present)?;
            }
        }
        Ok(())
    }
}

impl Record {
    pub fn decode(input: BitSpan<'_>, bound: Option<Kind>) -> Result<Decoded, Error> {
        Self::decode_context(input, bound, None, None, None, None)
    }
    /// Decodes one record using caller-supplied scene and entity schemas.
    /// The supplied context is borrowed and remains unchanged on every error.
    pub fn decode_context(
        input: BitSpan<'_>,
        bound: Option<Kind>,
        content: Option<&sublevel::Content>,
        profile: Option<&sublevel::Profile>,
        entity_content: Option<&entity::creation::Content>,
        entity_binding: Option<&std::sync::Arc<entity::creation::Binding>>,
    ) -> Result<Decoded, Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let id = r.reference()?;
        let create = r.bit()?;
        let kind = if create {
            Kind::from_index(r.take(5)? as u8)?
        } else {
            bound.ok_or(Error::UnknownObject)?
        };
        if kind == Kind::SubLevel {
            return Self::decode_scene(input, id, create, r.pos, content, profile);
        }
        if kind == Kind::Entity {
            return Self::decode_entity(input, id, create, r.pos, entity_content, entity_binding);
        }
        let initial = if create { Some(r.initial(kind)?) } else { None };
        let initial_bits = initial.as_ref().map(|_| r.pos);
        let update = r.update(kind)?;
        Ok(Decoded {
            record: Self {
                id,
                initial,
                update,
            },
            initial_bits,
            bits: r.pos,
        })
    }
    pub fn encode(&self) -> Result<BitWriter, Error> {
        if self
            .initial
            .as_ref()
            .is_some_and(|v| v.kind() != self.update.kind())
        {
            return Err(Error::TypeMismatch);
        }
        let mut w = Writer(BitWriter::new());
        w.reference(&self.id)?;
        w.bit(self.initial.is_some())?;
        if matches!(
            &self.update,
            Update::SubLevel { .. } | Update::Entity { .. } | Update::Vehicle { .. }
        ) {
            if self.initial.is_some() {
                w.put(u64::from(self.update.kind().index()), 5)?;
            }
            let body = match &self.update {
                Update::Entity { .. } => self.entity_wire()?,
                Update::Vehicle { .. } => self.vehicle_wire()?,
                _ => self.scene_wire()?,
            };
            if w.0.len() + body.len() > MAX_RECORD_BITS {
                return Err(Error::Bound);
            }
            w.0.put_span(body.span());
            return Ok(w.0);
        }
        if let Some(initial) = &self.initial {
            w.put(u64::from(initial.kind().index()), 5)?;
            w.initial(initial)?;
        }
        w.update(&self.update)?;
        Ok(w.0)
    }
}

pub mod entity;
mod entity_record;

pub mod players;
mod scene_record;

pub mod section;
pub mod state;
pub mod sublevel;

pub mod vehicle;
mod vehicle_record;
