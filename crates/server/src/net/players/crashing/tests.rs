// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    bits::BitWriter,
    participants::{Bindings, Endpoint},
    replication::{
        Initial,
        entity::creation::{Asset, Catalog, Content as Entities},
        sublevel::{self, Content, Kind, Profile, ordinary},
    },
    sequences::Component,
};
use serde_json::json;

fn request(endpoint: Endpoint, participant: u16, method: u64) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(method, 32)
        .align();
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(endpoint.scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
fn setup() -> (PlayerListener, Owner) {
    let mut startup = vec![Kind::RpcReferences; 117];
    startup[0] = Kind::Noop;
    let mut garage = vec![Kind::Rpc; 28];
    garage[0] = Kind::Noop;
    let scenes = Content::new(vec![
        (701, sublevel::gameplay::profile().unwrap()),
        (702, Profile::new(false, &startup).unwrap()),
        (703, Profile::new(false, &garage).unwrap()),
    ])
    .unwrap();
    let mut listener = PlayerListener::new(400);
    let creations = scenes
        .profiles()
        .enumerate()
        .map(|(i, (key, p))| {
            ordinary::creation(
                (i + 1) as u16,
                key,
                None,
                Serial::new(0).unwrap(),
                &ordinary::unpopulated(p).unwrap(),
                &scenes,
            )
            .unwrap()
        })
        .collect();
    let records = listener
        .players
        .create_scenes(&[1, 2, 3], creations, &scenes)
        .unwrap();
    listener.participants.bind(&records, 701, 702).unwrap();
    listener.participants.bind_exit(&records, 702, 703).unwrap();
    let bindings = Bindings::from_records(&records, 701, 702).unwrap().unwrap();
    let player = listener
        .players
        .create(
            HOST_SELECTOR as u8,
            400,
            Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = listener
        .players
        .join(HOST_SELECTOR as u8, 400, player)
        .unwrap()
        .unwrap()
        .id;
    let owner = Owner {
        connection: HOST_SELECTOR as u8,
        persona: 400,
        participant,
    };
    listener.participants.begin(participant).unwrap();
    listener.participants.set_garage([None; 5]).unwrap();
    listener
        .participants
        .receive(request(bindings.inventory, participant, 3).span(), |_| true)
        .unwrap();
    listener.participants.take_garage_bindings();
    listener.participants.finish_persistent(|_| true);
    assert!(listener.participants.begin_progression(participant));
    listener.participants.enter_garage_loading(participant);
    listener.participants.enter_customization(participant);
    let exit = listener.participants.exit_bindings().unwrap();
    for (e, m) in [
        (exit.exit_request, 0),
        (exit.state_77, 3),
        (exit.ready_request, 0),
    ] {
        listener
            .participants
            .receive(request(e, participant, m).span(), |_| true)
            .unwrap();
    }
    listener.participants.take_exited();
    listener.participants.take_arriving();
    listener.participants.arrived(participant);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    let asset = Asset {
        bundle: 9,
        type_id: 77,
        local_index: 0,
    };
    listener.crashes = Some(
        Crashes::new(
            nfs_world::crash::Definition {
                asset,
                manager: Component {
                    scene_key: 701,
                    index: 3,
                },
                embedded_bus: 2,
            },
            Entities::new(
                vec![Catalog::new(9, &[(77, 1)]).unwrap()],
                vec![(asset, nfs_world::crash::profile())],
            )
            .unwrap(),
            4500,
        )
        .unwrap(),
    );
    let entity = |id, index| {
        json!({"capture_id":id,
        "asset":{"bundle":10,"type_id":80,"local_index":index},
        "blueprint":{"scene_content_key":701},"roles":[{"kind":"root","bus_count":1}]})
    };
    let empty = json!({"scene_updates":[],"entities":[]});
    let logic = crate::garage_logic::GarageLogic::from_json(&json!({
        "format":"nfs-garage-logic","version":1,"build_sha256":crate::content::BUILD,
        "content":{"catalogs":[{"bundle":10,"entries":[[80,2]]}],
            "profiles":([0,1].map(|i|json!({"asset":{"bundle":10,"type_id":80,"local_index":i},"serializers":["root"]})))},
        "spawn":{"scene_updates":[],"entities":[entity(100,0)]},
        "entry":empty,
        "crashed":{"scene_updates":[],"entities":[entity(101,1)],"delete_capture_ids":[100]},
        "recovered":{"scene_updates":[],"entities":[entity(100,0)],"delete_capture_ids":[101]}
    })).unwrap();
    let mut ghosts = BTreeMap::new();
    logic
        .produce(
            crate::garage_logic::Which::Spawn,
            &mut listener.players,
            participant,
            &mut ghosts,
        )
        .unwrap();
    listener.logic_ghosts.insert(participant, ghosts);
    listener.logic = Some(std::sync::Arc::new(logic));
    listener.players.sync_player_views().unwrap();
    listener.players.sync_player_manager().unwrap();
    (listener, owner)
}
fn completion_frame(owner: Owner, ghost: u16, sequence: u8) -> BitWriter {
    let call = Completed {
        sequence: ghost,
        participant: owner.participant,
    }
    .encode()
    .unwrap();
    frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages: vec![frame::Message::Opaque {
                    index: 47,
                    body: call.span(),
                }],
            }],
        )
        .build()
        .unwrap()
}

#[test]
fn crash_start_and_completion_are_atomic_under_queue_pressure_and_repeat() {
    let (mut listener, owner) = setup();
    let initial_count = listener.players.objects().len();
    for cycle in 0..3 {
        let before = listener.players.objects().snapshot();
        let crashes = listener.crashes.as_mut().unwrap();
        crashes
            .observe(&listener.players, owner, cycle * 3 + 1, true, true)
            .unwrap();
        crashes
            .observe(&listener.players, owner, cycle * 3 + 2, false, true)
            .unwrap();
        listener.session_backlog = MAX_PENDING;
        listener.advance_crashes(u64::from(cycle) * 5000).unwrap();
        assert_eq!(listener.players.objects().snapshot(), before);
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::FreeRoam)
        );
        listener.session_backlog = 0;
        listener
            .advance_crashes(u64::from(cycle) * 5000 + 1)
            .unwrap();
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::Crashing)
        );
        let (section, calls) = listener.pending_entries.pop_front().unwrap();
        assert!(calls.is_empty());
        assert_eq!(section.deleted.len(), 1);
        assert_eq!(section.records.len(), 2);
        let ghost = section
            .records
            .iter()
            .find_map(|r| {
                let Some(Initial::Entity { fields, .. }) = &r.initial else {
                    return None;
                };
                matches!(
                    fields[0],
                    nfs_world::replication::entity::Initial::SequenceRoot { .. }
                )
                .then_some(r.id)
            })
            .unwrap();
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&100));
        if cycle == 1 {
            listener
                .advance_crashes(u64::from(cycle) * 5000 + 4501)
                .unwrap();
            assert!(listener.pending_rpcs.iter().any(|rpc| matches!(
                rpc, HostRpc::SequenceStop(stop) if stop.sequence == ghost
            )));
        }
        let wire = completion_frame(owner, ghost, cycle as u8);
        listener
            .pending_entries
            .resize(MAX_PENDING, (returning::empty_section(), vec![]));
        let crashed = listener.players.objects().snapshot();
        assert!(!listener.frame(Queued {
            advertised: None,
            frame: cycle as u16,
            data: wire.span()
        }));
        assert_eq!(listener.players.objects().snapshot(), crashed);
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::Crashing)
        );
        listener.pending_entries.clear();
        assert!(listener.frame(Queued {
            advertised: None,
            frame: cycle as u16,
            data: wire.span()
        }));
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::FreeRoam)
        );
        assert_eq!(listener.players.objects().len(), initial_count);
        assert!(listener.logic_ghosts[&owner.participant].contains_key(&100));
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&101));
        assert!(!listener.pending_rpcs.iter().any(|rpc| matches!(
            rpc, HostRpc::SequenceStop(stop) if stop.sequence == ghost
        )));
        let complete = listener.players.objects().snapshot();
        listener.pending_entries.clear();
        listener.pending.clear();
        assert!(listener.frame(Queued {
            advertised: None,
            frame: cycle as u16,
            data: wire.span()
        }));
        assert_eq!(listener.players.objects().snapshot(), complete);
        assert!(listener.pending_entries.is_empty());
    }
    assert_eq!(listener.unsupported_rpcs, 0);
}

fn car(listener: &mut PlayerListener, owner: Owner) -> u16 {
    use nfs_world::replication::{
        entity::{self, creation::Prefix},
        vehicle,
    };
    listener
        .players
        .create_actor(owner.connection, owner.persona, owner.participant)
        .unwrap();
    let asset = Asset {
        bundle: 12,
        type_id: 30,
        local_index: 0,
    };
    let content = Entities::new(vec![Catalog::new(12, &[(30, 1)]).unwrap()], vec![])
        .unwrap()
        .with_vehicles(vec![(
            asset,
            vehicle::Profile::new(vec![
                vehicle::Kind::Root {
                    property_owner: false,
                },
                vehicle::Kind::Chassis,
            ])
            .unwrap(),
        )])
        .unwrap();
    let root = vehicle::root::State::new(None)
        .unwrap()
        .initial(
            [0.; 3],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            [0.; 3],
            entity::Rpc {
                selector: 0,
                serial: Serial::new(1).unwrap(),
            },
        )
        .unwrap();
    let custom = vehicle::Customization {
        value: 0,
        list_a: vec![],
        list_b: vec![],
        index: None,
        values: [0; 2],
        flag: false,
        tuning: [0; 32],
    };
    let chassis =
        vehicle::chassis::State::new([40., 2., -80.], [0., 0., 0., 1.], 3, custom.clone()).unwrap();
    let creation = vehicle::EntityCreation {
        prefix: Prefix {
            parent: None,
            blueprint: 0,
            sub_id: 1,
            owner: None,
            asset,
        },
        body: vehicle::Body {
            creation: Some(vehicle::Creation {
                setup: None,
                connection_id: owner.connection.into(),
                fields: vec![
                    vehicle::Initial::Root(Box::new(root)),
                    vehicle::Initial::Chassis(Box::new(vehicle::ChassisInitial {
                        rpc: entity::Rpc {
                            selector: 1,
                            serial: Serial::new(2).unwrap(),
                        },
                        vectors: [[None; 3]; 2],
                        custom,
                    })),
                ],
            }),
            updates: vec![
                Some(vehicle::Update::Root(None)),
                Some(chassis.update([0.; 3], 11).unwrap()),
            ],
        },
    };
    listener
        .players
        .create_vehicle(
            owner.connection,
            owner.persona,
            owner.participant,
            1,
            creation,
            &content,
        )
        .unwrap()
        .id
}

#[test]
fn movement_pulse_between_echoes_survives_and_only_owned_ready_cars_trigger() {
    let (mut listener, owner) = setup();
    let id = car(&mut listener, owner);
    listener.world_cars.insert(id, (owner.participant, true));
    let initial = listener
        .players
        .vehicle_physics(owner.connection, owner.persona, id)
        .unwrap();
    let wire = |revision, signal| {
        let mut sample = Sample {
            physics: initial.clone(),
            flag: true,
        };
        sample.physics.identity_words = [0, 0, revision];
        sample.physics.controls.flags[0] = signal;
        let (header, body) = sample.encode().unwrap();
        let mut frame = BitWriter::new();
        frame
            .put(1 << frame::MOVEMENT, 6)
            .put_span(header.span())
            .put(1, 6)
            .put(id.into(), 16)
            .put(0, 4)
            .put(body.len() as u64, 16)
            .put_span(body.span())
            .align();
        frame
    };
    for (revision, now, signal) in [(1, 100, false), (2, 120, true), (3, 140, false)] {
        listener.world_ms = now;
        let packet = wire(revision, signal);
        assert!(listener.frame(Queued {
            advertised: None,
            frame: revision as u16,
            data: packet.span()
        }));
        listener.pending.clear();
    }
    assert_eq!(listener.crashes.as_ref().unwrap().pending(), vec![owner]);
    // Neither the short crash pulse nor its clearing report were echoed.
    assert!(
        !listener
            .players
            .vehicle_physics(owner.connection, owner.persona, id)
            .unwrap()
            .controls
            .flags[0]
    );
    listener.advance_crashes(150).unwrap();
    assert_eq!(
        listener.participants.stage(owner.participant),
        Some(Stage::Crashing)
    );

    let (mut fresh, owner) = setup();
    let id = car(&mut fresh, owner);
    let physics = fresh
        .players
        .vehicle_physics(owner.connection, owner.persona, id)
        .unwrap();
    let mut sample = Sample {
        physics,
        flag: true,
    };
    sample.physics.controls.flags[0] = true;
    let (header, body) = sample.encode().unwrap();
    let movement = frame::Movement {
        header: Some(header.span()),
        records: vec![frame::MovementRecord {
            id,
            component: 0,
            data: body.span(),
        }],
    };
    // Unselected/AI traffic never feeds this player's crash state.
    PlayerListener::observe_crashes(
        fresh.crashes.as_mut().unwrap(),
        &fresh.players,
        &fresh.participants,
        &BTreeMap::new(),
        owner.persona,
        &movement,
    )
    .unwrap();
    assert!(fresh.crashes.as_ref().unwrap().pending().is_empty());
    PlayerListener::observe_crashes(
        fresh.crashes.as_mut().unwrap(),
        &fresh.players,
        &fresh.participants,
        &BTreeMap::from([(id, (owner.participant, false))]),
        owner.persona,
        &movement,
    )
    .unwrap();
    assert!(fresh.crashes.as_ref().unwrap().pending().is_empty());
    assert!(
        PlayerListener::observe_crashes(
            fresh.crashes.as_mut().unwrap(),
            &fresh.players,
            &fresh.participants,
            &BTreeMap::from([(id, (owner.participant, true))]),
            owner.persona + 1,
            &movement
        )
        .is_err()
    );
}
