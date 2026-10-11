// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Initial, Kind, MAX_REFERENCES, Reader, Rpc, Set64, Update, Writer};
use nfs_protocol::world::rpc::Serial;

fn rpc(r: &mut Reader<'_>) -> Result<Rpc, Error> {
    Ok(Rpc {
        selector: r.take(9)? as u16,
        serial: Serial::new(r.take(10)? as u16).ok_or(Error::Shape)?,
    })
}
fn put_rpc(w: &mut Writer, rpc: &Rpc) -> Result<(), Error> {
    w.put(u64::from(rpc.selector), 9)?;
    w.put(u64::from(rpc.serial.value()), 10)
}
fn list<T>(
    r: &mut Reader<'_>,
    width: u8,
    read: impl Fn(&mut Reader<'_>) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let count = r.take(width)? as usize;
    if count > MAX_REFERENCES {
        return Err(Error::Bound);
    }
    (0..count).map(|_| read(r)).collect()
}
fn put_list<T>(
    w: &mut Writer,
    values: &[T],
    width: u8,
    put: impl Fn(&mut Writer, &T) -> Result<(), Error>,
) -> Result<(), Error> {
    if values.len() > MAX_REFERENCES {
        return Err(Error::Bound);
    }
    w.put(values.len() as u64, usize::from(width))?;
    for value in values {
        put(w, value)?;
    }
    Ok(())
}
fn reference_map(r: &mut Reader<'_>) -> Result<Vec<(u16, u32)>, Error> {
    list(r, 8, |r| Ok((r.reference()?, r.take(32)?)))
}
fn put_map(w: &mut Writer, values: &[(u16, u32)]) -> Result<(), Error> {
    put_list(w, values, 8, |w, v| {
        w.reference(&v.0)?;
        w.put(u64::from(v.1), 32)
    })
}
pub(super) fn read_initial(r: &mut Reader<'_>, kind: Kind) -> Result<Initial, Error> {
    Ok(match kind {
        Kind::Noop | Kind::LevelReference => Initial::Empty,
        Kind::BoolProperty => Initial::BoolProperty(r.optional(Reader::bit)?),
        Kind::FloatProperty => Initial::FloatProperty(r.optional(|r| r.take(32))?),
        Kind::I32Property => Initial::I32Property(r.optional(|r| Ok(r.take(32)? as i32))?),
        Kind::Rpc | Kind::RpcFourReferences | Kind::RpcSet64 | Kind::RpcPursuitMaps => {
            Initial::Rpc(rpc(r)?)
        }
        Kind::RpcSpawn => {
            let rpc = rpc(r)?;
            let references = list(r, 16, |r| r.reference())?;
            if r.bit()? {
                return Err(Error::Unsupported);
            }
            Initial::RpcSpawnInactive { rpc, references }
        }
        Kind::RpcReferences => Initial::RpcReferences {
            rpc: rpc(r)?,
            value: list(r, 16, |r| r.reference())?,
        },
        Kind::RpcPairs => Initial::RpcPairs {
            rpc: rpc(r)?,
            value: list(r, 16, |r| Ok((r.reference()?, r.reference()?)))?,
        },
        Kind::RpcTagged => Initial::RpcTagged {
            rpc: rpc(r)?,
            value: list(r, 16, |r| Ok((r.reference()?, r.take(8)? as i8)))?,
        },
        Kind::RpcOptional64 => Initial::RpcOptional64 {
            rpc: rpc(r)?,
            value: r.optional(|r| Ok((u64::from(r.take(32)?) << 32) | u64::from(r.take(32)?)))?,
        },
        Kind::RpcGuid => Initial::RpcGuid {
            rpc: rpc(r)?,
            value: r.optional(|r| {
                let mut bytes = [0; 16];
                for byte in &mut bytes {
                    *byte = r.take(8)? as u8;
                }
                Ok(bytes)
            })?,
        },
        Kind::RpcBool => Initial::RpcBool {
            rpc: rpc(r)?,
            value: r.bit()?,
        },
        Kind::RpcMap => Initial::RpcMap {
            rpc: rpc(r)?,
            value: reference_map(r)?,
        },
    })
}
pub(super) fn write_initial(w: &mut Writer, kind: Kind, value: &Initial) -> Result<(), Error> {
    match (kind, value) {
        (Kind::Noop | Kind::LevelReference, Initial::Empty) => Ok(()),
        (
            Kind::Rpc | Kind::RpcFourReferences | Kind::RpcSet64 | Kind::RpcPursuitMaps,
            Initial::Rpc(rpc),
        ) => put_rpc(w, rpc),
        (Kind::BoolProperty, Initial::BoolProperty(v)) => w.optional(v, |w, v| w.bit(*v)),
        (Kind::FloatProperty, Initial::FloatProperty(v)) => {
            w.optional(v, |w, v| w.put(u64::from(*v), 32))
        }
        (Kind::I32Property, Initial::I32Property(v)) => {
            w.optional(v, |w, v| w.put(u64::from(*v as u32), 32))
        }
        (Kind::RpcReferences, Initial::RpcReferences { rpc, value }) => {
            put_rpc(w, rpc)?;
            put_list(w, value, 16, Writer::reference)
        }
        (Kind::RpcPairs, Initial::RpcPairs { rpc, value }) => {
            put_rpc(w, rpc)?;
            put_list(w, value, 16, |w, v| {
                w.reference(&v.0)?;
                w.reference(&v.1)
            })
        }
        (Kind::RpcTagged, Initial::RpcTagged { rpc, value }) => {
            put_rpc(w, rpc)?;
            put_list(w, value, 16, |w, v| {
                w.reference(&v.0)?;
                w.put(u64::from(v.1 as u8), 8)
            })
        }
        (Kind::RpcOptional64, Initial::RpcOptional64 { rpc, value }) => {
            put_rpc(w, rpc)?;
            w.optional(value, |w, v| {
                w.put(*v >> 32, 32)?;
                w.put(u64::from(*v as u32), 32)
            })
        }
        (Kind::RpcGuid, Initial::RpcGuid { rpc, value }) => {
            put_rpc(w, rpc)?;
            w.optional(value, |w, v| {
                for byte in v {
                    w.put(u64::from(*byte), 8)?;
                }
                Ok(())
            })
        }
        (Kind::RpcBool, Initial::RpcBool { rpc, value }) => {
            put_rpc(w, rpc)?;
            w.bit(*value)
        }
        (Kind::RpcMap, Initial::RpcMap { rpc, value }) => {
            put_rpc(w, rpc)?;
            put_map(w, value)
        }
        (Kind::RpcSpawn, Initial::RpcSpawnInactive { rpc, references }) => {
            put_rpc(w, rpc)?;
            put_list(w, references, 16, Writer::reference)?;
            w.bit(false)
        }
        _ => Err(Error::TypeMismatch),
    }
}
pub(super) fn read_update(r: &mut Reader<'_>, kind: Kind) -> Result<Update, Error> {
    Ok(match kind {
        Kind::LevelReference => Update::LevelReference(r.optional(|r| Ok(r.take(16)? as u16))?),
        Kind::BoolProperty => Update::BoolProperty(r.optional(Reader::bit)?),
        Kind::FloatProperty => Update::FloatProperty(r.optional(|r| r.take(32))?),
        Kind::I32Property => Update::I32Property(r.optional(|r| Ok(r.take(32)? as i32))?),
        Kind::RpcMap => Update::ReferenceMap(r.optional(reference_map)?),
        Kind::RpcFourReferences => {
            let mut values = [None; 4];
            for v in &mut values {
                *v = r.optional(Reader::reference)?;
            }
            Update::FourReferences(values)
        }
        Kind::RpcSet64 => Update::Set64(r.optional(|r| {
            Ok(Set64 {
                entries: r.optional(|r| {
                    list(r, 7, |r| {
                        Ok(u64::from(r.take(32)?) | (u64::from(r.take(32)?) << 32))
                    })
                })?,
            })
        })?),
        Kind::RpcPursuitMaps => Update::PursuitMaps([
            r.optional(|r| list(r, 4, |r| Ok((r.take(32)?, r.reference()?))))?,
            r.optional(|r| list(r, 8, |r| Ok((r.take(32)?, r.reference()?))))?,
        ]),
        Kind::Noop
        | Kind::Rpc
        | Kind::RpcReferences
        | Kind::RpcPairs
        | Kind::RpcTagged
        | Kind::RpcOptional64
        | Kind::RpcGuid
        | Kind::RpcBool
        | Kind::RpcSpawn => Update::Noop,
    })
}
pub(super) fn write_update(w: &mut Writer, kind: Kind, value: &Update) -> Result<(), Error> {
    match (kind, value) {
        (Kind::LevelReference, Update::LevelReference(v)) => {
            w.optional(v, |w, v| w.put(u64::from(*v), 16))
        }
        (Kind::BoolProperty, Update::BoolProperty(v)) => w.optional(v, |w, v| w.bit(*v)),
        (Kind::FloatProperty, Update::FloatProperty(v)) => {
            w.optional(v, |w, v| w.put(u64::from(*v), 32))
        }
        (Kind::I32Property, Update::I32Property(v)) => {
            w.optional(v, |w, v| w.put(u64::from(*v as u32), 32))
        }
        (Kind::RpcMap, Update::ReferenceMap(v)) => w.optional(v, |w, v| put_map(w, v)),
        (Kind::RpcFourReferences, Update::FourReferences(v)) => {
            for v in v {
                w.optional(v, Writer::reference)?;
            }
            Ok(())
        }
        (Kind::RpcSet64, Update::Set64(v)) => w.optional(v, |w, v| {
            w.optional(&v.entries, |w, v| {
                put_list(w, v, 7, |w, v| {
                    w.put(u64::from(*v as u32), 32)?;
                    w.put(*v >> 32, 32)
                })
            })
        }),
        (Kind::RpcPursuitMaps, Update::PursuitMaps(maps)) => {
            for (value, width) in maps.iter().zip([4, 8]) {
                w.optional(value, |w, values| {
                    put_list(w, values, width, |w, v| {
                        w.put(u64::from(v.0), 32)?;
                        w.reference(&v.1)
                    })
                })?;
            }
            Ok(())
        }
        (
            Kind::Noop
            | Kind::Rpc
            | Kind::RpcReferences
            | Kind::RpcPairs
            | Kind::RpcTagged
            | Kind::RpcOptional64
            | Kind::RpcGuid
            | Kind::RpcBool
            | Kind::RpcSpawn,
            Update::Noop,
        ) => Ok(()),
        _ => Err(Error::TypeMismatch),
    }
}
