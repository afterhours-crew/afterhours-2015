// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::entity::Rpc;
use super::{Error, Reader, Writer};
use crate::bits::BitWriter;
use nfs_protocol::world::{BitSpan, rpc::Serial};
pub mod appearance;
pub mod authority;
pub mod chassis;
pub mod components;
pub mod customization;
pub mod health;
pub mod initialize;
pub mod lifecycle;
pub mod mesh;
pub mod nos;
pub mod orientation;
mod parts;
pub mod placement;
mod position;
mod primitives;
pub mod root;
mod rotation;
pub mod tuning;
pub mod wheel_customization;
pub use primitives::{Axis25, Quaternion14, Quaternion25, Resource, SparseVector, Vector};
use primitives::{array, put_signed, signed, sparse_float_read, sparse_float_write};
mod codec;
pub mod creation;
pub use creation::EntityCreation;
pub mod state;

pub const MAX_SERIALIZERS: usize = 512;
pub const MAX_MESHES: usize = 128;
pub const MAX_MESH_ASSETS: usize = 256;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshExtra {
    None,
    Rim,
    Light,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeshKind {
    pub index_bits: u8,
    pub extra: MeshExtra,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    Root { property_owner: bool },
    Chassis,
    Part { variants: u16 },
    TripleNibbles,
    Bool,
    Wheel,
    Index,
    Tuning,
    Tagged,
    NibblesGuid,
    FourBit,
    Appearance,
    Mesh(Vec<MeshKind>),
    GuidFloat,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile(Vec<Kind>);
impl Profile {
    pub fn new(kinds: Vec<Kind>) -> Result<Self, Error> {
        if kinds.is_empty() || kinds.len() > MAX_SERIALIZERS {
            return Err(Error::Bound);
        }
        if !matches!(kinds[0], Kind::Root { .. })
            || kinds[1..].iter().any(|k| matches!(k, Kind::Root { .. }))
        {
            return Err(Error::Shape);
        }
        for k in &kinds {
            match k {
                Kind::Part { variants } if !(1..=256).contains(variants) => {
                    return Err(Error::Bound);
                }
                Kind::Mesh(entries)
                    if entries.len() > MAX_MESHES
                        || entries.iter().any(|e| !(1..=16).contains(&e.index_bits)) =>
                {
                    return Err(Error::Bound);
                }
                _ => (),
            }
        }
        Ok(Self(kinds))
    }
    pub fn kinds(&self) -> &[Kind] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Setup {
    pub flags: Option<Vec<bool>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootInitial {
    pub property_value: Option<u16>,
    pub position: Vector,
    pub rotation: Quaternion25,
    pub flag: bool,
    pub reference: Option<u16>,
    pub rpc: Rpc,
    pub fine_position: Vector,
    pub fine_rotation: Quaternion25,
    pub fine_flag: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Customization {
    pub value: u8,
    pub list_a: Vec<u8>,
    pub list_b: Vec<u8>,
    pub index: Option<u8>,
    pub values: [u16; 2],
    pub flag: bool,
    pub tuning: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChassisInitial {
    pub rpc: Rpc,
    pub vectors: [[Option<u32>; 3]; 2],
    pub custom: Customization,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Initial {
    Root(Box<RootInitial>),
    Chassis(Box<ChassisInitial>),
    Part(bool),
    Rpc(Rpc),
    Empty,
    Wheel {
        rpc: Rpc,
        flag: bool,
    },
    Index {
        rpc: Rpc,
        index: Option<u8>,
        value: u8,
    },
    FourBit(u8),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Creation {
    pub setup: Option<Setup>,
    pub connection_id: u16,
    pub fields: Vec<Initial>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsControls {
    pub value3: u8,
    pub value6: u8,
    pub optional6: Option<u8>,
    pub flag: bool,
    pub values: [u16; 11],
    pub flags: [bool; 4],
    pub wheel_pairs: Option<[[u8; 2]; 4]>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Physics {
    pub identity_words: [u32; 3],
    pub position: Vector,
    pub rotation: Quaternion14,
    pub velocity: SparseVector,
    pub angular_velocity: SparseVector,
    pub controls: PhysicsControls,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChassisUpdate {
    pub physics: Option<Physics>,
    pub flag: Option<bool>,
    pub custom: Option<Customization>,
    pub pair: Option<(u32, bool)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tuning {
    pub values: [u16; 14],
    pub flags: [bool; 2],
    pub tail: [u16; 2],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tagged {
    pub tag: u8,
    pub reference: Option<u16>,
    pub pair: Option<[u32; 2]>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NibblesGuid {
    pub head: [u8; 3],
    pub guid: [u32; 4],
    pub tail: [u8; 6],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paint {
    pub colors: [[u8; 3]; 2],
    pub value: u8,
    pub name: Vec<u8>,
    pub flag: bool,
    pub resource: Resource,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Wrap {
    Preset([u8; 2]),
    Custom {
        words: [u32; 2],
        resource: Resource,
        flag: bool,
        value: i32,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Appearance {
    pub paint: Option<Paint>,
    pub palette1: Option<[[u8; 3]; 4]>,
    pub palette2: Option<[[u8; 3]; 4]>,
    pub wrap: Option<Wrap>,
    pub index: Option<u8>,
    pub indices: Option<[u8; 2]>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeshValue {
    None,
    Rim { first: i16, middle: u16, last: i16 },
    Light(u32),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mesh {
    pub index: i16,
    pub extra: MeshValue,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeshCustomization {
    pub meshes: Vec<Mesh>,
    pub assets: Vec<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Root(Option<u8>),
    Chassis(Box<ChassisUpdate>),
    Part(Option<u16>),
    TripleNibbles(Option<[u8; 3]>),
    Bool(Option<bool>),
    Wheel(Option<bool>),
    Index {
        index: Option<u8>,
        value: Option<u8>,
    },
    Tuning(Option<Tuning>),
    Tagged(Option<Tagged>),
    NibblesGuid(Option<NibblesGuid>),
    Noop,
    Appearance(Box<Appearance>),
    Mesh(Option<MeshCustomization>),
    GuidFloat {
        guid: Option<[u32; 4]>,
        value: Option<u32>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Body {
    pub creation: Option<Creation>,
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
            let setup = r.optional(|r| {
                Ok(Setup {
                    flags: r.optional(|r| {
                        let count = r.take(5)?;
                        (0..count).map(|_| r.bit()).collect()
                    })?,
                })
            })?;
            let connection_id = r.take(16)? as u16;
            let fields = profile
                .0
                .iter()
                .map(|k| codec::initial_read(&mut r, k))
                .collect::<Result<_, _>>()?;
            Some(Creation {
                setup,
                connection_id,
                fields,
            })
        } else {
            None
        };
        let initial_bits = creation.then_some(r.pos);
        let mut updates = Vec::with_capacity(profile.0.len());
        for k in &profile.0 {
            updates.push(if profile.0.len() == 1 || r.bit()? {
                Some(codec::update_read(&mut r, k)?)
            } else {
                None
            });
        }
        Ok(Decoded {
            body: Self {
                creation: initial,
                updates,
            },
            initial_bits,
            bits: r.pos,
        })
    }
    pub fn encode(&self, profile: &Profile) -> Result<BitWriter, Error> {
        if self.updates.len() != profile.0.len()
            || self
                .creation
                .as_ref()
                .is_some_and(|c| c.fields.len() != profile.0.len())
        {
            return Err(Error::Shape);
        }
        let mut w = Writer(BitWriter::new());
        if let Some(c) = &self.creation {
            w.optional(&c.setup, |w, s| {
                w.optional(&s.flags, |w, flags| {
                    if flags.len() > 31 {
                        return Err(Error::Bound);
                    }
                    w.put(flags.len() as u64, 5)?;
                    for f in flags {
                        w.bit(*f)?;
                    }
                    Ok(())
                })
            })?;
            w.put(u64::from(c.connection_id), 16)?;
            for (v, k) in c.fields.iter().zip(&profile.0) {
                codec::initial_write(&mut w, k, v)?;
            }
        }
        for (v, k) in self.updates.iter().zip(&profile.0) {
            if profile.0.len() > 1 {
                w.bit(v.is_some())?;
            } else if v.is_none() {
                return Err(Error::Shape);
            }
            if let Some(v) = v {
                codec::update_write(&mut w, k, v)?;
            }
        }
        Ok(w.0)
    }
}
