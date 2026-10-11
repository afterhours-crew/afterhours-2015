// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Reader, Writer};
use crate::bits::BitWriter;
use nfs_protocol::world::{BitSpan, rpc::Serial};

pub const MAX_SERIALIZERS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Root,
    SequenceRoot,
    Rpc,
    RpcReference,
    RpcFlags,
    BoolProperty,
    I32Property,
    RpcVariant,
    RpcGhostReference,
    RpcOptionalReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile(Vec<Kind>);
impl Profile {
    pub fn new(kinds: &[Kind]) -> Result<Self, Error> {
        if kinds.is_empty() || kinds.len() > MAX_SERIALIZERS {
            return Err(Error::Bound);
        }
        let root = |kind: &Kind| matches!(kind, Kind::Root | Kind::SequenceRoot);
        if !root(&kinds[0]) || kinds[1..].iter().any(root) {
            return Err(Error::Unsupported);
        }
        Ok(Self(kinds.to_vec()))
    }
    pub fn kinds(&self) -> &[Kind] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rpc {
    pub selector: u16,
    pub serial: Serial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Initial {
    Root {
        value: u16,
        rpc: Rpc,
        reference: u16,
    },
    SequenceRoot {
        value: u16,
        rpc: Rpc,
        manager: u16,
        manager_selector: u16,
        sequence: u32,
        participants: Vec<u16>,
        stopping: bool,
    },
    Rpc(Rpc),
    RpcReference {
        rpc: Rpc,
        reference: u16,
        target_selector: u16,
    },
    RpcFlags {
        rpc: Rpc,
        flags: [bool; 2],
    },
    BoolProperty(Option<bool>),
    I32Property(Option<i32>),
    RpcVariant {
        rpc: Rpc,
        value: Option<(u8, Option<u32>)>,
    },
    RpcGhostReference {
        rpc: Rpc,
        reference: u16,
    },
    RpcOptionalReference {
        rpc: Rpc,
        reference: Option<u16>,
    },
}
impl Initial {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Root { .. } => Kind::Root,
            Self::SequenceRoot { .. } => Kind::SequenceRoot,
            Self::Rpc(_) => Kind::Rpc,
            Self::RpcReference { .. } => Kind::RpcReference,
            Self::RpcFlags { .. } => Kind::RpcFlags,
            Self::BoolProperty(_) => Kind::BoolProperty,
            Self::I32Property(_) => Kind::I32Property,
            Self::RpcVariant { .. } => Kind::RpcVariant,
            Self::RpcGhostReference { .. } => Kind::RpcGhostReference,
            Self::RpcOptionalReference { .. } => Kind::RpcOptionalReference,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Noop,
    BoolProperty(Option<bool>),
    I32Property(Option<i32>),
    GhostReference(Option<u16>),
}
impl Update {
    fn matches(&self, kind: Kind) -> bool {
        matches!(
            (self, kind),
            (
                Self::Noop,
                Kind::Root
                    | Kind::SequenceRoot
                    | Kind::Rpc
                    | Kind::RpcReference
                    | Kind::RpcFlags
                    | Kind::RpcVariant
                    | Kind::RpcOptionalReference
            ) | (Self::BoolProperty(_), Kind::BoolProperty)
                | (Self::GhostReference(_), Kind::RpcGhostReference)
                | (Self::I32Property(_), Kind::I32Property)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Body {
    pub initial: Option<Vec<Initial>>,
    pub updates: Vec<Option<Update>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    pub body: Body,
    pub initial_bits: Option<usize>,
    pub bits: usize,
}

fn read_rpc(r: &mut Reader<'_>) -> Result<Rpc, Error> {
    Ok(Rpc {
        selector: r.take(9)? as u16,
        serial: Serial::new(r.take(10)? as u16).ok_or(Error::Shape)?,
    })
}
fn write_rpc(w: &mut Writer, rpc: &Rpc) -> Result<(), Error> {
    w.put(u64::from(rpc.selector), 9)?;
    w.put(u64::from(rpc.serial.value()), 10)
}

impl Body {
    pub fn decode(input: BitSpan<'_>, profile: &Profile, creation: bool) -> Result<Decoded, Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let initial = if creation {
            let mut values = Vec::with_capacity(profile.0.len());
            for kind in &profile.0 {
                values.push(match kind {
                    Kind::Root => Initial::Root {
                        value: r.take(16)? as u16,
                        rpc: read_rpc(&mut r)?,
                        reference: r.reference()?,
                    },
                    Kind::SequenceRoot => {
                        let value = r.take(16)? as u16;
                        let rpc = read_rpc(&mut r)?;
                        let manager = r.reference()?;
                        let manager_selector = r.take(9)? as u16;
                        let sequence = r.take(32)?;
                        let count = r.take(8)?;
                        let participants = (0..count)
                            .map(|_| r.reference())
                            .collect::<Result<_, _>>()?;
                        Initial::SequenceRoot {
                            value,
                            rpc,
                            manager,
                            manager_selector,
                            sequence,
                            participants,
                            stopping: r.bit()?,
                        }
                    }
                    Kind::Rpc => Initial::Rpc(read_rpc(&mut r)?),
                    Kind::RpcReference => Initial::RpcReference {
                        rpc: read_rpc(&mut r)?,
                        reference: r.reference()?,
                        target_selector: r.take(9)? as u16,
                    },
                    Kind::RpcFlags => Initial::RpcFlags {
                        rpc: read_rpc(&mut r)?,
                        flags: [r.bit()?, r.bit()?],
                    },
                    Kind::BoolProperty => Initial::BoolProperty(r.optional(Reader::bit)?),
                    Kind::I32Property => {
                        Initial::I32Property(r.optional(|r| Ok(r.take(32)? as i32))?)
                    }
                    Kind::RpcVariant => Initial::RpcVariant {
                        rpc: read_rpc(&mut r)?,
                        value: r.optional(|r| {
                            let tag = r.take(8)? as u8;
                            let value = if tag <= 2 { Some(r.take(32)?) } else { None };
                            Ok((tag, value))
                        })?,
                    },
                    Kind::RpcGhostReference => Initial::RpcGhostReference {
                        rpc: read_rpc(&mut r)?,
                        reference: r.reference()?,
                    },
                    Kind::RpcOptionalReference => Initial::RpcOptionalReference {
                        rpc: read_rpc(&mut r)?,
                        reference: r.optional(Reader::reference)?,
                    },
                });
            }
            Some(values)
        } else {
            None
        };
        let initial_bits = creation.then_some(r.pos);
        let mut updates = Vec::with_capacity(profile.0.len());
        for kind in &profile.0 {
            let present = profile.0.len() == 1 || r.bit()?;
            updates.push(if present {
                Some(match kind {
                    Kind::Root
                    | Kind::SequenceRoot
                    | Kind::Rpc
                    | Kind::RpcReference
                    | Kind::RpcFlags
                    | Kind::RpcVariant
                    | Kind::RpcOptionalReference => Update::Noop,
                    Kind::RpcGhostReference => {
                        Update::GhostReference(r.optional(Reader::reference)?)
                    }
                    Kind::BoolProperty => Update::BoolProperty(r.optional(Reader::bit)?),
                    Kind::I32Property => {
                        Update::I32Property(r.optional(|r| Ok(r.take(32)? as i32))?)
                    }
                })
            } else {
                None
            });
        }
        Ok(Decoded {
            body: Body { initial, updates },
            initial_bits,
            bits: r.pos,
        })
    }

    pub fn encode(&self, profile: &Profile) -> Result<BitWriter, Error> {
        if self.updates.len() != profile.0.len()
            || self
                .initial
                .as_ref()
                .is_some_and(|v| v.len() != profile.0.len())
        {
            return Err(Error::Shape);
        }
        let mut w = Writer(BitWriter::new());
        if let Some(values) = &self.initial {
            for (value, kind) in values.iter().zip(&profile.0) {
                if value.kind() != *kind {
                    return Err(Error::TypeMismatch);
                }
                match value {
                    Initial::Root {
                        value,
                        rpc,
                        reference,
                    } => {
                        w.put(u64::from(*value), 16)?;
                        write_rpc(&mut w, rpc)?;
                        w.reference(reference)?;
                    }
                    Initial::Rpc(rpc) => write_rpc(&mut w, rpc)?,
                    Initial::SequenceRoot {
                        value,
                        rpc,
                        manager,
                        manager_selector,
                        sequence,
                        participants,
                        stopping,
                    } => {
                        if participants.len() > 255 {
                            return Err(Error::Bound);
                        }
                        w.put(u64::from(*value), 16)?;
                        write_rpc(&mut w, rpc)?;
                        w.reference(manager)?;
                        w.put(u64::from(*manager_selector), 9)?;
                        w.put(u64::from(*sequence), 32)?;
                        w.put(participants.len() as u64, 8)?;
                        for participant in participants {
                            w.reference(participant)?;
                        }
                        w.bit(*stopping)?;
                    }
                    Initial::RpcReference {
                        rpc,
                        reference,
                        target_selector,
                    } => {
                        write_rpc(&mut w, rpc)?;
                        w.reference(reference)?;
                        w.put(u64::from(*target_selector), 9)?;
                    }
                    Initial::RpcFlags { rpc, flags } => {
                        write_rpc(&mut w, rpc)?;
                        w.bit(flags[0])?;
                        w.bit(flags[1])?;
                    }
                    Initial::BoolProperty(v) => w.optional(v, |w, v| w.bit(*v))?,
                    Initial::I32Property(v) => {
                        w.optional(v, |w, v| w.put(u64::from(*v as u32), 32))?
                    }
                    Initial::RpcVariant { rpc, value } => {
                        write_rpc(&mut w, rpc)?;
                        w.optional(value, |w, (tag, value)| {
                            if (*tag <= 2) != value.is_some() {
                                return Err(Error::Shape);
                            }
                            w.put(u64::from(*tag), 8)?;
                            if let Some(value) = value {
                                w.put(u64::from(*value), 32)?;
                            }
                            Ok(())
                        })?
                    }
                    Initial::RpcGhostReference { rpc, reference } => {
                        write_rpc(&mut w, rpc)?;
                        w.reference(reference)?;
                    }
                    Initial::RpcOptionalReference { rpc, reference } => {
                        write_rpc(&mut w, rpc)?;
                        w.optional(reference, Writer::reference)?;
                    }
                }
            }
        }
        for (value, kind) in self.updates.iter().zip(&profile.0) {
            if profile.0.len() > 1 {
                w.bit(value.is_some())?;
            } else if value.is_none() {
                return Err(Error::Shape);
            }
            if let Some(value) = value {
                if !value.matches(*kind) {
                    return Err(Error::TypeMismatch);
                }
                match value {
                    Update::Noop => (),
                    Update::GhostReference(v) => w.optional(v, Writer::reference)?,
                    Update::BoolProperty(v) => w.optional(v, |w, v| w.bit(*v))?,
                    Update::I32Property(v) => {
                        w.optional(v, |w, v| w.put(u64::from(*v as u32), 32))?
                    }
                }
            }
        }
        Ok(w.0)
    }
}

#[cfg(test)]
mod sequence_tests;

pub mod creation;
pub mod state;
