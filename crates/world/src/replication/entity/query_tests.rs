// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use creation::{Asset, Catalog, Content, Creation, Prefix};

fn profile() -> Profile {
    Profile::new(&[
        Kind::Root,
        Kind::RpcBool,
        Kind::RpcReferences,
        Kind::FloatProperty,
    ])
    .unwrap()
}

fn wire(flag: bool, references: &[u16]) -> BitWriter {
    // Constructed independently of Body::encode: 48-bit root, 20-bit query,
    // 19-bit RPC + 16-bit reference count, then optional float and update mask.
    let mut wire = BitWriter::new();
    wire.put(2, 16).put(0, 9).put(7, 10).put(5, 13);
    wire.put(1, 9).put(7, 10).put(u64::from(flag), 1);
    wire.put(2, 9).put(7, 10).put(references.len() as u64, 16);
    for id in references {
        wire.put(u64::from(*id), 13);
    }
    wire.put(1, 1).put(0x8000_0000, 32);
    wire.put(0, 1).put(1, 1).put(1, 1).put(1, 1).put(0, 1);
    wire
}

#[test]
fn query_initialization_keeps_following_fields_and_exact_record_boundary() {
    let profile = profile();
    for flag in [false, true] {
        for references in [vec![], vec![0, 5, 8191]] {
            let wire = wire(flag, &references);
            let decoded = Body::decode(wire.span(), &profile, true).unwrap();
            assert_eq!(decoded.initial_bits, Some(136 + 13 * references.len()));
            assert_eq!(decoded.bits, 141 + 13 * references.len());
            assert_eq!(decoded.bits, wire.len());
            let initial = decoded.body.initial.as_ref().unwrap();
            assert_eq!(
                initial[1],
                Initial::RpcBool {
                    rpc: Rpc {
                        selector: 1,
                        serial: Serial::new(7).unwrap()
                    },
                    value: flag,
                }
            );
            assert_eq!(
                initial[2],
                Initial::RpcReferences {
                    rpc: Rpc {
                        selector: 2,
                        serial: Serial::new(7).unwrap()
                    },
                    references,
                }
            );
            assert_eq!(initial[3], Initial::FloatProperty(Some(0x8000_0000)));
            assert_eq!(decoded.body.encode(&profile).unwrap(), wire);
            for end in 0..wire.len() {
                assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, true).is_err());
            }
            let mut combined = wire.clone();
            combined.put(0xabc, 12);
            assert_eq!(
                Body::decode(combined.span(), &profile, true).unwrap(),
                decoded
            );
        }
    }
}

#[test]
fn reference_count_and_identity_bounds_fail_before_accepting_a_record() {
    let profile = profile();
    let maximal = wire(true, &vec![8191; MAX_REFERENCES]);
    let decoded = Body::decode(maximal.span(), &profile, true).unwrap();
    assert_eq!(decoded.body.encode(&profile).unwrap(), maximal);
    for count in [MAX_REFERENCES + 1, u16::MAX as usize] {
        let mut invalid = BitWriter::new();
        invalid
            .put(0, 48)
            .put(0, 20)
            .put(0, 19)
            .put(count as u64, 16);
        assert_eq!(
            Body::decode(invalid.span(), &profile, true),
            Err(Error::Bound)
        );
    }
    let mut body = decoded.body;
    let Initial::RpcReferences { references, .. } = &mut body.initial.as_mut().unwrap()[2] else {
        panic!()
    };
    references.push(1);
    assert_eq!(body.encode(&profile), Err(Error::Bound));
    let Initial::RpcReferences { references, .. } = &mut body.initial.as_mut().unwrap()[2] else {
        panic!()
    };
    *references = vec![8192];
    assert!(body.encode(&profile).is_err());
}

#[test]
fn query_state_validates_every_reference_and_rejects_typed_updates_atomically() {
    let profile = profile();
    let asset = Asset {
        bundle: 2,
        type_id: 7,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(2, &[(7, 1)]).unwrap()],
        vec![(asset, profile.clone())],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 3,
            sub_id: 1,
            owner: Some(5),
            asset,
        },
        body: Body::decode(wire(true, &[0, 5, 7]).span(), &profile, true)
            .unwrap()
            .body,
    };
    assert_eq!(
        state::State::new(creation.clone(), &content, |id| [3, 5].contains(&id)),
        Err(Error::UnknownObject)
    );
    let mut state = state::State::new(creation, &content, |id| [3, 5, 7].contains(&id)).unwrap();
    let before = state.clone();
    for index in [1, 2] {
        let mut delta = vec![None; 4];
        delta[3] = Some(Update::FloatProperty(Some(1)));
        delta[index] = Some(Update::BoolProperty(Some(false)));
        assert_eq!(state.apply(delta), Err(Error::TypeMismatch));
        assert_eq!(state, before);
    }
    let delta = vec![None, Some(Update::Noop), Some(Update::Noop), None];
    state.apply(delta.clone()).unwrap();
    state.apply(delta).unwrap();
    assert_eq!(state, before);
    let snapshot = state.snapshot();
    let encoded = snapshot.encode(&content).unwrap();
    assert_eq!(
        Creation::decode(encoded.span(), &content).unwrap().creation,
        snapshot
    );
}

fn sorter_wire(value: Option<(u16, i32)>) -> BitWriter {
    let mut wire = BitWriter::new();
    // Root, query RPC, optional (13-bit participant, signed 32-bit score),
    // following Boolean property, and three update flags.
    wire.put(2, 16).put(0, 9).put(7, 10).put(5, 13);
    wire.put(1, 9).put(7, 10).put(u64::from(value.is_some()), 1);
    if let Some((reference, score)) = value {
        wire.put(u64::from(reference), 13)
            .put(u64::from(score as u32), 32);
    }
    wire.put(1, 1).put(1, 1).put(1, 1).put(1, 1).put(0, 1);
    wire
}

#[test]
fn sorter_optional_result_preserves_signed_score_and_record_boundary() {
    let profile = Profile::new(&[
        Kind::Root,
        Kind::RpcOptionalReferenceI32,
        Kind::BoolProperty,
    ])
    .unwrap();
    for value in [
        None,
        Some((0, 0)),
        Some((7, -1)),
        Some((8191, i32::MIN)),
        Some((1, i32::MAX)),
    ] {
        let wire = sorter_wire(value);
        let decoded = Body::decode(wire.span(), &profile, true).unwrap();
        assert_eq!(decoded.bits, if value.is_some() { 118 } else { 73 });
        assert_eq!(decoded.initial_bits, Some(decoded.bits - 3));
        assert_eq!(
            decoded.body.initial.as_ref().unwrap()[1],
            Initial::RpcOptionalReferenceI32 {
                rpc: Rpc {
                    selector: 1,
                    serial: Serial::new(7).unwrap()
                },
                value,
            }
        );
        assert_eq!(
            decoded.body.initial.as_ref().unwrap()[2],
            Initial::BoolProperty(Some(true))
        );
        assert_eq!(decoded.body.encode(&profile).unwrap(), wire);
        for end in 0..wire.len() {
            assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, true).is_err());
        }
        let mut concatenated = wire.clone();
        concatenated.put(0xabc, 12);
        assert_eq!(
            Body::decode(concatenated.span(), &profile, true).unwrap(),
            decoded
        );
    }
}

#[test]
fn sorter_result_checks_references_and_has_no_property_update_body() {
    let profile = Profile::new(&[
        Kind::Root,
        Kind::RpcOptionalReferenceI32,
        Kind::BoolProperty,
    ])
    .unwrap();
    let asset = Asset {
        bundle: 2,
        type_id: 7,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(2, &[(7, 1)]).unwrap()],
        vec![(asset, profile.clone())],
    )
    .unwrap();
    let mut creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 3,
            sub_id: 1,
            owner: Some(5),
            asset,
        },
        body: Body::decode(sorter_wire(Some((7, -100))).span(), &profile, true)
            .unwrap()
            .body,
    };
    assert_eq!(
        state::State::new(creation.clone(), &content, |id| [3, 5].contains(&id)),
        Err(Error::UnknownObject)
    );
    let mut state =
        state::State::new(creation.clone(), &content, |id| [3, 5, 7].contains(&id)).unwrap();
    let before = state.clone();
    assert_eq!(
        state.apply(vec![None, Some(Update::I32Property(Some(8))), None]),
        Err(Error::TypeMismatch)
    );
    assert_eq!(state, before);
    state.apply(vec![None, Some(Update::Noop), None]).unwrap();
    assert_eq!(state, before);
    let snapshot = state.snapshot();
    assert_eq!(
        Creation::decode(snapshot.encode(&content).unwrap().span(), &content)
            .unwrap()
            .creation,
        snapshot
    );
    creation.body.initial.as_mut().unwrap()[1] = Initial::RpcOptionalReferenceI32 {
        rpc: Rpc {
            selector: 1,
            serial: Serial::new(7).unwrap(),
        },
        value: Some((8192, 0)),
    };
    assert!(creation.encode(&content).is_err());
    creation.body.initial.as_mut().unwrap()[1] = Initial::RpcOptionalReferenceI32 {
        rpc: Rpc {
            selector: 1,
            serial: Serial::new(7).unwrap(),
        },
        value: None,
    };
    assert!(state::State::new(creation, &content, |id| [3, 5].contains(&id)).is_ok());
}
