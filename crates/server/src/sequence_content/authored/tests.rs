// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    logic::EntityRef,
    replication::{
        entity::{
            Body, Initial, Kind, Profile, Rpc, Update,
            creation::{Creation, Prefix},
        },
        players::Request,
        sublevel,
    },
    sequences::Component,
};

fn fixture() -> (Players, Authored, Owner, Owner) {
    let mut players = Players::default();
    let scene_content = sublevel::Content::new(vec![(
        7,
        sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::Noop,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    players
        .create_scenes(
            &[1],
            vec![
                sublevel::passive::creation(1, 7, Serial::new(9).unwrap(), &scene_content).unwrap(),
            ],
            &scene_content,
        )
        .unwrap();
    let mut join = |connection, persona| {
        let player = players
            .create(
                connection,
                persona,
                Request {
                    name: b"Driver".to_vec(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap()
            .id;
        Owner {
            connection,
            persona,
            participant: players
                .join(connection, persona, player)
                .unwrap()
                .unwrap()
                .id,
        }
    };
    let a = join(3, 10);
    let b = join(5, 20);
    let parent = Asset {
        bundle: 4,
        type_id: 8,
        local_index: 0,
    };
    let parent_content = Content::new(
        vec![Catalog::new(4, &[(8, 1), (9, 1)]).unwrap()],
        vec![(
            parent,
            Profile::new(&[Kind::Root, Kind::BoolProperty, Kind::Rpc]).unwrap(),
        )],
    )
    .unwrap();
    for owner in [a, b] {
        players
            .spawn_entities(
                vec![Creation {
                    prefix: Prefix {
                        parent: None,
                        blueprint: 1,
                        sub_id: 1,
                        owner: None,
                        asset: parent,
                    },
                    body: Body {
                        initial: Some(vec![
                            Initial::Root {
                                value: 1,
                                rpc: Rpc {
                                    selector: 0,
                                    serial: Serial::new(1).unwrap(),
                                },
                                reference: owner.participant,
                            },
                            Initial::BoolProperty(Some(false)),
                            Initial::Rpc(Rpc {
                                selector: 1,
                                serial: Serial::new(1).unwrap(),
                            }),
                        ]),
                        updates: vec![Some(Update::Noop), None, Some(Update::Noop)],
                    },
                }],
                &parent_content,
            )
            .unwrap();
    }
    let data = Sequence {
        blueprint: "opening".into(),
        asset: Asset {
            bundle: 4,
            type_id: 9,
            local_index: 0,
        },
        catalog: vec![(8, 1), (9, 1)],
        manager: Component {
            scene_key: 7,
            index: 2,
        },
        embedded_bus: 2,
        bus_count: 1,
        fields: vec![
            Kind::SequenceRoot,
            Kind::RpcReference,
            Kind::RpcOptionalReference,
            Kind::RpcReference,
        ],
        channels: BTreeMap::from([
            (1, Channel::Participant { serializer: 2 }),
            (
                3,
                Channel::Scene {
                    content_key: 7,
                    serializer: 2,
                },
            ),
        ]),
    };
    (
        players,
        Authored::new(
            data,
            parent,
            vec![Binding {
                entity: 1,
                event: 37,
                node: 2,
            }],
        )
        .unwrap(),
        a,
        b,
    )
}
fn fields(record: &Record) -> &[Initial] {
    let Some(replication::Initial::Entity { fields, .. }) = &record.initial else {
        panic!()
    };
    fields
}
#[test]
fn resolves_live_selectors_and_owner_instead_of_serializer_positions_or_fixed_ids() {
    let (mut players, content, a, b) = fixture();
    let mut instances = Instances::default();
    let first = instances.start(&content, &mut players, a).unwrap().unwrap();
    let second = instances.start(&content, &mut players, b).unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        matches!(&fields(&first)[0],Initial::SequenceRoot{manager:1,manager_selector:0,sequence:0,participants,..} if participants==&[a.participant])
    );
    assert!(
        matches!(&fields(&second)[0],Initial::SequenceRoot{sequence:1,participants,..} if participants==&[b.participant])
    );
    assert!(matches!(
        &fields(&first)[1],
        Initial::RpcReference {
            target_selector: 1,
            ..
        }
    ));
    assert!(matches!(
        &fields(&first)[3],
        Initial::RpcReference {
            reference: 1,
            target_selector: 0,
            ..
        }
    ));
    assert_ne!(fields(&first)[1], fields(&second)[1]);
    let before = players.objects().snapshot();
    assert!(
        instances
            .start(&content, &mut players, a)
            .unwrap()
            .is_none()
    );
    assert_eq!(players.objects().snapshot(), before);
    assert!(
        instances
            .start(
                &content,
                &mut players,
                Owner {
                    persona: b.persona,
                    ..a
                }
            )
            .is_err()
    );
}
#[test]
fn completion_requires_the_owned_signal_then_stop_and_is_repeat_safe() {
    let (mut players, content, a, b) = fixture();
    let mut instances = Instances::default();
    let ghost = instances
        .start(&content, &mut players, a)
        .unwrap()
        .unwrap()
        .id;
    let call = Completed {
        sequence: ghost,
        participant: a.participant,
    };
    assert!(instances.complete(&mut players, a, call).is_err());
    let signal = |event, player| Message::Reached {
        target: EntityRef { ghost, entity: 1 },
        event,
        player,
    };
    for message in [signal(37, 1), signal(37, 255), signal(38, 0)] {
        assert!(
            instances
                .signal(&content, &players, a, &message)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        instances
            .signal(
                &content,
                &players,
                Owner {
                    persona: b.persona,
                    ..a
                },
                &signal(37, 0)
            )
            .is_err()
    );
    // An uncommitted staged transition (for a full output queue) has no effect.
    let mut staged = instances.clone();
    let expected = staged
        .signal(&content, &players, a, &signal(37, 0))
        .unwrap()
        .unwrap();
    assert!(instances.complete(&mut players, a, call).is_err());
    let stop = instances
        .signal(&content, &players, a, &signal(37, 0))
        .unwrap()
        .unwrap();
    assert_eq!(stop, expected);
    assert_eq!(Stop::decode(stop.encode().unwrap().span()).unwrap(), stop);
    assert!(
        instances
            .signal(&content, &players, a, &signal(37, 0))
            .unwrap()
            .is_none()
    );
    assert!(instances.complete(&mut players, b, call).is_err());
    assert_eq!(
        instances.complete(&mut players, a, call).unwrap(),
        Some(ghost)
    );
    assert!(players.objects().get(ghost).is_none());
    assert_eq!(instances.complete(&mut players, a, call).unwrap(), None);
    assert!(
        instances
            .signal(&content, &players, a, &signal(37, 0))
            .unwrap()
            .is_none()
    );
}
#[test]
fn bad_channel_and_ambiguous_owner_do_not_allocate() {
    let (mut players, mut content, a, _) = fixture();
    content
        .data
        .channels
        .insert(1, Channel::Participant { serializer: 1 });
    let before = players.objects().snapshot();
    let mut instances = Instances::default();
    assert!(instances.start(&content, &mut players, a).is_err());
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(instances.ghost(a.participant), None);
    content
        .data
        .channels
        .insert(1, Channel::Participant { serializer: 2 });
    let (prefix,initial)=players.objects().iter().find_map(|(_,o)| match &o.initial {
        replication::Initial::Entity{prefix,fields} if prefix.asset==content.parent && matches!(fields[0],Initial::Root{reference,..} if reference==a.participant)=>Some((prefix.clone(),fields.clone())),_=>None,
    }).unwrap();
    let parent_content = Content::new(
        vec![Catalog::new(4, &[(8, 1), (9, 1)]).unwrap()],
        vec![(
            content.parent,
            Profile::new(&[Kind::Root, Kind::BoolProperty, Kind::Rpc]).unwrap(),
        )],
    )
    .unwrap();
    players
        .spawn_entities(
            vec![Creation {
                prefix,
                body: Body {
                    initial: Some(initial),
                    updates: vec![Some(Update::Noop), None, Some(Update::Noop)],
                },
            }],
            &parent_content,
        )
        .unwrap();
    let before = players.objects().snapshot();
    assert_eq!(
        instances.start(&content, &mut players, a),
        Err(Error::DuplicateObject)
    );
    assert_eq!(players.objects().snapshot(), before);
}
