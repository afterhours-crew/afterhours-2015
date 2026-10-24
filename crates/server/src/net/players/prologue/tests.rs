// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    bits::BitWriter,
    participants::{Bindings, Endpoint},
    replication::{
        entity::{
            self,
            creation::{Asset, Catalog, Content, Creation, Prefix},
        },
        sublevel::{self, Kind, Profile},
    },
    sequences::Component,
};
use std::sync::Arc;

fn request(endpoint: Endpoint, participant: u16, method: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(method.into(), 32)
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
fn packet(sequence: u8, index: u8, body: BitSpan<'_>) -> BitWriter {
    frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages: vec![frame::Message::Opaque { index, body }],
            }],
        )
        .build()
        .unwrap()
}
fn send(listener: &mut PlayerListener, sequence: u8, index: u8, body: BitSpan<'_>) -> bool {
    let wire = packet(sequence, index, body);
    listener.frame(Queued {
        advertised: None,
        frame: sequence.into(),
        data: wire.span(),
    })
}
fn drain(listener: &mut PlayerListener) {
    listener.pending.clear();
    listener.pending_rpcs.clear();
    listener.pending_content.clear();
    listener.pending_entries.clear();
}
pub(super) fn setup() -> (PlayerListener, u16, Arc<nfs_world::items::Collection>) {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .without_time()
        .try_init();
    let (population, mut players, vehicles, layout, items, _) =
        crate::vehicle_content::population::tests::fixture();
    let persona = 0x100000002;
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"Intro".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(HOST_SELECTOR as u8, persona, participant)
        .unwrap();
    let mut startup = vec![Kind::RpcReferences; 117];
    startup[0] = Kind::Noop;
    let scenes =
        sublevel::Content::new(vec![(909, Profile::new(false, &startup).unwrap())]).unwrap();
    players
        .create_scenes(
            &[77],
            vec![
                sublevel::ordinary::creation(
                    77,
                    909,
                    None,
                    Serial::new(0).unwrap(),
                    &sublevel::ordinary::unpopulated(scenes.profile(909).unwrap()).unwrap(),
                    &scenes,
                )
                .unwrap(),
            ],
            &scenes,
        )
        .unwrap();
    let records = players.objects().snapshot();
    let mut participants = Lifecycle::default();
    participants
        .bind(&records, layout.item_scene_key, 909)
        .unwrap();
    participants.bind_intro(&records, 909).unwrap();
    participants.bind_return(&records, 909).unwrap();
    let bindings = Bindings::from_records(&records, layout.item_scene_key, 909)
        .unwrap()
        .unwrap();
    participants.begin(participant).unwrap();
    participants.set_garage([None; 5]).unwrap();
    participants
        .receive(request(bindings.inventory, participant, 3).span(), |_| true)
        .unwrap();
    participants.take_garage_bindings();
    participants.finish_persistent(|_| true);
    assert!(participants.begin_progression(participant));
    let actor = players
        .bind_actor(
            layout.item_scene_key,
            HOST_SELECTOR as u8,
            persona,
            participant,
        )
        .unwrap()
        .unwrap();
    participants.enter_intro_with_actor(actor).unwrap();
    let parent = Asset {
        bundle: 30,
        type_id: 8,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(30, &[(8, 1), (9, 1)]).unwrap()],
        vec![(
            parent,
            entity::Profile::new(&[entity::Kind::Root, entity::Kind::Rpc]).unwrap(),
        )],
    )
    .unwrap();
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
                body: entity::Body {
                    initial: Some(vec![
                        entity::Initial::Root {
                            value: 1,
                            rpc: entity::Rpc {
                                selector: 0,
                                serial: Serial::new(1).unwrap(),
                            },
                            reference: participant,
                        },
                        entity::Initial::Rpc(entity::Rpc {
                            selector: 1,
                            serial: Serial::new(1).unwrap(),
                        }),
                    ]),
                    updates: vec![Some(entity::Update::Noop); 2],
                },
            }],
            &content,
        )
        .unwrap();
    players.enable_player_manager().unwrap();
    players.sync_player_views().unwrap();
    players.sync_player_manager().unwrap();
    let story = crate::prologue::Content::for_test(
        items.clone(),
        parent,
        Component {
            scene_key: layout.item_scene_key,
            index: 3,
        },
    );
    let mut listener = PlayerListener::new(persona)
        .with_vehicles(Some(Arc::new(GarageContent { vehicles, layout })))
        .with_prologue(Some(Arc::new(story)));
    listener.players = players;
    listener.replication_origin_ms = Some(100_000);
    listener.world_ms = 100_000;
    listener.population = Some(population);
    listener.participants = participants;
    let definition = |layout, local_index| nfs_world::sequences::Definition {
        layout,
        asset: Asset {
            bundle: 31,
            type_id: 42,
            local_index,
        },
        manager: Component {
            scene_key: listener.vehicles.as_ref().unwrap().layout.item_scene_key,
            index: 3,
        },
        channel: Component {
            scene_key: 909,
            index: 62,
        },
        presentation: (layout == nfs_world::sequences::Layout::GarageEntry).then_some(Component {
            scene_key: 909,
            index: 64,
        }),
        embedded_bus: 2,
    };
    let codec = |d: nfs_world::sequences::Definition| {
        Content::new(
            vec![Catalog::new(31, &[(42, 2)]).unwrap()],
            vec![(d.asset, d.layout.profile())],
        )
        .unwrap()
    };
    let entry = definition(nfs_world::sequences::Layout::GarageEntry, 0);
    listener.returns = Some(
        nfs_world::arrival::Arrivals::returning(entry, codec(entry), 7500)
            .unwrap()
            .with_spline(replication::spline::Definition {
                asset: Asset {
                    bundle: 31,
                    type_id: 5100,
                    local_index: 0,
                },
                catalog: Catalog::new(31, &[(5100, 1)]).unwrap(),
                flag: false,
            })
            .unwrap(),
    );
    let waiting = definition(nfs_world::sequences::Layout::Waiting, 1);
    listener.sequences =
        Some(nfs_world::sequences::Sequences::new(waiting, codec(waiting)).unwrap());
    (listener, participant, items)
}

#[test]
fn opening_runs_after_car_and_render_readiness_and_waits_for_native_completion() {
    let (mut listener, participant, items) = setup();
    let load_asset = Asset {
        bundle: 32,
        type_id: 10,
        local_index: 0,
    };
    let load = crate::prologue::loading::Loading::new(nfs_content::participant_logic::Layout {
        asset: load_asset,
        catalog: vec![(10, 1)],
        scene_content_key: 909,
        owner: nfs_content::participant_logic::Owner::Scene,
        bus_count: 1,
        fields: vec![
            nfs_content::participant_logic::Field {
                kind: entity::Kind::Root,
                channel: None,
            },
            nfs_content::participant_logic::Field {
                kind: entity::Kind::BoolProperty,
                channel: None,
            },
            nfs_content::participant_logic::Field {
                kind: entity::Kind::RpcReference,
                channel: Some(nfs_content::participant_logic::Channel::Scene {
                    content_key: 909,
                    serializer: 1,
                }),
            },
        ],
    })
    .unwrap();
    let story = Arc::make_mut(listener.prologue.as_mut().unwrap());
    story.loading = Some(load);
    story.selection = Some(story.opening.clone());
    story.selection_program = Some(crate::sequence_content::execution::tests::fixture(false));
    listener.roles = Some(crate::scene_roles::SceneRoles {
        level: b"test".to_vec(),
        gameplay: listener.vehicles.as_ref().unwrap().layout.item_scene_key,
        startup: 909,
        garage: listener.vehicles.as_ref().unwrap().layout.scene_key,
        progression: 909,
        traffic: [0; 10],
        customization_timer: load_asset,
        streaming_gate: load_asset,
        spawn_points: None,
        free_roam: None,
        bank: None,
    });
    let before = (*items).clone();
    listener.advance_prologue().unwrap();
    let vehicle = listener
        .players
        .temporary_vehicle(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    assert!(listener.opening.ghost(participant).is_none());
    assert!(!listener.pending_content.is_empty());
    for section in &listener.pending {
        assert_eq!(section.setup, Some(Setup::RawEscape([0; 3])));
        assert!(section.frame().unwrap().len() <= nfs_world::application::OUTBOUND_FRAME_BITS);
    }
    drain(&mut listener);
    let glass = nfs_world::logic::Message::Reached {
        event: 21_578_436,
        target: nfs_world::logic::EntityRef {
            ghost: vehicle,
            entity: 2,
        },
        player: 0,
    }
    .encode()
    .unwrap();
    let loading = listener.players.objects().iter().find_map(|(id,o)| matches!(&o.initial, replication::Initial::Entity{prefix,..} if prefix.asset == load_asset).then_some(id)).unwrap();
    let streamed = nfs_world::logic::Message::Reached {
        event: nfs_world::sequences::STREAMING_LOADED,
        target: nfs_world::logic::EntityRef {
            ghost: loading,
            entity: 1,
        },
        player: 0,
    }
    .encode()
    .unwrap();
    let first = frame::Builder::new()
        .messages(
            None,
            vec![
                frame::Group {
                    channel: 2,
                    sequence: Some(0),
                    messages: vec![frame::Message::Opaque {
                        index: 82,
                        body: streamed.span(),
                    }],
                },
                frame::Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: vec![frame::Message::Opaque {
                        index: 82,
                        body: glass.span(),
                    }],
                },
            ],
        )
        .build()
        .unwrap();
    assert!(listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: first.span()
    }));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroTeleporting)
    );
    assert!(listener.pending_spawns.is_empty());
    assert_eq!(listener.pending_teleports.len(), 1);
    let command = listener.pending_teleports.front().unwrap().1;
    assert_eq!(command.position, [10., 2., 30.]);
    drain(&mut listener);
    listener.pending_teleports.clear();
    let bindings = listener.participants.intro_bindings().unwrap();
    assert!(send(
        &mut listener,
        1,
        47,
        request(bindings.teleport, participant, 3).span()
    ));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroRendering)
    );
    listener.advance_prologue().unwrap();
    assert!(listener.opening.ghost(participant).is_none());
    drain(&mut listener);
    assert!(send(
        &mut listener,
        2,
        47,
        request(bindings.render_ready, participant, 0).span()
    ));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroOpening)
    );
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    let ghost = listener.opening.ghost(participant).unwrap();
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert!(listener.pending.is_empty());
    let signal = nfs_world::logic::Message::Reached {
        event: 37,
        target: nfs_world::logic::EntityRef { ghost, entity: 1 },
        player: 0,
    }
    .encode()
    .unwrap();
    assert!(send(&mut listener, 3, 82, signal.span()));
    assert!(
        listener
            .pending_rpcs
            .iter()
            .any(|r| matches!(r,HostRpc::SequenceStop(stop) if stop.sequence==ghost))
    );
    drain(&mut listener);
    let completed = Completed {
        sequence: ghost,
        participant,
    }
    .encode()
    .unwrap();
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    assert!(!send(&mut listener, 4, 47, completed.span()));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroOpening)
    );
    assert!(listener.players.objects().get(ghost).is_some());
    drain(&mut listener);
    assert!(send(&mut listener, 4, 47, completed.span()));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroStartingDrive)
    );
    assert!(listener.players.objects().get(ghost).is_none());
    drain(&mut listener);
    assert!(send(&mut listener, 5, 47, completed.span()));
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(*items, before);
    assert_eq!(
        listener
            .players
            .vehicle_item_of(HOST_SELECTOR as u8, listener.persona, vehicle),
        None
    );
    // Mode/sequence/goals publication waits for both replication and RPC space.
    let before_mode = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_mode);
    assert!(listener.intro_modes.is_empty());
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    assert_eq!(listener.intro_modes.len(), 1);
    let mut roots = vec![];
    let mut goals = vec![];
    for (id, object) in listener.players.objects().iter() {
        match &object.initial {
            replication::Initial::Entity { fields, .. }
                if matches!(fields.first(), Some(entity::Initial::SequenceRoot { .. })) =>
            {
                roots.push(id)
            }
            replication::Initial::GotoWaypointGoal(_) | replication::Initial::SpeedLimitGoal(_) => {
                goals.push(id)
            }
            _ => {}
        }
    }
    assert_eq!(roots.len(), 1);
    assert_eq!(goals.len(), 2);
    let drive_sequence = roots[0];
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert!(listener.pending.is_empty());
    let completed = Completed {
        sequence: drive_sequence,
        participant,
    }
    .encode()
    .unwrap();
    // Completion before the installed stop signal is refused without deleting goals.
    assert!(!send(&mut listener, 6, 47, completed.span()));
    assert!(
        goals
            .iter()
            .all(|g| listener.players.objects().get(*g).is_some())
    );
    let signal = nfs_world::logic::Message::Reached {
        event: 43,
        target: nfs_world::logic::EntityRef {
            ghost: drive_sequence,
            entity: 1,
        },
        player: 0,
    }
    .encode()
    .unwrap();
    assert!(send(&mut listener, 6, 82, signal.span()));
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|r| matches!(r,HostRpc::SequenceStop(s) if s.sequence==drive_sequence))
            .count(),
        1
    );
    drain(&mut listener);
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    assert!(!send(&mut listener, 7, 47, completed.span()));
    assert!(
        goals
            .iter()
            .all(|g| listener.players.objects().get(*g).is_some())
    );
    drain(&mut listener);
    listener.world_ms = 105_000;
    assert!(send(&mut listener, 7, 47, completed.span()));
    assert!(
        goals
            .iter()
            .all(|g| listener.players.objects().get(*g).is_none())
    );
    assert!(listener.players.objects().get(drive_sequence).is_none());
    let methods = listener
        .pending_rpcs
        .iter()
        .filter_map(|r| match r {
            HostRpc::GameMode(c) => Some((c.selector, c.method)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(methods, vec![(1, 1), (2, 0), (2, 3)]);
    let actor = listener
        .players
        .owned_actor(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    let replication::Update::Actor(control) =
        &listener.players.objects().get(actor).unwrap().current
    else {
        panic!()
    };
    assert_eq!(control.flag_a, Some(false));
    let routes = control.references_a.as_ref().unwrap();
    assert_eq!(routes.len(), 1);
    assert!(
        matches!(&listener.players.objects().get(routes[0]).unwrap().initial,replication::Initial::GotoWaypointGoal(route) if route.progress_updates)
    );
    let before_duplicate = listener.players.objects().snapshot();
    drain(&mut listener);
    assert!(send(&mut listener, 8, 47, completed.span()));
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(*items, before);
    assert_eq!(listener.players.objects().snapshot(), before_duplicate);
    // The route clock starts at driving entry, not process or mode creation time.
    let (drive, _) = listener
        .prologue
        .as_ref()
        .unwrap()
        .driving
        .as_ref()
        .unwrap();
    let &(key, serializer) = &drive.data.game_mode.checkpoints[0];
    let (scene, selector) = listener
        .players
        .objects()
        .scene_endpoint(key, serializer)
        .unwrap();
    let crossing = |car, seconds: f64| {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(2, 32)
            .put(seconds.to_bits() >> 32, 32)
            .put(seconds.to_bits() & 0xffff_ffff, 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(scene.into(), 13)
            .put(car, 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    };
    listener.world_ms = 107_000;
    for (sequence, car, seconds) in [
        (9, u64::from(participant), 7.),
        (10, u64::from(vehicle), 4.),
    ] {
        assert!(send(
            &mut listener,
            sequence,
            47,
            crossing(car, seconds).span()
        ));
        assert!(listener.pending_rpcs.is_empty());
        assert_eq!(listener.players.objects().snapshot(), before_duplicate);
    }
    let accepted = crossing(vehicle.into(), 7.);
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    assert!(!send(&mut listener, 11, 47, accepted.span()));
    assert_eq!(listener.players.objects().snapshot(), before_duplicate);
    drain(&mut listener);
    assert!(send(&mut listener, 11, 47, accepted.span()));
    assert!(listener.pending_rpcs.iter().any(|r| matches!(r,HostRpc::GameMode(c) if c.method==2 && c.arguments==nfs_world::game_mode::Arguments::Progress { passed:0,next:0,seconds:2f32.to_bits(),lap:2 })));
    assert!(!listener.intro_modes[&participant].0.arrived());
    let finished_world = listener.players.objects().snapshot();
    drain(&mut listener);
    assert!(send(&mut listener, 12, 47, accepted.span()));
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(listener.players.objects().snapshot(), finished_world);
    listener.world_ms = 107_499;
    listener.advance_prologue().unwrap();
    assert!(!listener.intro_modes[&participant].0.arrived());
    assert!(listener.pending_rpcs.is_empty());
    listener.world_ms = 107_500;
    listener.pending_rpcs.resize(
        nfs_world::session::MAX_RPC_QUEUE,
        HostRpc::SequenceStop(nfs_world::sequences::Stop {
            sequence: drive_sequence,
            serial: Serial::new(1).unwrap(),
        }),
    );
    listener.advance_prologue().unwrap();
    assert!(!listener.intro_modes[&participant].0.arrived());
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert!(listener.intro_modes[&participant].0.arrived());
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter_map(|r| match r {
                HostRpc::GameMode(c) => Some((c.selector, c.method)),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(2, 1), (3, 0)]
    );
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert!(listener.pending_rpcs.is_empty());
    let returning = listener.participants.return_bindings().unwrap();
    let mut preview_source = crate::vehicle_content::population::tests::previews(
        &listener.vehicles.as_ref().unwrap().layout,
        items.clone(),
    );
    // This fixture also exposes the shared ItemBuilder at serializer 3.
    // Reserve distinct serializers for the three vehicle acknowledgements.
    for slot in &mut Arc::make_mut(&mut preview_source).garage.slots {
        std::mem::swap(&mut slot.component, &mut slot.item_component);
    }
    Arc::make_mut(listener.prologue.as_mut().unwrap()).previews = Some(preview_source);
    let enter = request(returning.request, participant, 0);
    listener.progression = Some(Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
    ));
    listener
        .returns
        .as_mut()
        .unwrap()
        .poll_capacity(listener.world_ms, 0)
        .unwrap();
    listener.pending_entries.resize(
        MAX_PENDING,
        (super::super::returning::empty_section(), vec![]),
    );
    assert!(!send(&mut listener, 13, 47, enter.span()));
    assert!(listener.story_pending.is_empty());
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    assert!(
        listener
            .returns
            .as_ref()
            .unwrap()
            .ghost(participant)
            .is_none()
    );
    drain(&mut listener);
    assert!(send(&mut listener, 13, 47, enter.span()));
    assert_eq!(listener.story_pending.len(), 1);
    assert_eq!(
        listener.story_pending[0].action,
        crate::net::items::story::Action::Complete { objective: 42 }
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroEnteringGarage)
    );
    let entry = listener
        .returns
        .as_ref()
        .unwrap()
        .ghost(participant)
        .unwrap();
    let before_previews = listener.players.objects().snapshot();
    // The entry notification is an ordering barrier for later preview cars.
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_previews);
    drain(&mut listener);
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_previews);
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|r| matches!(r, HostRpc::Vehicle(_)))
            .count(),
        3
    );
    let preview_bindings = listener
        .pending_rpcs
        .iter()
        .filter_map(|r| match r {
            HostRpc::Vehicle(b) => Some(*b),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listener
            .players
            .owned_vehicles(HOST_SELECTOR as u8, listener.persona),
        listener
            .pending_rpcs
            .iter()
            .filter_map(|r| match r {
                HostRpc::Vehicle(b) => Some(b.vehicle),
                _ => None,
            })
            .chain([vehicle])
            .collect()
    );
    let live = listener.players.objects().snapshot();
    drain(&mut listener);
    assert!(send(&mut listener, 14, 47, enter.span()));
    assert_eq!(listener.players.objects().snapshot(), live);
    assert!(listener.pending_entries.is_empty());
    let complete = Completed {
        sequence: entry,
        participant,
    }
    .encode()
    .unwrap();
    assert!(!send(&mut listener, 15, 47, complete.span()));
    assert_eq!(listener.players.objects().snapshot(), live);
    listener.world_ms += 7500;
    let stops = listener
        .returns
        .as_mut()
        .unwrap()
        .poll_capacity(listener.world_ms, 1)
        .unwrap();
    assert_eq!(stops.len(), 1);
    listener.pending_entries.resize(
        MAX_PENDING,
        (super::super::returning::empty_section(), vec![]),
    );
    assert!(!send(&mut listener, 15, 47, complete.span()));
    assert!(listener.players.objects().get(vehicle).is_some());
    drain(&mut listener);
    assert!(send(&mut listener, 15, 47, complete.span()));
    assert_eq!(
        listener
            .pending_entries
            .iter()
            .flat_map(|(_, calls)| calls)
            .filter(|call| matches!(call, HostRpc::GaragePresence(p) if p.enabled))
            .count(),
        1
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    assert!(listener.players.objects().get(vehicle).is_none());
    assert!(listener.players.objects().get(entry).is_none());
    assert!(!listener.world_cars.contains_key(&vehicle));
    assert!(
        listener
            .sequences
            .as_ref()
            .unwrap()
            .ghost(participant)
            .is_some()
    );
    assert_eq!(*items, before);
    let waiting = listener
        .sequences
        .as_ref()
        .unwrap()
        .ghost(participant)
        .unwrap();
    drain(&mut listener);
    listener.advance_sequences(listener.world_ms).unwrap();
    let complete = Completed {
        sequence: waiting,
        participant,
    }
    .encode()
    .unwrap();
    assert!(!send(&mut listener, 16, 47, complete.span()));
    for (i, b) in preview_bindings.iter().enumerate() {
        let ack = b.expected().encode().unwrap();
        assert!(send(&mut listener, 16 + i as u8, 47, ack.span()));
    }
    listener.advance_sequences(listener.world_ms).unwrap();
    assert!(
        listener
            .pending_rpcs
            .iter()
            .all(|r| !matches!(r, HostRpc::SequenceStop(_)))
    );
    listener
        .advance_sequences(listener.world_ms + nfs_world::sequences::FIRST_READY_MS)
        .unwrap();
    assert!(
        listener
            .pending_rpcs
            .iter()
            .any(|r| matches!(r, HostRpc::SequenceStop(s) if s.sequence == waiting))
    );
    drain(&mut listener);
    assert!(send(&mut listener, 19, 47, complete.span()));
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    let before_selection = listener.players.objects().snapshot();
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_selection);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroCarSelection)
    );
    assert!(listener.players.objects().get(waiting).is_none());
    assert!(listener.selection.ghost(participant).is_some());
    assert!(listener.selection_execution.contains_key(&participant));
    assert_eq!(listener.pending_rpcs.iter().filter(|r| matches!(r, HostRpc::Event(f) if f.event == 7 && Some(f.target.ghost) == listener.selection.ghost(participant))).count(), 1);
    let selected = listener.players.objects().snapshot();
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), selected);
    assert!(listener.pending_rpcs.is_empty());
}

#[test]
fn full_output_queue_preserves_the_car_allocation_and_readiness_gate() {
    let (mut listener, participant, _) = setup();
    let before = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before);
    assert!(
        listener
            .players
            .temporary_vehicle(HOST_SELECTOR as u8, listener.persona, participant)
            .is_none()
    );
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    let vehicle = listener
        .players
        .temporary_vehicle(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    drain(&mut listener);
    let before = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    let glass = nfs_world::logic::Message::Reached {
        event: 21_578_436,
        target: nfs_world::logic::EntityRef {
            ghost: vehicle,
            entity: 2,
        },
        player: 0,
    }
    .encode()
    .unwrap();
    assert!(!send(&mut listener, 0, 82, glass.span()));
    assert_eq!(listener.players.objects().snapshot(), before);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroSpawning)
    );
    assert!(listener.pending_teleports.is_empty());
    drain(&mut listener);
    assert!(send(&mut listener, 0, 82, glass.span()));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroTeleporting)
    );
}
