// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Reader, Writer};
use crate::bits::BitWriter;
use nfs_protocol::world::{BitSpan, rpc::Serial};

pub const MAX_SERIALIZERS: usize = 512;
pub const MAX_REFERENCES: usize = 1024;
/// Bound of a synced string's 10-bit byte length.
pub const MAX_STRING_BYTES: usize = 1023;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Root,
    SequenceRoot,
    Rpc,
    RpcReference,
    RpcFlags,
    /// RPC identity followed by one initialization Boolean; updates have no body.
    RpcBool,
    /// RPC identity, a 16-bit count and 13-bit references; updates have no body.
    RpcReferences,
    BoolProperty,
    FloatProperty,
    I32Property,
    RpcVariant,
    RpcGhostReference,
    RpcOptionalReference,
    /// RPC identity followed by an optional participant reference and signed
    /// 32-bit value. The sorter's query-by-index snapshot uses this pair.
    RpcOptionalReferenceI32,
    /// Optional byte string with a 10-bit length, in creations and updates.
    StringProperty,
    /// Game-mode root (client reader `0x14404c330`): bus count, RPC, the f64 start
    /// time as 64 bits, an f32 the client adds to it, three (reference, selector)
    /// bindings, two 8-bit-count reference lists and a flag.
    GameModeRoot,
    /// Countdown state (`0x444feb0`): RPC, 16-bit-count participant references and an
    /// optional 64-bit start time.
    CountdownState,
    /// Waypoints state (`0x4465310`): RPC, 16-bit-count participant references, three
    /// 32-bit words and an 8-bit-count record list. Only the empty list is modeled.
    WaypointsState,
    /// DNF transition (`0x4464d90`): RPC and an 8-bit-count list of (participant
    /// reference, 64-bit value, high word first) (`0x14445f6d0`).
    DnfTransition,
    /// Channel waypoint list provider (`0x446b480`): RPC, a 4-bit lap count and an
    /// 8-bit-count list of (scene reference, 9-bit selector).
    WaypointList,
    /// RPC and an optional 128-bit identifier (`0x42150f0`), as the scene codec reads it.
    RpcGuid,
    /// RPC whose update is the scene codec's optional set: an optional 7-bit-count list
    /// of 64-bit values, low word first.
    RpcSet64,
}

pub const MAX_WAYPOINTS: usize = 255;
pub const MAX_SET64: usize = 127;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile(Vec<Kind>);
impl Profile {
    pub fn new(kinds: &[Kind]) -> Result<Self, Error> {
        if kinds.is_empty() || kinds.len() > MAX_SERIALIZERS {
            return Err(Error::Bound);
        }
        let root =
            |kind: &Kind| matches!(kind, Kind::Root | Kind::SequenceRoot | Kind::GameModeRoot);
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

/// The game-mode root's initial values. `start` holds f64 bits on the host's
/// replication clock; `age` holds f32 bits that the client adds to `start`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameModeRoot {
    pub value: u16,
    pub rpc: Rpc,
    pub start: u64,
    pub age: u32,
    pub bindings: [(u16, u16); 3],
    pub participants: Vec<u16>,
    pub others: Vec<u16>,
    pub flag: bool,
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
    RpcBool {
        rpc: Rpc,
        value: bool,
    },
    RpcReferences {
        rpc: Rpc,
        references: Vec<u16>,
    },
    BoolProperty(Option<bool>),
    /// IEEE-754 bits, preserving NaN payloads and signed zero.
    FloatProperty(Option<u32>),
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
    RpcOptionalReferenceI32 {
        rpc: Rpc,
        value: Option<(u16, i32)>,
    },
    StringProperty(Option<Vec<u8>>),
    GameModeRoot(Box<GameModeRoot>),
    CountdownState {
        rpc: Rpc,
        references: Vec<u16>,
        start: Option<u64>,
    },
    WaypointsState {
        rpc: Rpc,
        references: Vec<u16>,
        words: [u32; 3],
    },
    DnfTransition {
        rpc: Rpc,
        values: Vec<(u16, u64)>,
    },
    WaypointList {
        rpc: Rpc,
        laps: u8,
        waypoints: Vec<(u16, u16)>,
    },
    RpcGuid {
        rpc: Rpc,
        guid: Option<[u8; 16]>,
    },
    RpcSet64(Rpc),
}
impl Initial {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Root { .. } => Kind::Root,
            Self::SequenceRoot { .. } => Kind::SequenceRoot,
            Self::Rpc(_) => Kind::Rpc,
            Self::RpcReference { .. } => Kind::RpcReference,
            Self::RpcFlags { .. } => Kind::RpcFlags,
            Self::RpcBool { .. } => Kind::RpcBool,
            Self::RpcReferences { .. } => Kind::RpcReferences,
            Self::BoolProperty(_) => Kind::BoolProperty,
            Self::FloatProperty(_) => Kind::FloatProperty,
            Self::I32Property(_) => Kind::I32Property,
            Self::RpcVariant { .. } => Kind::RpcVariant,
            Self::RpcGhostReference { .. } => Kind::RpcGhostReference,
            Self::RpcOptionalReference { .. } => Kind::RpcOptionalReference,
            Self::RpcOptionalReferenceI32 { .. } => Kind::RpcOptionalReferenceI32,
            Self::StringProperty(_) => Kind::StringProperty,
            Self::GameModeRoot(_) => Kind::GameModeRoot,
            Self::CountdownState { .. } => Kind::CountdownState,
            Self::WaypointsState { .. } => Kind::WaypointsState,
            Self::DnfTransition { .. } => Kind::DnfTransition,
            Self::WaypointList { .. } => Kind::WaypointList,
            Self::RpcGuid { .. } => Kind::RpcGuid,
            Self::RpcSet64(_) => Kind::RpcSet64,
        }
    }
    /// The RPC identity, when this initial value carries one.
    pub fn rpc(&self) -> Option<&Rpc> {
        match self {
            Self::Root { rpc, .. }
            | Self::SequenceRoot { rpc, .. }
            | Self::Rpc(rpc)
            | Self::RpcReference { rpc, .. }
            | Self::RpcFlags { rpc, .. }
            | Self::RpcBool { rpc, .. }
            | Self::RpcReferences { rpc, .. }
            | Self::RpcVariant { rpc, .. }
            | Self::RpcGhostReference { rpc, .. }
            | Self::RpcOptionalReference { rpc, .. }
            | Self::RpcOptionalReferenceI32 { rpc, .. }
            | Self::CountdownState { rpc, .. }
            | Self::WaypointsState { rpc, .. }
            | Self::DnfTransition { rpc, .. }
            | Self::WaypointList { rpc, .. }
            | Self::RpcGuid { rpc, .. }
            | Self::RpcSet64(rpc) => Some(rpc),
            Self::GameModeRoot(root) => Some(&root.rpc),
            Self::BoolProperty(_)
            | Self::FloatProperty(_)
            | Self::I32Property(_)
            | Self::StringProperty(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Noop,
    BoolProperty(Option<bool>),
    FloatProperty(Option<u32>),
    I32Property(Option<i32>),
    GhostReference(Option<u16>),
    StringProperty(Option<Vec<u8>>),
    /// The scene codec's optional set of 64-bit values.
    Set64(Option<super::sublevel::Set64>),
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
                    | Kind::RpcBool
                    | Kind::RpcReferences
                    | Kind::RpcVariant
                    | Kind::RpcOptionalReference
                    | Kind::RpcOptionalReferenceI32
                    | Kind::GameModeRoot
                    | Kind::CountdownState
                    | Kind::WaypointsState
                    | Kind::DnfTransition
                    | Kind::WaypointList
                    | Kind::RpcGuid
            ) | (Self::BoolProperty(_), Kind::BoolProperty)
                | (Self::FloatProperty(_), Kind::FloatProperty)
                | (Self::GhostReference(_), Kind::RpcGhostReference)
                | (Self::I32Property(_), Kind::I32Property)
                | (Self::StringProperty(_), Kind::StringProperty)
                | (Self::Set64(_), Kind::RpcSet64)
        )
    }
}

fn read_u64(r: &mut Reader<'_>) -> Result<u64, Error> {
    let high = u64::from(r.take(32)?);
    Ok((high << 32) | u64::from(r.take(32)?))
}
fn write_u64(w: &mut Writer, v: u64) -> Result<(), Error> {
    w.put(v >> 32, 32)?;
    w.put(v & 0xffff_ffff, 32)
}
fn read_wide_references(r: &mut Reader<'_>) -> Result<Vec<u16>, Error> {
    let count = r.take(16)? as usize;
    if count > MAX_REFERENCES {
        return Err(Error::Bound);
    }
    (0..count).map(|_| r.reference()).collect()
}
fn write_wide_references(w: &mut Writer, references: &[u16]) -> Result<(), Error> {
    if references.len() > MAX_REFERENCES {
        return Err(Error::Bound);
    }
    w.put(references.len() as u64, 16)?;
    references.iter().try_for_each(|r| w.reference(r))
}
fn read_set64(r: &mut Reader<'_>) -> Result<Option<super::sublevel::Set64>, Error> {
    r.optional(|r| {
        Ok(super::sublevel::Set64 {
            entries: r.optional(|r| {
                let count = r.take(7)? as usize;
                (0..count)
                    .map(|_| Ok(u64::from(r.take(32)?) | (u64::from(r.take(32)?) << 32)))
                    .collect()
            })?,
        })
    })
}
fn write_set64(w: &mut Writer, set: &Option<super::sublevel::Set64>) -> Result<(), Error> {
    w.optional(set, |w, set| {
        w.optional(&set.entries, |w, values| {
            if values.len() > MAX_SET64 {
                return Err(Error::Bound);
            }
            w.put(values.len() as u64, 7)?;
            values.iter().try_for_each(|v| {
                w.put(u64::from(*v as u32), 32)?;
                w.put(*v >> 32, 32)
            })
        })
    })
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
                    Kind::RpcBool => Initial::RpcBool {
                        rpc: read_rpc(&mut r)?,
                        value: r.bit()?,
                    },
                    Kind::RpcReferences => {
                        let rpc = read_rpc(&mut r)?;
                        let count = r.take(16)? as usize;
                        if count > MAX_REFERENCES {
                            return Err(Error::Bound);
                        }
                        let references = (0..count)
                            .map(|_| r.reference())
                            .collect::<Result<_, _>>()?;
                        Initial::RpcReferences { rpc, references }
                    }
                    Kind::BoolProperty => Initial::BoolProperty(r.optional(Reader::bit)?),
                    Kind::FloatProperty => Initial::FloatProperty(r.optional(|r| r.take(32))?),
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
                    Kind::RpcOptionalReferenceI32 => Initial::RpcOptionalReferenceI32 {
                        rpc: read_rpc(&mut r)?,
                        value: r.optional(|r| Ok((r.reference()?, r.take(32)? as i32)))?,
                    },
                    Kind::StringProperty => {
                        Initial::StringProperty(r.optional(|r| r.string(10, MAX_STRING_BYTES))?)
                    }
                    Kind::GameModeRoot => {
                        let value = r.take(16)? as u16;
                        let rpc = read_rpc(&mut r)?;
                        let start = read_u64(&mut r)?;
                        let age = r.take(32)?;
                        let mut bindings = [(0, 0); 3];
                        for binding in &mut bindings {
                            *binding = (r.reference()?, r.take(9)? as u16);
                        }
                        Initial::GameModeRoot(Box::new(GameModeRoot {
                            value,
                            rpc,
                            start,
                            age,
                            bindings,
                            participants: r.references()?,
                            others: r.references()?,
                            flag: r.bit()?,
                        }))
                    }
                    Kind::CountdownState => Initial::CountdownState {
                        rpc: read_rpc(&mut r)?,
                        references: read_wide_references(&mut r)?,
                        start: r.optional(read_u64)?,
                    },
                    Kind::WaypointsState => {
                        let rpc = read_rpc(&mut r)?;
                        let references = read_wide_references(&mut r)?;
                        let words = [r.take(32)?, r.take(32)?, r.take(32)?];
                        // Record elements (three bytes, a value and a car ID per the
                        // client reader) are not modeled; official creations hold none.
                        if r.take(8)? != 0 {
                            return Err(Error::Unsupported);
                        }
                        Initial::WaypointsState {
                            rpc,
                            references,
                            words,
                        }
                    }
                    Kind::DnfTransition => {
                        let rpc = read_rpc(&mut r)?;
                        let count = r.take(8)? as usize;
                        Initial::DnfTransition {
                            rpc,
                            values: (0..count)
                                .map(|_| Ok((r.reference()?, read_u64(&mut r)?)))
                                .collect::<Result<_, Error>>()?,
                        }
                    }
                    Kind::WaypointList => {
                        let rpc = read_rpc(&mut r)?;
                        let laps = r.take(4)? as u8;
                        let count = r.take(8)? as usize;
                        Initial::WaypointList {
                            rpc,
                            laps,
                            waypoints: (0..count)
                                .map(|_| Ok((r.reference()?, r.take(9)? as u16)))
                                .collect::<Result<_, Error>>()?,
                        }
                    }
                    Kind::RpcGuid => Initial::RpcGuid {
                        rpc: read_rpc(&mut r)?,
                        guid: r.optional(|r| {
                            let mut bytes = [0; 16];
                            for byte in &mut bytes {
                                *byte = r.take(8)? as u8;
                            }
                            Ok(bytes)
                        })?,
                    },
                    Kind::RpcSet64 => Initial::RpcSet64(read_rpc(&mut r)?),
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
                    | Kind::RpcBool
                    | Kind::RpcReferences
                    | Kind::RpcVariant
                    | Kind::RpcOptionalReference
                    | Kind::RpcOptionalReferenceI32
                    | Kind::GameModeRoot
                    | Kind::CountdownState
                    | Kind::WaypointsState
                    | Kind::DnfTransition
                    | Kind::WaypointList
                    | Kind::RpcGuid => Update::Noop,
                    Kind::RpcSet64 => Update::Set64(read_set64(&mut r)?),
                    Kind::RpcGhostReference => {
                        Update::GhostReference(r.optional(Reader::reference)?)
                    }
                    Kind::BoolProperty => Update::BoolProperty(r.optional(Reader::bit)?),
                    Kind::FloatProperty => Update::FloatProperty(r.optional(|r| r.take(32))?),
                    Kind::I32Property => {
                        Update::I32Property(r.optional(|r| Ok(r.take(32)? as i32))?)
                    }
                    Kind::StringProperty => {
                        Update::StringProperty(r.optional(|r| r.string(10, MAX_STRING_BYTES))?)
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
                    Initial::RpcBool { rpc, value } => {
                        write_rpc(&mut w, rpc)?;
                        w.bit(*value)?;
                    }
                    Initial::RpcReferences { rpc, references } => {
                        if references.len() > MAX_REFERENCES {
                            return Err(Error::Bound);
                        }
                        write_rpc(&mut w, rpc)?;
                        w.put(references.len() as u64, 16)?;
                        for reference in references {
                            w.reference(reference)?;
                        }
                    }
                    Initial::BoolProperty(v) => w.optional(v, |w, v| w.bit(*v))?,
                    Initial::FloatProperty(v) => w.optional(v, |w, v| w.put(u64::from(*v), 32))?,
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
                    Initial::RpcOptionalReferenceI32 { rpc, value } => {
                        write_rpc(&mut w, rpc)?;
                        w.optional(value, |w, (reference, value)| {
                            w.reference(reference)?;
                            w.put(u64::from(*value as u32), 32)
                        })?;
                    }
                    Initial::StringProperty(v) => {
                        w.optional(v, |w, v| w.string(v, 10, MAX_STRING_BYTES))?
                    }
                    Initial::GameModeRoot(root) => {
                        w.put(u64::from(root.value), 16)?;
                        write_rpc(&mut w, &root.rpc)?;
                        write_u64(&mut w, root.start)?;
                        w.put(u64::from(root.age), 32)?;
                        for (reference, selector) in &root.bindings {
                            w.reference(reference)?;
                            w.put(u64::from(*selector), 9)?;
                        }
                        if root.participants.len() > 255 || root.others.len() > 255 {
                            return Err(Error::Bound);
                        }
                        w.references(&root.participants)?;
                        w.references(&root.others)?;
                        w.bit(root.flag)?;
                    }
                    Initial::CountdownState {
                        rpc,
                        references,
                        start,
                    } => {
                        write_rpc(&mut w, rpc)?;
                        write_wide_references(&mut w, references)?;
                        w.optional(start, |w, v| write_u64(w, *v))?;
                    }
                    Initial::WaypointsState {
                        rpc,
                        references,
                        words,
                    } => {
                        write_rpc(&mut w, rpc)?;
                        write_wide_references(&mut w, references)?;
                        for word in words {
                            w.put(u64::from(*word), 32)?;
                        }
                        w.put(0, 8)?;
                    }
                    Initial::DnfTransition { rpc, values } => {
                        if values.len() > 255 {
                            return Err(Error::Bound);
                        }
                        write_rpc(&mut w, rpc)?;
                        w.put(values.len() as u64, 8)?;
                        for (participant, v) in values {
                            w.reference(participant)?;
                            write_u64(&mut w, *v)?;
                        }
                    }
                    Initial::WaypointList {
                        rpc,
                        laps,
                        waypoints,
                    } => {
                        if waypoints.len() > MAX_WAYPOINTS {
                            return Err(Error::Bound);
                        }
                        write_rpc(&mut w, rpc)?;
                        w.put(u64::from(*laps), 4)?;
                        w.put(waypoints.len() as u64, 8)?;
                        for (reference, selector) in waypoints {
                            w.reference(reference)?;
                            w.put(u64::from(*selector), 9)?;
                        }
                    }
                    Initial::RpcGuid { rpc, guid } => {
                        write_rpc(&mut w, rpc)?;
                        w.optional(guid, |w, bytes| {
                            bytes.iter().try_for_each(|b| w.put(u64::from(*b), 8))
                        })?;
                    }
                    Initial::RpcSet64(rpc) => write_rpc(&mut w, rpc)?,
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
                    Update::FloatProperty(v) => w.optional(v, |w, v| w.put(u64::from(*v), 32))?,
                    Update::I32Property(v) => {
                        w.optional(v, |w, v| w.put(u64::from(*v as u32), 32))?
                    }
                    Update::StringProperty(v) => {
                        w.optional(v, |w, v| w.string(v, 10, MAX_STRING_BYTES))?
                    }
                    Update::Set64(set) => write_set64(&mut w, set)?,
                }
            }
        }
        Ok(w.0)
    }
}

#[cfg(test)]
mod float_tests;
#[cfg(test)]
mod game_mode_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod sequence_tests;

pub mod creation;
pub mod state;
