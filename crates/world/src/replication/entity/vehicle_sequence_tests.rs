// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::replication::{self, state::World};
use creation::{Asset, Catalog, Content, Creation, Prefix};

#[test]
fn native_vehicle_sequence_lists_preserve_following_serializer_boundary() {
    let profile = Profile::new(&[Kind::VehicleSequenceRoot, Kind::BoolProperty]).unwrap();
    // Native initial order, then independent presence/count/slot bits for the
    // two lists. The following property catches an omitted or oversized list.
    let mut wire = BitWriter::new();
    wire.put(1, 16)
        .put(0, 9)
        .put(2, 10)
        .put(17, 13)
        .put(4, 9)
        .put(3, 32)
        .put(1, 8)
        .put(18, 13)
        .put(0, 1)
        .put(1, 1)
        .put(0, 1)
        .put(1, 1)
        .put(1, 1)
        .put(3, 8)
        .put(0, 1)
        .put(1, 1)
        .put(0, 13)
        .put(1, 1)
        .put(8191, 13)
        .put(1, 1)
        .put(0, 8)
        .put(1, 1)
        .put(1, 1)
        .put(1, 1);
    let decoded = Body::decode(wire.span(), &profile, true).unwrap();
    assert_eq!(decoded.initial_bits, Some(113));
    assert_eq!(decoded.bits, wire.len());
    assert!(matches!(
        decoded.body.initial.as_ref().unwrap()[0],
        Initial::VehicleSequenceRoot { manager: 17, .. }
    ));
    assert_eq!(
        decoded.body.updates,
        vec![
            Some(Update::VehicleSequence(VehicleSequenceUpdate {
                lists: [Some(vec![None, Some(0), Some(8191)]), Some(vec![])]
            })),
            Some(Update::BoolProperty(Some(true))),
        ]
    );
    assert_eq!(decoded.body.encode(&profile).unwrap(), wire);
    for end in 0..wire.len() {
        assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, true).is_err());
    }
    let end = wire.len();
    wire.put(0x5a, 8);
    assert_eq!(Body::decode(wire.span(), &profile, true).unwrap().bits, end);
    let wrong = Profile::new(&[Kind::SequenceRoot, Kind::BoolProperty]).unwrap();
    assert_eq!(decoded.body.encode(&wrong), Err(Error::TypeMismatch));
}

#[test]
fn absent_empty_and_maximum_lists_are_distinct_and_bounded() {
    let profile = Profile::new(&[Kind::VehicleSequenceRoot]).unwrap();
    for (lists, expected_bits) in [
        ([None, None], 2),
        ([Some(vec![]), None], 10),
        ([None, Some(vec![None])], 11),
        ([Some(vec![Some(8191); 255]), Some(vec![])], 3588),
    ] {
        let body = Body {
            initial: None,
            updates: vec![Some(Update::VehicleSequence(VehicleSequenceUpdate {
                lists,
            }))],
        };
        let wire = body.encode(&profile).unwrap();
        assert_eq!(wire.len(), expected_bits);
        let decoded = Body::decode(wire.span(), &profile, false).unwrap();
        assert_eq!(decoded.body, body);
        assert_eq!(decoded.bits, wire.len());
        for end in 0..wire.len() {
            assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, false).is_err());
        }
    }
    for entries in [vec![None; 256], vec![Some(8192)]] {
        let body = Body {
            initial: None,
            updates: vec![Some(Update::VehicleSequence(VehicleSequenceUpdate {
                lists: [Some(entries), None],
            }))],
        };
        assert!(body.encode(&profile).is_err());
    }
    assert_eq!(
        Body {
            initial: None,
            updates: vec![Some(Update::Noop)]
        }
        .encode(&profile),
        Err(Error::TypeMismatch)
    );
    assert_eq!(
        Profile::new(&[Kind::Root, Kind::VehicleSequenceRoot]),
        Err(Error::Unsupported)
    );
}

fn fixture() -> (Creation, Content) {
    let asset = Asset {
        bundle: 3,
        type_id: 7,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(3, &[(7, 1)]).unwrap()],
        vec![(asset, Profile::new(&[Kind::VehicleSequenceRoot]).unwrap())],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 0,
            sub_id: 1,
            owner: None,
            asset,
        },
        body: Body {
            initial: Some(vec![Initial::VehicleSequenceRoot {
                value: 1,
                rpc: Rpc {
                    selector: 0,
                    serial: Serial::new(1).unwrap(),
                },
                manager: 0,
                manager_selector: 0,
                sequence: 1,
                participants: vec![],
                stopping: false,
            }]),
            updates: vec![Some(Update::VehicleSequence(Default::default()))],
        },
    };
    (creation, content)
}

#[test]
fn owned_sequence_references_are_atomic_retained_and_block_retirement() {
    let (creation, content) = fixture();
    let mut world = World::default();
    let goal = world
        .spawn(
            replication::Initial::ChaseTargetGoal(replication::target_goal::Chase {
                allocation_serial: 0,
                target: 0,
            }),
            replication::Update::ChaseTargetGoal,
        )
        .unwrap()
        .id;
    let mut invalid = creation.clone();
    invalid.body.updates = vec![Some(Update::VehicleSequence(VehicleSequenceUpdate {
        lists: [None, Some(vec![Some(8191)])],
    }))];
    let before = world.clone();
    assert_eq!(
        world.spawn_entity(invalid, &content),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        world.iter().collect::<Vec<_>>(),
        before.iter().collect::<Vec<_>>()
    );
    let sequence = world.spawn_entity(creation, &content).unwrap();
    let replication::Update::Entity { binding, .. } = sequence.update else {
        panic!()
    };
    let change = |lists| replication::Update::Entity {
        binding: binding.clone(),
        fields: vec![Some(Update::VehicleSequence(VehicleSequenceUpdate {
            lists,
        }))],
    };
    world
        .change(sequence.id, change([Some(vec![Some(goal), None]), None]))
        .unwrap()
        .unwrap();
    assert_eq!(world.change(sequence.id, change([None, None])), Ok(None));
    world
        .change(sequence.id, change([None, Some(vec![Some(0)])]))
        .unwrap()
        .unwrap();
    let before = world.clone();
    assert_eq!(
        world.change(sequence.id, change([Some(vec![]), Some(vec![Some(8191)])])),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        world.iter().collect::<Vec<_>>(),
        before.iter().collect::<Vec<_>>()
    );
    assert_eq!(world.remove(goal), Err(Error::UnknownObject));
    assert_eq!(
        world.iter().collect::<Vec<_>>(),
        before.iter().collect::<Vec<_>>()
    );
    let replication::Update::Entity { fields, .. } = &world.get(sequence.id).unwrap().current
    else {
        panic!()
    };
    assert_eq!(
        fields,
        &vec![Some(Update::VehicleSequence(VehicleSequenceUpdate {
            lists: [Some(vec![Some(goal), None]), Some(vec![Some(0)])]
        }))]
    );
    world
        .change(sequence.id, change([Some(vec![]), None]))
        .unwrap()
        .unwrap();
    world.remove(goal).unwrap();
    assert_eq!(
        world.change(sequence.id, change([Some(vec![]), None])),
        Ok(None)
    );
    world.remove(sequence.id).unwrap();
    assert!(world.is_empty());
}
