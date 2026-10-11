// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Content, Creation, Error, Initial, Kind, Prefix, Profile, Rpc, Set64, Update};
use nfs_protocol::world::rpc::Serial;

pub const REFERENCES: [usize; 6] = [1, 16, 26, 33, 36, 50];
pub const BOOLS: [usize; 6] = [19, 21, 141, 142, 169, 171];
pub const INTEGERS: [usize; 6] = [20, 80, 81, 82, 83, 84];
pub const TAGGED: [usize; 6] = [60, 65, 66, 67, 68, 69];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Inputs {
    pub references: [Vec<u16>; 6],
    pub active_id: Option<u64>,
    pub active_guid: Option<[u8; 16]>,
    pub booleans: [bool; 6],
    pub integers: [Option<i32>; 6],
    pub pairs: Vec<(u16, u16)>,
    pub tagged: [Vec<(u16, i8)>; 6],
    pub values: Vec<(u16, u32)>,
    pub four_references: [u16; 4],
    pub identifiers: Option<Vec<u64>>,
    pub boolean_property: Option<bool>,
    pub floats: [Option<u32>; 2],
}

pub fn profile() -> Result<Profile, Error> {
    let mut kinds = [Kind::Rpc; 189];
    kinds[0] = Kind::Noop;
    for index in REFERENCES {
        kinds[index] = Kind::RpcReferences;
    }
    for index in BOOLS {
        kinds[index] = Kind::RpcBool;
    }
    for index in INTEGERS {
        kinds[index] = Kind::I32Property;
    }
    for index in TAGGED {
        kinds[index] = Kind::RpcTagged;
    }
    for (index, kind) in [
        (6, Kind::RpcOptional64),
        (18, Kind::RpcGuid),
        (22, Kind::RpcSet64),
        (24, Kind::RpcPairs),
        (28, Kind::RpcFourReferences),
        (44, Kind::RpcMap),
        (184, Kind::BoolProperty),
        (187, Kind::FloatProperty),
        (188, Kind::FloatProperty),
    ] {
        kinds[index] = kind;
    }
    Profile::new(false, &kinds)
}

pub fn creation(
    key: u32,
    level_id: u16,
    serial: Serial,
    inputs: &Inputs,
) -> Result<Creation, Error> {
    if [0, u16::MAX].contains(&level_id) {
        return Err(Error::Shape);
    }
    let profile = profile()?;
    let mut endpoint = 0;
    let mut initial = Vec::with_capacity(profile.kinds().len());
    let mut updates = Vec::with_capacity(profile.kinds().len());
    for (index, kind) in profile.kinds().iter().enumerate() {
        let (field, update) = match kind {
            Kind::Noop => (Initial::Empty, Update::Noop),
            Kind::I32Property => {
                let slot = INTEGERS
                    .iter()
                    .position(|i| *i == index)
                    .ok_or(Error::Shape)?;
                (
                    Initial::I32Property(inputs.integers[slot]),
                    Update::I32Property(None),
                )
            }
            Kind::BoolProperty => (
                Initial::BoolProperty(inputs.boolean_property),
                Update::BoolProperty(None),
            ),
            Kind::FloatProperty => (
                Initial::FloatProperty(inputs.floats[index - 187]),
                Update::FloatProperty(None),
            ),
            _ => {
                let rpc = Rpc {
                    selector: endpoint,
                    serial,
                };
                endpoint += 1;
                match kind {
                    Kind::Rpc => (Initial::Rpc(rpc), Update::Noop),
                    Kind::RpcReferences => {
                        let slot = REFERENCES
                            .iter()
                            .position(|i| *i == index)
                            .ok_or(Error::Shape)?;
                        (
                            Initial::RpcReferences {
                                rpc,
                                value: inputs.references[slot].clone(),
                            },
                            Update::Noop,
                        )
                    }
                    Kind::RpcBool => {
                        let slot = BOOLS.iter().position(|i| *i == index).ok_or(Error::Shape)?;
                        (
                            Initial::RpcBool {
                                rpc,
                                value: inputs.booleans[slot],
                            },
                            Update::Noop,
                        )
                    }
                    Kind::RpcTagged => {
                        let slot = TAGGED
                            .iter()
                            .position(|i| *i == index)
                            .ok_or(Error::Shape)?;
                        (
                            Initial::RpcTagged {
                                rpc,
                                value: inputs.tagged[slot].clone(),
                            },
                            Update::Noop,
                        )
                    }
                    Kind::RpcPairs => (
                        Initial::RpcPairs {
                            rpc,
                            value: inputs.pairs.clone(),
                        },
                        Update::Noop,
                    ),
                    Kind::RpcOptional64 => (
                        Initial::RpcOptional64 {
                            rpc,
                            value: inputs.active_id,
                        },
                        Update::Noop,
                    ),
                    Kind::RpcGuid => (
                        Initial::RpcGuid {
                            rpc,
                            value: inputs.active_guid,
                        },
                        Update::Noop,
                    ),
                    Kind::RpcMap => (
                        Initial::RpcMap {
                            rpc,
                            value: inputs.values.clone(),
                        },
                        Update::ReferenceMap(Some(inputs.values.clone())),
                    ),
                    Kind::RpcSet64 => (
                        Initial::Rpc(rpc),
                        Update::Set64(Some(Set64 {
                            entries: inputs.identifiers.clone(),
                        })),
                    ),
                    Kind::RpcFourReferences => (
                        Initial::Rpc(rpc),
                        Update::FourReferences(inputs.four_references.map(Some)),
                    ),
                    _ => return Err(Error::Unsupported),
                }
            }
        };
        initial.push(field);
        updates.push(Some(update));
    }
    let result = Creation {
        prefix: Prefix {
            level_id,
            content_key: key,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word: None,
        },
        body: Body {
            initial: Some(initial),
            updates,
        },
    };
    result.encode(&Content::new(vec![(key, profile)])?)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::sublevel::state::{Bindings, State};
    fn content() -> Content {
        Content::new(vec![(crate::test_data::GAMEPLAY_KEY, profile().unwrap())]).unwrap()
    }
    fn bindings() -> Bindings<'static> {
        Bindings {
            ghost: &|id| id == 7,
            level: &|id| id == 91,
        }
    }

    #[test]
    fn fresh_scene_has_no_bound_objects_or_property_overrides() {
        let creation = creation(
            crate::test_data::GAMEPLAY_KEY,
            91,
            Serial::new(0).unwrap(),
            &Inputs::default(),
        )
        .unwrap();
        let fields = creation.body.initial.as_ref().unwrap();
        assert_eq!(fields.len(), 189);
        assert_eq!(
            fields[1],
            Initial::RpcReferences {
                rpc: Rpc {
                    selector: 0,
                    serial: Serial::new(0).unwrap()
                },
                value: vec![]
            }
        );
        assert_eq!(fields[20], Initial::I32Property(None));
        assert_eq!(fields[187], Initial::FloatProperty(None));
        assert_eq!(fields[188], Initial::FloatProperty(None));
        assert_eq!(
            fields[186],
            Initial::Rpc(Rpc {
                selector: 178,
                serial: Serial::new(0).unwrap()
            })
        );
        State::new(creation.clone(), &content(), bindings()).unwrap();
        let wire = creation.encode(&content()).unwrap();
        assert_eq!(
            Creation::decode(wire.span(), &content()).unwrap().creation,
            creation
        );
    }

    #[test]
    fn supplied_state_survives_snapshot_and_rejects_foreign_references() {
        let mut inputs = Inputs {
            active_id: Some(0x123456789abcdef0),
            active_guid: Some([7; 16]),
            booleans: [true; 6],
            integers: [Some(-2); 6],
            boolean_property: Some(true),
            floats: [Some(1f32.to_bits()), Some(0.25f32.to_bits())],
            four_references: [7; 4],
            values: vec![(7, 123)],
            pairs: vec![(7, 7)],
            identifiers: Some(vec![19]),
            ..Inputs::default()
        };
        inputs.references[3].push(7);
        inputs.tagged[2].push((7, -1));
        let creation = creation(
            crate::test_data::GAMEPLAY_KEY,
            91,
            Serial::new(9).unwrap(),
            &inputs,
        )
        .unwrap();
        let state = State::new(creation.clone(), &content(), bindings()).unwrap();
        let snapshot = state.snapshot(bindings()).unwrap();
        assert_eq!(snapshot.body.initial, creation.body.initial);
        assert_eq!(
            state.snapshot(Bindings {
                ghost: &|_| false,
                level: &|_| true
            }),
            Err(Error::UnknownObject)
        );
        assert_eq!(
            State::new(
                creation,
                &content(),
                Bindings {
                    ghost: &|_| true,
                    level: &|_| false
                }
            ),
            Err(Error::UnknownObject)
        );
    }

    #[test]
    fn oversized_inputs_and_root_identity_are_rejected() {
        let mut inputs = Inputs::default();
        inputs.references[0] = vec![7; 1025];
        assert_eq!(
            creation(
                crate::test_data::GAMEPLAY_KEY,
                91,
                Serial::new(0).unwrap(),
                &inputs
            ),
            Err(Error::Bound)
        );
        for id in [0, u16::MAX] {
            assert_eq!(
                creation(
                    crate::test_data::GAMEPLAY_KEY,
                    id,
                    Serial::new(0).unwrap(),
                    &Inputs::default()
                ),
                Err(Error::Shape)
            );
        }
    }
}
