// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Content, Creation, Error, Initial, Kind, Prefix, Profile, Rpc, Set64, Update};
use nfs_protocol::world::rpc::Serial;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Noop,
    Rpc,
    BoolProperty(Option<bool>),
    FloatProperty(Option<u32>),
    I32Property(Option<i32>),
    References(Vec<u16>),
    Pairs(Vec<(u16, u16)>),
    Tagged(Vec<(u16, i8)>),
    Optional64(Option<u64>),
    Guid(Option<[u8; 16]>),
    Boolean(bool),
    Map(Vec<(u16, u32)>),
    FourReferences([Option<u16>; 4]),
    Identifiers(Option<Set64>),
    PursuitMaps([Option<Vec<(u32, u16)>>; 2]),
    InactiveSpawn(Vec<u16>),
}

pub fn unpopulated(profile: &Profile) -> Result<Vec<Value>, Error> {
    if profile.is_root() {
        return Err(Error::Unsupported);
    }
    profile
        .kinds()
        .iter()
        .map(|kind| {
            Ok(match kind {
                Kind::Noop => Value::Noop,
                Kind::Rpc => Value::Rpc,
                Kind::BoolProperty => Value::BoolProperty(None),
                Kind::FloatProperty => Value::FloatProperty(None),
                Kind::I32Property => Value::I32Property(None),
                Kind::RpcReferences => Value::References(Vec::new()),
                Kind::RpcPairs => Value::Pairs(Vec::new()),
                Kind::RpcTagged => Value::Tagged(Vec::new()),
                Kind::RpcOptional64 => Value::Optional64(None),
                Kind::RpcGuid => Value::Guid(None),
                Kind::RpcBool => Value::Boolean(false),
                Kind::RpcMap => Value::Map(Vec::new()),
                Kind::RpcFourReferences => Value::FourReferences([Some(0); 4]),
                Kind::RpcSet64 => Value::Identifiers(Some(Set64 { entries: None })),
                Kind::RpcPursuitMaps => Value::PursuitMaps([Some(Vec::new()), Some(Vec::new())]),
                Kind::RpcSpawn => Value::InactiveSpawn(Vec::new()),
                Kind::LevelReference => return Err(Error::Unsupported),
            })
        })
        .collect()
}

pub fn creation(
    level_id: u16,
    key: u32,
    constructor_word: Option<u32>,
    serial: Serial,
    values: &[Value],
    content: &Content,
) -> Result<Creation, Error> {
    let profile = content.profile(key)?;
    if profile.is_root()
        || [0, u16::MAX].contains(&level_id)
        || values.len() != profile.kinds().len()
    {
        return Err(Error::Shape);
    }
    let mut endpoint = 0;
    let mut initial = Vec::with_capacity(values.len());
    let mut updates = Vec::with_capacity(values.len());
    for (kind, value) in profile.kinds().iter().zip(values) {
        let rpc = Rpc {
            selector: endpoint,
            serial,
        };
        if !matches!(
            kind,
            Kind::Noop
                | Kind::BoolProperty
                | Kind::FloatProperty
                | Kind::I32Property
                | Kind::LevelReference
        ) {
            endpoint += 1;
        }
        let (field, update) = match (kind, value) {
            (Kind::Noop, Value::Noop) => (Initial::Empty, Update::Noop),
            (Kind::Rpc, Value::Rpc) => (Initial::Rpc(rpc), Update::Noop),
            (Kind::BoolProperty, Value::BoolProperty(v)) => {
                (Initial::BoolProperty(*v), Update::BoolProperty(None))
            }
            (Kind::FloatProperty, Value::FloatProperty(v)) => {
                (Initial::FloatProperty(*v), Update::FloatProperty(None))
            }
            (Kind::I32Property, Value::I32Property(v)) => {
                (Initial::I32Property(*v), Update::I32Property(None))
            }
            (Kind::RpcReferences, Value::References(v)) => (
                Initial::RpcReferences {
                    rpc,
                    value: v.clone(),
                },
                Update::Noop,
            ),
            (Kind::RpcPairs, Value::Pairs(v)) => (
                Initial::RpcPairs {
                    rpc,
                    value: v.clone(),
                },
                Update::Noop,
            ),
            (Kind::RpcTagged, Value::Tagged(v)) => (
                Initial::RpcTagged {
                    rpc,
                    value: v.clone(),
                },
                Update::Noop,
            ),
            (Kind::RpcOptional64, Value::Optional64(v)) => {
                (Initial::RpcOptional64 { rpc, value: *v }, Update::Noop)
            }
            (Kind::RpcGuid, Value::Guid(v)) => (Initial::RpcGuid { rpc, value: *v }, Update::Noop),
            (Kind::RpcBool, Value::Boolean(v)) => {
                (Initial::RpcBool { rpc, value: *v }, Update::Noop)
            }
            (Kind::RpcMap, Value::Map(v)) => (
                Initial::RpcMap {
                    rpc,
                    value: v.clone(),
                },
                Update::ReferenceMap(Some(v.clone())),
            ),
            (Kind::RpcFourReferences, Value::FourReferences(v)) => {
                (Initial::Rpc(rpc), Update::FourReferences(*v))
            }
            (Kind::RpcSet64, Value::Identifiers(v)) => {
                (Initial::Rpc(rpc), Update::Set64(v.clone()))
            }
            (Kind::RpcPursuitMaps, Value::PursuitMaps(v)) => {
                (Initial::Rpc(rpc), Update::PursuitMaps(v.clone()))
            }
            (Kind::RpcSpawn, Value::InactiveSpawn(v)) => (
                Initial::RpcSpawnInactive {
                    rpc,
                    references: v.clone(),
                },
                Update::Noop,
            ),
            _ => return Err(Error::TypeMismatch),
        };
        initial.push(field);
        updates.push(Some(update));
    }
    let creation = Creation {
        prefix: Prefix {
            level_id,
            content_key: key,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word,
        },
        body: Body {
            initial: Some(initial),
            updates,
        },
    };
    creation.encode(content)?;
    Ok(creation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::state::World;
    #[test]
    fn all_ordinary_shapes_are_owned_and_isolated() {
        let kinds = [
            Kind::Noop,
            Kind::Rpc,
            Kind::BoolProperty,
            Kind::FloatProperty,
            Kind::I32Property,
            Kind::RpcReferences,
            Kind::RpcPairs,
            Kind::RpcTagged,
            Kind::RpcOptional64,
            Kind::RpcGuid,
            Kind::RpcBool,
            Kind::RpcMap,
            Kind::RpcFourReferences,
            Kind::RpcSet64,
            Kind::RpcPursuitMaps,
            Kind::RpcSpawn,
        ];
        let profile = Profile::new(false, &kinds).unwrap();
        let values = unpopulated(&profile).unwrap();
        let content = Content::new(vec![(77, profile)]).unwrap();
        let baseline =
            creation(91, 77, Some(7), Serial::new(0).unwrap(), &values, &content).unwrap();
        assert_eq!(baseline.prefix.constructor_word, Some(7));
        let mut a = World::default();
        let mut b = World::default();
        a.register_levels(&[91]).unwrap();
        b.register_levels(&[91]).unwrap();
        let first = a.spawn_scene(baseline.clone(), &content).unwrap();
        assert_eq!(b.spawn_scene(baseline, &content).unwrap(), first);
        let mut delta = vec![None; kinds.len()];
        delta[2] = Some(Update::BoolProperty(Some(true)));
        let change = crate::replication::Update::SubLevel {
            profile: content.profile(77).unwrap().clone(),
            fields: delta,
        };
        assert!(a.change(1, change.clone()).unwrap().is_some());
        assert!(a.change(1, change).unwrap().is_none());
        assert_ne!(a.snapshot(), b.snapshot());
        let mut decoder = crate::replication::section::Bindings::with_sublevels(content.clone());
        let section = crate::replication::section::Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: Some(crate::replication::section::Setup::Packed {
                tag: None,
                width: None,
                axes: [None; 3],
            }),
            records: a.snapshot(),
        };
        let wire = section.encode().unwrap();
        assert_eq!(decoder.decode(wire.span()).unwrap().section, section);
        let mut invalid = values.clone();
        invalid[5] = Value::References(vec![2]);
        let bad = creation(92, 77, None, Serial::new(0).unwrap(), &invalid, &content).unwrap();
        a.register_levels(&[92]).unwrap();
        assert_eq!(a.spawn_scene(bad, &content), Err(Error::UnknownObject));
        assert_eq!(a.len(), 1);
        invalid[5] = Value::References(vec![0; 1025]);
        assert_eq!(
            creation(92, 77, None, Serial::new(0).unwrap(), &invalid, &content),
            Err(Error::Bound)
        );
        invalid[5] = Value::Boolean(false);
        assert_eq!(
            creation(92, 77, None, Serial::new(0).unwrap(), &invalid, &content),
            Err(Error::TypeMismatch)
        );
    }
}
