// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::replication::sublevel::{Blueprint, Content, Kind, Rpc, Set64};
use nfs_protocol::world::rpc::Serial;

fn bindings() -> Bindings<'static> {
    Bindings {
        ghost: &|id| (1..=10).contains(&id),
        level: &|id| id == 0 || id == 7,
    }
}
fn fixture() -> (Creation, Content) {
    let rpc = Rpc {
        selector: 3,
        serial: Serial::new(9).unwrap(),
    };
    let profile = Profile::new(
        true,
        &[
            Kind::Noop,
            Kind::BoolProperty,
            Kind::FloatProperty,
            Kind::I32Property,
            Kind::RpcMap,
            Kind::RpcFourReferences,
            Kind::RpcSet64,
            Kind::RpcPursuitMaps,
            Kind::LevelReference,
            Kind::RpcReferences,
            Kind::RpcPairs,
            Kind::RpcTagged,
            Kind::RpcSpawn,
        ],
    )
    .unwrap();
    let initial = vec![
        Initial::Empty,
        Initial::BoolProperty(Some(false)),
        Initial::FloatProperty(Some(0x7fc00001)),
        Initial::I32Property(Some(-7)),
        Initial::RpcMap {
            rpc: rpc.clone(),
            value: vec![(1, 17)],
        },
        Initial::Rpc(rpc.clone()),
        Initial::Rpc(rpc.clone()),
        Initial::Rpc(rpc.clone()),
        Initial::Empty,
        Initial::RpcReferences {
            rpc: rpc.clone(),
            value: vec![0, 2],
        },
        Initial::RpcPairs {
            rpc: rpc.clone(),
            value: vec![(3, 4)],
        },
        Initial::RpcTagged {
            rpc: rpc.clone(),
            value: vec![(5, -1)],
        },
        Initial::RpcSpawnInactive {
            rpc,
            references: vec![6],
        },
    ];
    (
        Creation {
            prefix: Prefix {
                level_id: 0,
                content_key: 7,
                blueprint: Some(Blueprint {
                    ghost: 1,
                    sub_id: 31,
                }),
                word_a: 12,
                word_b: 13,
                constructor_word: None,
            },
            body: Body {
                initial: Some(initial),
                updates: vec![None; 13],
            },
        },
        Content::new(vec![(7, profile)]).unwrap(),
    )
}
fn updates() -> Vec<Option<Update>> {
    vec![None; 13]
}

#[test]
fn creation_changes_become_current_values_and_reconnect_state() {
    let (mut creation, content) = fixture();
    creation.body.updates[1] = Some(Update::BoolProperty(Some(true)));
    creation.body.updates[2] = Some(Update::FloatProperty(Some(0x80000000)));
    creation.body.updates[3] = Some(Update::I32Property(Some(i32::MIN)));
    creation.body.updates[4] = Some(Update::ReferenceMap(Some(vec![(7, 23)])));
    creation.body.updates[5] = Some(Update::FourReferences([Some(8), None, Some(0), None]));
    creation.body.updates[8] = Some(Update::LevelReference(Some(7)));
    let state = State::new(creation.clone(), &content, bindings()).unwrap();
    let current = state.snapshot(bindings()).unwrap();
    let initial = current.body.initial.as_ref().unwrap();
    assert_eq!(initial[1], Initial::BoolProperty(Some(true)));
    assert_eq!(initial[2], Initial::FloatProperty(Some(0x80000000)));
    assert_eq!(initial[3], Initial::I32Property(Some(i32::MIN)));
    assert!(matches!(&initial[4], Initial::RpcMap { value, .. } if value == &vec![(7, 23)]));
    assert_eq!(current.prefix, creation.prefix);
    assert_eq!(&current.body.updates[1..5], &[None, None, None, None]);
    let wire = current.encode(&content).unwrap();
    let decoded = Creation::decode(wire.span(), &content).unwrap().creation;
    assert_eq!(State::new(decoded, &content, bindings()).unwrap(), state);
}

#[test]
fn absent_values_and_repeated_changes_are_idempotent() {
    let (creation, content) = fixture();
    let mut state = State::new(creation, &content, bindings()).unwrap();
    let mut delta = updates();
    delta[5] = Some(Update::FourReferences([Some(1), Some(2), None, None]));
    delta[7] = Some(Update::PursuitMaps([
        Some(vec![(3, 4)]),
        Some(vec![(5, 6)]),
    ]));
    assert!(state.apply(delta.clone(), bindings()).unwrap());
    assert!(!state.apply(delta, bindings()).unwrap());
    let original = state.clone();
    let mut empty = updates();
    empty[0] = Some(Update::Noop);
    empty[1] = Some(Update::BoolProperty(None));
    empty[2] = Some(Update::FloatProperty(None));
    empty[3] = Some(Update::I32Property(None));
    empty[4] = Some(Update::ReferenceMap(None));
    empty[5] = Some(Update::FourReferences([None; 4]));
    empty[6] = Some(Update::Set64(None));
    empty[7] = Some(Update::PursuitMaps([None, None]));
    empty[8] = Some(Update::LevelReference(None));
    assert!(!state.apply(empty, bindings()).unwrap());
    assert_eq!(state, original);
}

#[test]
fn independent_fields_merge_and_explicit_empty_values_clear() {
    let (creation, content) = fixture();
    let mut state = State::new(creation, &content, bindings()).unwrap();
    let mut first = updates();
    first[5] = Some(Update::FourReferences([Some(1), Some(2), None, None]));
    first[6] = Some(Update::Set64(Some(Set64 {
        entries: Some(vec![123]),
    })));
    first[7] = Some(Update::PursuitMaps([
        Some(vec![(3, 4)]),
        Some(vec![(5, 6)]),
    ]));
    first[8] = Some(Update::LevelReference(Some(7)));
    state.apply(first, bindings()).unwrap();
    let mut second = updates();
    second[4] = Some(Update::ReferenceMap(Some(vec![])));
    second[5] = Some(Update::FourReferences([Some(0), None, Some(3), None]));
    second[6] = Some(Update::Set64(Some(Set64 { entries: None })));
    second[7] = Some(Update::PursuitMaps([Some(vec![]), None]));
    second[8] = Some(Update::LevelReference(Some(u16::MAX)));
    state.apply(second, bindings()).unwrap();
    let current = state.snapshot(bindings()).unwrap();
    assert!(
        matches!(&current.body.initial.unwrap()[4], Initial::RpcMap { value, .. } if value.is_empty())
    );
    assert_eq!(
        current.body.updates[5],
        Some(Update::FourReferences([Some(0), Some(2), Some(3), None]))
    );
    assert_eq!(
        current.body.updates[6],
        Some(Update::Set64(Some(Set64 { entries: None })))
    );
    assert_eq!(
        current.body.updates[7],
        Some(Update::PursuitMaps([Some(vec![]), Some(vec![(5, 6)])]))
    );
    assert_eq!(
        current.body.updates[8],
        Some(Update::LevelReference(Some(u16::MAX)))
    );
}

#[test]
fn invalid_late_fields_and_references_rollback_every_change() {
    let (creation, content) = fixture();
    let state = State::new(creation, &content, bindings()).unwrap();
    for (index, bad, expected) in [
        (8, Update::LevelReference(Some(9)), Error::UnknownObject),
        (
            5,
            Update::FourReferences([None, None, None, Some(11)]),
            Error::UnknownObject,
        ),
        (
            7,
            Update::PursuitMaps([None, Some(vec![(1, 11)])]),
            Error::UnknownObject,
        ),
        (
            4,
            Update::ReferenceMap(Some(vec![(11, 0)])),
            Error::UnknownObject,
        ),
        (8, Update::BoolProperty(Some(true)), Error::TypeMismatch),
        (
            5,
            Update::FourReferences([Some(8192), None, None, None]),
            Error::Shape,
        ),
    ] {
        let mut next = state.clone();
        let mut delta = updates();
        delta[1] = Some(Update::BoolProperty(Some(true)));
        delta[index] = Some(bad);
        assert_eq!(next.apply(delta, bindings()), Err(expected));
        assert_eq!(next, state);
    }
}

#[test]
fn all_initial_reference_shapes_and_scene_registration_are_checked() {
    let (creation, content) = fixture();
    for id in 1..=6 {
        let known = |value| value != id;
        assert_eq!(
            State::new(
                creation.clone(),
                &content,
                Bindings {
                    ghost: &known,
                    level: &|_| true
                }
            ),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(
        State::new(
            creation.clone(),
            &content,
            Bindings {
                ghost: &|_| true,
                level: &|_| false
            }
        ),
        Err(Error::UnknownObject)
    );
    let mut no_initial = creation;
    no_initial.body.initial = None;
    assert_eq!(
        State::new(no_initial, &content, bindings()),
        Err(Error::Shape)
    );
}

#[test]
fn removed_binding_blocks_snapshot_until_reference_is_cleared() {
    let (creation, content) = fixture();
    let mut state = State::new(creation, &content, bindings()).unwrap();
    let mut delta = updates();
    delta[8] = Some(Update::LevelReference(Some(7)));
    state.apply(delta, bindings()).unwrap();
    let removed = Bindings {
        ghost: bindings().ghost,
        level: &|id| id == 0,
    };
    assert_eq!(state.snapshot(removed), Err(Error::UnknownObject));
    let mut clear = updates();
    clear[8] = Some(Update::LevelReference(Some(u16::MAX)));
    state.apply(clear, removed).unwrap();
    state.snapshot(removed).unwrap();
}

#[test]
fn worlds_and_reconnect_observers_do_not_share_mutable_fields() {
    let (creation, content) = fixture();
    let mut a = State::new(creation.clone(), &content, bindings()).unwrap();
    let b = State::new(creation, &content, bindings()).unwrap();
    let old = a.snapshot(bindings()).unwrap();
    let mut delta = updates();
    delta[3] = Some(Update::I32Property(Some(77)));
    a.apply(delta, bindings()).unwrap();
    assert_ne!(a, b);
    assert_eq!(b.snapshot(bindings()).unwrap(), old);
    assert_eq!(
        State::new(a.snapshot(bindings()).unwrap(), &content, bindings()).unwrap(),
        a
    );
}

#[test]
fn accumulated_snapshot_size_is_bounded_and_failure_is_atomic() {
    let rpc = Rpc {
        selector: 0,
        serial: Serial::new(0).unwrap(),
    };
    let mut kinds = vec![Kind::Noop];
    kinds.extend([Kind::RpcSet64; 4]);
    kinds.extend([Kind::BoolProperty; 12]);
    let content = Content::new(vec![(7, Profile::new(true, &kinds).unwrap())]).unwrap();
    let (mut creation, _) = fixture();
    let mut initial = vec![Initial::Empty];
    initial.extend(vec![Initial::Rpc(rpc); 4]);
    initial.extend(vec![Initial::BoolProperty(Some(false)); 12]);
    creation.body = Body {
        initial: Some(initial),
        updates: vec![None; kinds.len()],
    };
    let mut state = State::new(creation, &content, bindings()).unwrap();
    for index in 1..4 {
        let mut delta = vec![None; kinds.len()];
        delta[index] = Some(Update::Set64(Some(Set64 {
            entries: Some(vec![0; 127]),
        })));
        state.apply(delta, bindings()).unwrap();
    }
    let before = state.clone();
    let mut delta = vec![None; kinds.len()];
    delta[4] = Some(Update::Set64(Some(Set64 {
        entries: Some(vec![0; 127]),
    })));
    assert_eq!(state.apply(delta, bindings()), Err(Error::Bound));
    assert_eq!(state, before);
}

#[test]
fn singleton_noop_snapshot_preserves_implicit_presence() {
    let (mut creation, _) = fixture();
    let content = Content::new(vec![(7, Profile::new(true, &[Kind::Noop]).unwrap())]).unwrap();
    creation.body = Body {
        initial: Some(vec![Initial::Empty]),
        updates: vec![Some(Update::Noop)],
    };
    let mut state = State::new(creation, &content, bindings()).unwrap();
    assert!(!state.apply(vec![Some(Update::Noop)], bindings()).unwrap());
    state
        .snapshot(bindings())
        .unwrap()
        .encode(&content)
        .unwrap();
}
