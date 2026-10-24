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
    let logic = crate::garage_logic::GarageLogic::from_json(&logic_document()).unwrap();
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

fn logic_document() -> serde_json::Value {
    let entity = |id, index| {
        json!({"capture_id":id,
        "asset":{"bundle":10,"type_id":80,"local_index":index},
        "blueprint":{"scene_content_key":701},"roles":[{"kind":"root","bus_count":1}]})
    };
    let empty = json!({"scene_updates":[],"entities":[]});
    json!({
        "format":"nfs-garage-logic","version":1,"build_sha256":crate::content::BUILD,
        "content":{"catalogs":[{"bundle":10,"entries":[[80,2]]}],
            "profiles":([0,1].map(|i|json!({"asset":{"bundle":10,"type_id":80,"local_index":i},"serializers":["root"]})))},
        "spawn":{"scene_updates":[],"entities":[entity(100,0)]},
        "entry":empty,
        "crashed":{"scene_updates":[],"entities":[entity(101,1)],"delete_capture_ids":[100]},
        "resetting":{"scene_updates":[],"entities":[entity(102,1)]},
        "reset_arrived":{"scene_updates":[],"entities":[],"delete_capture_ids":[102]},
        "recovered":{"scene_updates":[],"entities":[entity(100,0)],"delete_capture_ids":[101],
            "events":[{"target":{"capture_id":100},"entity":1,"event":17}]}
    })
}

#[test]
fn reset_presentation_requires_paired_phases_and_cleanup() {
    use crate::garage_logic::GarageLogic;
    for missing in ["resetting", "reset_arrived"] {
        let mut doc = logic_document();
        doc.as_object_mut().unwrap().remove(missing);
        assert!(GarageLogic::from_json(&doc).is_err());
    }
    let mut doc = logic_document();
    doc["reset_arrived"]["delete_capture_ids"] = json!([]);
    assert!(GarageLogic::from_json(&doc).is_err());
    // Older deployment documents remain compatible.
    doc.as_object_mut().unwrap().remove("resetting");
    doc.as_object_mut().unwrap().remove("reset_arrived");
    assert!(GarageLogic::from_json(&doc).is_ok());
}

#[test]
fn crash_keeps_ai_focus_and_existing_traffic_while_driving_presentation_is_hidden() {
    use crate::traffic_content::population::{Input, tests as traffic};
    let (mut listener, owner) = setup();
    let vehicle = car(&mut listener, owner);
    let mut physics = listener
        .players
        .vehicle_physics(owner.connection, owner.persona, vehicle)
        .unwrap();
    physics.position = nfs_world::replication::vehicle::Vector::from_position(
        traffic::focus().position,
        [0.; 3],
        7,
    )
    .unwrap();
    listener
        .players
        .set_vehicle_physics(owner.connection, owner.persona, vehicle, physics)
        .unwrap();
    listener
        .world_cars
        .insert(vehicle, (owner.participant, true));
    assert!(
        listener.ai_focus().unwrap().is_empty(),
        "initial presentation is still required"
    );
    listener.free_roam_presented.insert(owner.participant);
    let initial_focus = listener.ai_focus().unwrap();
    assert_eq!(initial_focus.len(), 1);
    let describe = |focus: &[crate::traffic_content::population::Focus]| {
        focus
            .iter()
            .map(|f| (f.id, f.position, f.velocity, f.forward))
            .collect::<Vec<_>>()
    };
    let (mut population, mut world, content) = traffic::fixture_with(owner.connection);
    let ids = traffic::spawn(&mut population, &mut world, &content, owner.connection)
        .spawned
        .unwrap();
    for cycle in 0..3 {
        listener.participants.crashed(owner.participant).unwrap();
        listener.advance_world_activation().unwrap();
        assert!(listener.participants.in_free_roam().is_empty());
        assert!(!listener.free_roam_presented.contains(&owner.participant));
        let focus = listener.ai_focus().unwrap();
        assert_eq!(describe(&focus), describe(&initial_focus));
        let changes = population
            .advance(
                &mut world,
                owner.connection,
                100,
                &content,
                Input {
                    now: 1002 + cycle * 5000,
                    focus: &focus,
                    random: [0; 2],
                },
            )
            .unwrap();
        assert!(changes.leave.is_empty() && changes.deleted.is_empty());
        assert!(population.count() >= 1);
        assert!(world.objects().get(ids.vehicle).is_some());
        listener.participants.recovered(owner.participant).unwrap();
        listener.advance_world_activation().unwrap();
        assert_eq!(
            describe(&listener.ai_focus().unwrap()),
            describe(&initial_focus)
        );
    }
    listener.world_cars.get_mut(&vehicle).unwrap().1 = false;
    listener.participants.crashed(owner.participant).unwrap();
    assert!(
        listener.ai_focus().unwrap().is_empty(),
        "unready cars remain excluded"
    );
    listener.world_cars.clear();
    assert!(
        listener.ai_focus().unwrap().is_empty(),
        "removed cars cannot retain AI"
    );
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

fn transport_with_snapshot(players: &Players) -> nfs_world::session::Host {
    use nfs_world::{
        application::{Fixed, Policy},
        session::{Host, Ids},
        transport::Codec,
    };
    let key = nfs_world::crypto::session_key(
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-aaaaaaaaaaaa",
    )
    .unwrap();
    let mut host = Host::new(
        Codec::new(key, [5; 64]).unwrap(),
        Ids {
            client: 1,
            host: 2,
            host_index: 0,
        },
        Policy::default(),
        Box::new(Fixed::new([1, 2, 3])),
    )
    .unwrap();
    for section in split_scenes(
        players.objects().snapshot(),
        nfs_world::application::OUTBOUND_FRAME_BITS,
    )
    .unwrap()
    {
        host.queue_replication(section).unwrap();
    }
    host
}

#[test]
fn crash_start_and_completion_are_atomic_under_queue_pressure_and_repeat() {
    let (mut listener, owner) = setup();
    let initial_count = listener.players.objects().len();
    for cycle in 0..3 {
        let mut transport = transport_with_snapshot(&listener.players);
        assert_eq!(
            transport.queue_rpc(HostRpc::SequenceStop(nfs_world::sequences::Stop {
                sequence: listener.logic_ghosts[&owner.participant][&100],
                serial: Serial::new(1).unwrap(),
            })),
            Err(nfs_world::session::Error::Replication(
                replication::Error::UnknownObject
            ))
        );
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
        transport
            .queue_replication_after(section.clone(), calls.clone())
            .unwrap();
        assert!(matches!(calls.as_slice(), [HostRpc::Participant(n)] if
            n.participant == owner.participant && n.call == Call::Enter));
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
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&102));
        if cycle > 0 {
            listener
                .advance_crashes(u64::from(cycle) * 5000 + 4501)
                .unwrap();
            assert!(listener.pending_rpcs.iter().any(|rpc| matches!(
                rpc, HostRpc::SequenceStop(stop) if stop.sequence == ghost
            )));
            if cycle == 2 {
                // A slow cinematic must be allowed to receive its timer Stop
                // through the production host's sequence type validation.
                let stop = listener.pending_rpcs.pop_front().unwrap();
                transport
                    .queue_replication_after(returning::empty_section(), vec![stop])
                    .unwrap();
            }
        }
        let wire = completion_frame(owner, ghost, cycle as u8);
        listener
            .pending_entries
            .resize(MAX_PENDING - 1, (returning::empty_section(), vec![]));
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
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&102));
        assert!(!listener.pending_rpcs.iter().any(|rpc| matches!(
            rpc, HostRpc::SequenceStop(stop) if stop.sequence == ghost
        )));
        // Exercise the socket host's reference validation as well as encoding.
        // Recovered statistics events name entities created by this transition.
        let entries: Vec<_> = listener.pending_entries.drain(..).collect();
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].0.records.is_empty());
        assert!(entries[0].1.is_empty());
        assert!(entries[1].0.records.is_empty());
        assert!(!entries[1].1.is_empty());
        for (section, calls) in entries {
            transport.queue_replication_after(section, calls).unwrap();
        }
        assert_eq!(
            transport.queue_rpc(HostRpc::SequenceStop(nfs_world::sequences::Stop {
                sequence: ghost,
                serial: Serial::new(1).unwrap(),
            })),
            Err(nfs_world::session::Error::Replication(
                replication::Error::UnknownObject
            ))
        );
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
        (&fresh.event_state, None),
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
        (&fresh.event_state, None),
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
            &movement,
            (&fresh.event_state, None),
        )
        .is_err()
    );
}

#[test]
fn rollover_relocation_waits_for_publication_and_acknowledgement_then_repeats() {
    use nfs_world::replication::vehicle::{
        chassis::{Controls, Motion},
        movement::Sample,
    };
    let (mut listener, owner) = setup();
    let car_id = car(&mut listener, owner);
    listener
        .world_cars
        .insert(car_id, (owner.participant, true));
    let mut revision = 0;
    let mut previous_loading = None;
    for cycle in 0..2 {
        let offset = f64::from(cycle) * 20.0;
        for i in 0..41 {
            revision += 1;
            let crashed = i == 40;
            let motion = Motion {
                identity: (offset + f64::from(i) / 4.0).to_bits(),
                revision,
                position: [i as f32, 2.0, -80.0],
                quaternion: if crashed {
                    [1., 0., 0., 0.]
                } else {
                    [0., 0., 0., 1.]
                },
                velocity: [0.; 3],
                angular_velocity: [0.; 3],
            };
            let mut controls = Controls::new(4).unwrap();
            controls.flags[0] = crashed;
            let sample = Sample {
                physics: motion.encode([0.; 3], &controls).unwrap(),
                flag: true,
            };
            listener
                .crashes
                .as_mut()
                .unwrap()
                .observe_vehicle(&listener.players, owner, car_id, &sample, true, None)
                .unwrap();
        }
        listener.world_ms = (offset * 1000.0) as u64 + 10_000;
        let mut transport = transport_with_snapshot(&listener.players);
        listener.advance_crashes(listener.world_ms).unwrap();
        let (creation, calls) = listener.pending_entries.pop_front().unwrap();
        let sequence = creation.records.last().unwrap().id;
        transport.queue_replication_after(creation, calls).unwrap();
        let done = completion_frame(owner, sequence, cycle * 4);
        // Failure to enqueue the complete transition must not expose the
        // loading entity or consume the sequence completion.
        listener
            .pending_entries
            .resize(MAX_PENDING - 1, (returning::empty_section(), vec![]));
        let before = listener.players.objects().snapshot();
        assert!(!listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4),
            data: done.span()
        }));
        assert_eq!(listener.players.objects().snapshot(), before);
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&102));
        listener.pending_entries.clear();
        assert!(listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4),
            data: done.span()
        }));
        assert!(
            listener
                .crashes
                .as_ref()
                .unwrap()
                .relocating(owner.participant)
        );
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::Crashing)
        );
        assert!(listener.logic_ghosts[&owner.participant].contains_key(&101));
        let loading = listener.logic_ghosts[&owner.participant][&102];
        assert_ne!(previous_loading, Some(loading));
        previous_loading = Some(loading);
        assert!(
            listener.pending_entries[0]
                .0
                .records
                .iter()
                .any(|r| r.id == loading)
        );
        assert!(listener.pending_entries[0].0.deleted.contains(&sequence));
        assert!(listener.due_teleport(listener.world_ms).is_none());
        let bindings = ResetBindings::from_scene(
            &listener.players,
            listener.participants.exit_bindings().unwrap().state_2.scene,
        )
        .unwrap();
        // An old/early acknowledgement cannot finish a command not yet sent.
        let ack_body = request(bindings.teleport, owner.participant, 3);
        let ack = |sequence| {
            frame::Builder::new()
                .messages(
                    None,
                    vec![frame::Group {
                        channel: 0,
                        sequence: Some(sequence),
                        messages: vec![frame::Message::Opaque {
                            index: 47,
                            body: ack_body.span(),
                        }],
                    }],
                )
                .build()
                .unwrap()
        };
        let early = ack(cycle * 4 + 1);
        assert!(listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4 + 1),
            data: early.span()
        }));
        assert!(
            listener
                .crashes
                .as_ref()
                .unwrap()
                .relocating(owner.participant)
        );
        assert!(listener.players.objects().get(loading).is_some());
        for (section, calls) in listener.pending_entries.drain(..) {
            transport.queue_replication_after(section, calls).unwrap();
        }
        listener.session_backlog = 1;
        assert!(listener.due_teleport(listener.world_ms).is_none());
        listener.session_backlog = 0;
        let command = listener.due_teleport(listener.world_ms).unwrap();
        assert_eq!(command.vehicle, car_id);
        assert_eq!(command.endpoint, bindings.teleport);
        assert_eq!(command.position, [29., 2., -80.]);
        assert_eq!(command.basis, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
        transport
            .queue_content(
                nfs_world::teleport::CONTENT_TARGET,
                &command.encode().unwrap(),
            )
            .unwrap();
        listener.teleport_sent();
        assert!(listener.pending_teleports.is_empty());
        // Another participant cannot acknowledge this owner's relocation.
        let mut foreign_crashes = listener.crashes.clone().unwrap();
        let mut foreign_players = listener.players.clone();
        let mut foreign_participants = listener.participants.clone();
        let mut foreign_ghosts = listener.logic_ghosts.clone();
        let bad_body = request(bindings.teleport, owner.participant + 1, 3);
        let objects = foreign_players.objects().len();
        assert!(
            PlayerListener::finish_crash_call(
                bad_body.span(),
                &mut foreign_crashes,
                &mut foreign_players,
                &mut foreign_participants,
                listener.logic.as_deref(),
                &mut foreign_ghosts,
                owner.persona,
                None,
                &listener.event_state,
            )
            .is_err()
        );
        assert_eq!(foreign_players.objects().len(), objects);
        assert!(foreign_crashes.relocating(owner.participant));
        let completed = ack(cycle * 4 + 2);
        listener
            .pending_entries
            .resize(MAX_PENDING - 1, (returning::empty_section(), vec![]));
        let before = listener.players.objects().snapshot();
        assert!(!listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4 + 2),
            data: completed.span()
        }));
        assert_eq!(listener.players.objects().snapshot(), before);
        assert!(listener.players.objects().get(loading).is_some());
        listener.pending_entries.clear();
        assert!(listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4 + 2),
            data: completed.span()
        }));
        assert_eq!(
            listener.participants.stage(owner.participant),
            Some(Stage::FreeRoam)
        );
        assert!(
            !listener
                .crashes
                .as_ref()
                .unwrap()
                .relocating(owner.participant)
        );
        assert!(listener.logic_ghosts[&owner.participant].contains_key(&100));
        assert!(!listener.logic_ghosts[&owner.participant].contains_key(&102));
        assert!(listener.players.objects().get(loading).is_none());
        let entries: Vec<_> = listener.pending_entries.drain(..).collect();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].0.deleted.contains(&loading));
        assert!(entries[0].1.is_empty());
        let states: Vec<_> = entries[1]
            .1
            .iter()
            .filter_map(|rpc| {
                if let HostRpc::Participant(n) = rpc {
                    Some(*n)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(states, bindings.finish(owner.participant));
        for (section, calls) in entries {
            transport.queue_replication_after(section, calls).unwrap();
        }
        let count = listener.players.objects().len();
        let duplicate = ack(cycle * 4 + 3);
        assert!(listener.frame(Queued {
            advertised: None,
            frame: u16::from(cycle * 4 + 3),
            data: duplicate.span()
        }));
        assert_eq!(listener.players.objects().len(), count);
        assert!(listener.pending_entries.is_empty());
        listener.pending.clear();
    }
    assert_eq!(listener.unsupported_rpcs, 0);
}

#[test]
fn crash_without_history_uses_installed_road_spawn_and_rejects_invalid_clock() {
    let (mut listener, owner) = setup();
    let id = car(&mut listener, owner);
    let mut sample = Sample {
        physics: listener
            .players
            .vehicle_physics(owner.connection, owner.persona, id)
            .unwrap(),
        flag: true,
    };
    sample.physics.controls.flags[0] = true;
    sample.physics.identity_words = [0x7ff8_0000, 0, 1];
    let crashes = listener.crashes.as_mut().unwrap();
    assert!(
        crashes
            .observe_vehicle(&listener.players, owner, id, &sample, true, None)
            .is_err()
    );
    assert!(crashes.pending().is_empty());
    sample.physics.identity_words = [0, 0, 1];
    crashes
        .observe_vehicle(&listener.players, owner, id, &sample, true, None)
        .unwrap();
    listener.advance_crashes(1).unwrap();
    let (creation, _) = listener.pending_entries.pop_front().unwrap();
    let sequence = creation.records.last().unwrap().id;
    let body = Completed {
        sequence,
        participant: owner.participant,
    }
    .encode()
    .unwrap();
    let mut fixture = crate::vehicle_content::layout::tests::fixture();
    fixture["exit_teleport"] = json!({"transform_bits":fixture["main"][1]["transform_bits"]});
    let layout = crate::vehicle_content::layout::Layout::from_json(&fixture).unwrap();
    let result = PlayerListener::finish_crash_call(
        body.span(),
        listener.crashes.as_mut().unwrap(),
        &mut listener.players,
        &mut listener.participants,
        listener.logic.as_deref(),
        &mut listener.logic_ghosts,
        owner.persona,
        Some(&layout),
        &listener.event_state,
    )
    .unwrap()
    .unwrap()
    .unwrap();
    let command = result.2.unwrap();
    assert_eq!(command.position, [7., 0., 0.]);
    assert_eq!(command.vehicle, id);
    assert_eq!(
        listener.participants.stage(owner.participant),
        Some(Stage::Crashing)
    );
}
