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

use crate::net::players::test_wire::client_padding;

#[test]
fn story_opponents_are_delegated_and_retire_after_departures_atomically() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (players, owner, mode, active) =
            crate::prologue::driving::opponents::tests::finished_fixture();
        assert_eq!(owner.connection, HOST_SELECTOR as u8);
        let (template, _, _) = setup();
        let mut content = (**template.prologue.as_ref().unwrap()).clone();
        content.driving = Some((mode.clone(), mode));
        let cars = active.delegated(&players);
        let root = active.ghost;
        let mut listener =
            PlayerListener::new(owner.persona).with_prologue(Some(Arc::new(content)));
        listener.players = players;
        attach_location_service(&mut listener);
        listener.replication_origin_ms = Some(100_000);
        listener.world_ms = 112_100;
        listener.intro_modes.insert(
            owner.participant,
            (active.clone(), active, Chapter::FirstDrive),
        );
        for &car in cars.keys() {
            assert!(listener.movement_objects().contains(&car));
            assert!(
                listener
                    .rival_glass_owner(&listener.players, car, 0)
                    .is_some()
            );
        }
        assert_eq!(
            listener
                .rival_map(&listener.players, &listener.intro_modes)
                .unwrap()
                .unwrap()
                .len(),
            2
        );
        let leave = request_with_padding(
            Endpoint {
                scene: root,
                selector: 0,
                serial: Serial::new(1).unwrap(),
            },
            owner.participant,
            0,
            0b1101101,
        );
        listener.pending_entries.resize(
            MAX_PENDING,
            (super::super::returning::empty_section(), vec![]),
        );
        let before = listener.players.objects().snapshot();
        assert!(!send(&mut listener, 0, 47, leave.span()));
        assert_eq!(listener.players.objects().snapshot(), before);
        assert!(!listener.intro_modes[&owner.participant].0.has_left());
        drain(&mut listener);
        assert!(send(&mut listener, 0, 47, leave.span()));
        assert!(listener.intro_modes[&owner.participant].0.has_left());
        assert!(listener.delegated_ai(&listener.players).is_empty());
        assert!(listener.pending.iter().all(|s| s.deleted.is_empty()));
        let first_delete = listener
            .pending_entries
            .iter()
            .position(|(s, _)| !s.deleted.is_empty())
            .unwrap();
        assert!(first_delete > 0);
        let departures = listener
            .pending_entries
            .iter()
            .take(first_delete)
            .flat_map(|(_, calls)| calls)
            .count();
        assert!(departures >= 4);
        for (section, calls) in &listener.pending_entries {
            assert!(
                nfs_world::session::replication_frame(section, calls, 0)
                    .unwrap()
                    .len()
                    <= nfs_world::application::OUTBOUND_FRAME_BITS
            );
        }
        drain(&mut listener);
        assert!(send(&mut listener, 1, 47, leave.span()));
        assert!(listener.pending_entries.is_empty());
        assert_eq!(
            listener.optional_map_failures, [0; 3],
            "retirement must be coherent, not merely contained"
        );
    });
}

fn attach_location_service(listener: &mut PlayerListener) {
    use nfs_world::{
        replication::sublevel::ordinary,
        world_activation::{Catalog as Activation, Site},
    };
    let key = 1999;
    let profiles = sublevel::Content::new(vec![(
        key,
        Profile::new(
            false,
            &[
                Kind::Noop,
                Kind::RpcBool,
                Kind::Rpc,
                Kind::RpcFourReferences,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    let scene = ordinary::creation(
        99,
        key,
        None,
        Serial::new(0).unwrap(),
        &ordinary::unpopulated(profiles.profile(key).unwrap()).unwrap(),
        &profiles,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[99], vec![scene], &profiles)
        .unwrap();
    let asset = Asset {
        bundle: 1,
        type_id: 1,
        local_index: 0,
    };
    listener.roles = Some(crate::scene_roles::SceneRoles {
        level: b"test".to_vec(),
        gameplay: 900,
        startup: 901,
        garage: 902,
        progression: 903,
        traffic: [0; 10],
        customization_timer: asset,
        streaming_gate: asset,
        spawn_points: None,
        bank: None,
        free_roam: Some(Activation {
            volumes: vec![],
            presence: Site {
                scene_key: key,
                serializer: 1,
            },
            streamed: Site {
                scene_key: key,
                serializer: 2,
            },
            location: Site {
                scene_key: key,
                serializer: 3,
            },
        }),
    });
}

fn request(endpoint: Endpoint, participant: u16, method: u32) -> BitWriter {
    request_with_padding(endpoint, participant, method, client_padding() as u8)
}
pub(super) fn native_completion(call: Completed) -> BitWriter {
    let wire = call.encode().unwrap();
    let mut padded = BitWriter::new();
    padded
        .put_span(wire.span().slice(0, wire.span().len() - 7).unwrap())
        .put(client_padding(), 7);
    padded
}
fn request_with_padding(
    endpoint: Endpoint,
    participant: u16,
    method: u32,
    padding: u8,
) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(method.into(), 32)
        .put(u64::from(padding), 7);
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
pub(super) fn send(
    listener: &mut PlayerListener,
    sequence: u8,
    index: u8,
    body: BitSpan<'_>,
) -> bool {
    let wire = packet(sequence, index, body);
    let before = crate::net::players::test_wire::queued(listener);
    let accepted = listener.frame(Queued {
        advertised: None,
        frame: sequence.into(),
        data: wire.span(),
    });
    crate::net::players::test_wire::assert_wire_ready(listener, before);
    accepted
}
/// One sequence-clock update `after_ms` past the listener's world time, with
/// its output checked like a frame's.
fn sequences_checked(listener: &mut PlayerListener, after_ms: u64) {
    let before = crate::net::players::test_wire::queued(listener);
    let now = listener.world_ms + after_ms;
    listener.advance_sequences(now).unwrap();
    crate::net::players::test_wire::assert_wire_ready(listener, before);
}
/// Hand every queued notification and entry to the session the way the world
/// loop does (replication first, then RPCs, then entries); returns the calls.
pub(in crate::net::players) fn send_all(listener: &mut PlayerListener) -> Vec<HostRpc> {
    listener.pending_content.clear();
    listener.pending.clear();
    let mut sent = Vec::new();
    while let Some(&rpc) = listener.pending_rpcs.front() {
        listener.rpc_sent();
        sent.push(rpc);
    }
    while let Some((_, calls)) = listener.pending_entries.front() {
        sent.extend(calls.iter().copied());
        listener.entry_sent();
    }
    sent
}
pub(super) fn drain(listener: &mut PlayerListener) {
    listener.pending.clear();
    listener.pending_rpcs.clear();
    listener.pending_content.clear();
    listener.pending_entries.clear();
}
/// Exercise the same update entry point as the live world connection, with
/// ordinary garage presentation configured but never spawned by the intro.
pub(in crate::net::players) fn tick_with_normal_presentation(
    listener: &mut PlayerListener,
) -> Result<(), replication::Error> {
    assert!(listener.logic.is_some());
    let before = crate::net::players::test_wire::queued(listener);
    let now = listener.world_ms;
    listener.advance_sequences(now)?;
    listener.advance_level_poll(now)?;
    listener.advance_settled(now)?;
    listener.advance_traffic(now)?;
    listener.advance_rivals(now)?;
    listener.advance_scoring(now)?;
    listener.advance_events(now)?;
    crate::net::players::test_wire::assert_wire_ready(listener, before);
    Ok(())
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
    let mut story = crate::prologue::Content::for_test(
        items.clone(),
        parent,
        Component {
            scene_key: layout.item_scene_key,
            index: 3,
        },
    );
    story.membership = Some(crate::prologue::membership::Content::for_test(909, 1..117));
    let mut listener = PlayerListener::new(persona)
        .with_vehicles(Some(Arc::new(GarageContent { vehicles, layout })))
        .with_prologue(Some(Arc::new(story)));
    listener.players = players;
    listener.replication_origin_ms = Some(100_000);
    listener.world_ms = 100_000;
    listener.population = Some(population);
    listener.participants = participants;
    // The ordinary garage logic lives on the layout's item scene, as the
    // installed content's gameplay scene does (the intro's customization
    // creates the entry presentation there).
    let key = listener.vehicles.as_ref().unwrap().layout.item_scene_key;
    let json: serde_json::Value = crate::garage_logic::tests::presentation_json()
        .to_string()
        .replace(
            "\"scene_content_key\":101",
            &format!("\"scene_content_key\":{key}"),
        )
        .parse()
        .unwrap();
    listener.logic = Some(Arc::new(
        crate::garage_logic::GarageLogic::from_json(&json).unwrap(),
    ));
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
    crate::net::players::test_wire::with_native_padding(|| {
        ready_for_selection(false);
    });
}
pub(super) fn ready_for_selection(
    with_services: bool,
) -> (PlayerListener, u16, Arc<nfs_world::items::Collection>) {
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
    tick_with_normal_presentation(&mut listener).unwrap();
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
        event: nfs_world::sequences::STREAMING_UNLOADED,
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
    tick_with_normal_presentation(&mut listener).unwrap();
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
    let opening_enter = HostRpc::Participant(nfs_world::participants::Notification {
        endpoint: bindings.opening,
        participant,
        call: nfs_world::participants::Call::Enter,
    });
    assert!(
        !listener.pending_rpcs.contains(&opening_enter),
        "opening entry must wait for sequence creation publication"
    );
    listener.pending_entries.resize(
        MAX_PENDING,
        (super::super::returning::empty_section(), Vec::new()),
    );
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.opening.ghost(participant).is_none());
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    let ghost = listener.opening.ghost(participant).unwrap();
    let creation = listener
        .pending_entries
        .iter()
        .position(|(section, _)| section.records.iter().any(|r| r.id == ghost))
        .expect("opening creation is an ordered entry");
    let entered = listener
        .pending_entries
        .iter()
        .position(|(_, calls)| calls.contains(&opening_enter))
        .expect("opening entry is queued after creation");
    assert!(creation < entered);
    assert!(!listener.pending_rpcs.contains(&opening_enter));
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
    let completed = native_completion(Completed {
        sequence: ghost,
        participant,
    });
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
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_mode);
    assert!(listener.intro_modes.is_empty());
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending.is_empty());
    assert!(!listener.free_roam_presented.contains(&participant));
    assert!(listener.desired_scoring(false).is_empty());
    // Normal crash recovery must not take over the story's temporary car when
    // the opening hands off to driving. Exercise a real movement crash pulse.
    let asset = Asset {
        bundle: 61,
        type_id: 700,
        local_index: 0,
    };
    let mut crashes = nfs_world::crash::Crashes::new(
        nfs_world::crash::Definition {
            asset,
            manager: Component {
                scene_key: listener.vehicles.as_ref().unwrap().layout.item_scene_key,
                index: 3,
            },
            embedded_bus: 2,
        },
        Content::new(
            vec![Catalog::new(61, &[(700, 1)]).unwrap()],
            vec![(asset, nfs_world::crash::profile())],
        )
        .unwrap(),
        4500,
    )
    .unwrap();
    let physics = listener
        .players
        .vehicle_physics(HOST_SELECTOR as u8, listener.persona, vehicle)
        .unwrap();
    for revision in [1, 2] {
        let mut sample = replication::vehicle::movement::Sample {
            physics: physics.clone(),
            flag: true,
        };
        sample.physics.identity_words[2] = revision;
        sample.physics.controls.flags[0] = revision == 2;
        let (header, body) = sample.encode().unwrap();
        PlayerListener::observe_crashes(
            &mut crashes,
            &listener.players,
            &listener.participants,
            &listener.world_cars,
            listener.persona,
            &frame::Movement {
                header: Some(header.span()),
                records: vec![frame::MovementRecord {
                    id: vehicle,
                    component: 0,
                    data: body.span(),
                }],
            },
            (&listener.event_state, None),
        )
        .unwrap();
    }
    assert!(crashes.pending().is_empty());
    let completed = native_completion(Completed {
        sequence: drive_sequence,
        participant,
    });
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
    // E1180: this frame closed the world when its records had no setup. As in
    // the official completion frame, the sequence and goal deletions travel
    // with the solo-route creation in one sendable section.
    let completion = listener
        .pending
        .iter()
        .find(|s| s.deleted.contains(&drive_sequence))
        .unwrap();
    assert!(completion.setup.is_some() && !completion.records.is_empty());
    assert!(goals.iter().all(|g| completion.deleted.contains(g)));
    assert!(
        completion
            .records
            .iter()
            .any(|r| matches!(r.initial, Some(replication::Initial::GotoWaypointGoal(_))))
    );
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
    tick_with_normal_presentation(&mut listener).unwrap();
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
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(!listener.intro_modes[&participant].0.arrived());
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
        vec![(2, 1), (3, 0), (0, 1), (0, 7)]
    );
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
    if with_services {
        let definition = super::selection::tests::program(&listener);
        Arc::make_mut(listener.prologue.as_mut().unwrap()).selection_program = Some(definition);
    }
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
    assert!(
        matches!(&listener.players.objects().get(loading).unwrap().initial,
        replication::Initial::Entity { fields, .. }
            if fields[1] == entity::Initial::BoolProperty(Some(false))),
        "a rejected return must not enable streaming"
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
    let replication::Initial::Entity { fields, .. } =
        &listener.players.objects().get(loading).unwrap().initial
    else {
        panic!("garage streaming object disappeared")
    };
    assert_eq!(
        fields[1],
        entity::Initial::BoolProperty(Some(true)),
        "the host must enable garage streaming before expecting a loaded report"
    );
    assert!(
        listener
            .pending
            .iter()
            .chain(listener.pending_entries.iter().map(|(s, _)| s))
            .flat_map(|s| &s.records)
            .any(|record| record.id == loading
                && matches!(&record.update, replication::Update::Entity { fields, .. }
                if fields[1] == Some(entity::Update::BoolProperty(Some(true))))),
        "the streaming flag must actually be queued for the client"
    );
    // The captured client reports loaded only after the synchronized flag.
    // Feeding it at initial spawn hid the missing host update.
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
    let streaming_frame = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 2,
                sequence: Some(1),
                messages: vec![frame::Message::Opaque {
                    index: 82,
                    body: streamed.span(),
                }],
            }],
        )
        .build()
        .unwrap();
    let queued = crate::net::players::test_wire::queued(&listener);
    assert!(listener.frame(Queued {
        advertised: None,
        frame: 13,
        data: streaming_frame.span()
    }));
    crate::net::players::test_wire::assert_wire_ready(&listener, queued);
    let entry = listener
        .returns
        .as_ref()
        .unwrap()
        .ghost(participant)
        .unwrap();
    // The cinematic Actor takes its scripted-control flag before the spline
    // goal is assigned (E1186 review; official career start 289.819 ms).
    let actor = listener
        .players
        .owned_actor(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    let flat: Vec<&replication::Record> = listener
        .pending_entries
        .iter()
        .flat_map(|(section, _)| section.records.iter())
        .collect();
    let flag = flat
        .iter()
        .position(|r| {
            r.id == actor
                && matches!(&r.update, replication::Update::Actor(a) if a.flag_a == Some(true))
        })
        .unwrap_or_else(|| panic!("no scripted-control flag record for actor {actor}"));
    let goal = flat
        .iter()
        .position(|r| matches!(r.initial, Some(replication::Initial::ScriptedSplineGoal(_))))
        .expect("a spline goal record");
    assert!(
        flag < goal,
        "the scripted-control flag precedes the spline goal"
    );
    let before_previews = listener.players.objects().snapshot();
    // The entry notification is an ordering barrier for later preview cars.
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_previews);
    drain(&mut listener);
    listener
        .pending
        .resize(MAX_PENDING, super::super::returning::empty_section());
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_previews);
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
    let complete = native_completion(Completed {
        sequence: entry,
        participant,
    });
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
    sequences_checked(&mut listener, 0);
    let complete = native_completion(Completed {
        sequence: waiting,
        participant,
    });
    assert!(!send(&mut listener, 16, 47, complete.span()));
    for (i, b) in preview_bindings.iter().enumerate() {
        let ack = b.expected().encode().unwrap();
        assert!(send(&mut listener, 16 + i as u8, 47, ack.span()));
    }
    // The intro garage wait stops 8 s after readiness (E1026), not 16 s.
    sequences_checked(
        &mut listener,
        nfs_world::sequences::INTRO_FIRST_READY_MS - 1,
    );
    assert!(
        listener
            .pending_rpcs
            .iter()
            .all(|r| !matches!(r, HostRpc::SequenceStop(_)))
    );
    sequences_checked(&mut listener, nfs_world::sequences::INTRO_FIRST_READY_MS);
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
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), before_selection);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
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
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), selected);
    assert!(listener.pending_rpcs.is_empty());
    (listener, participant, items)
}

#[test]
fn full_output_queue_preserves_the_car_allocation_and_readiness_gate() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant, _) = setup();
        let asset = Asset {
            bundle: 32,
            type_id: 10,
            local_index: 0,
        };
        let story = Arc::make_mut(listener.prologue.as_mut().unwrap());
        story.membership = Some(story.membership.take().unwrap().with_test_presentation(
            nfs_content::participant_logic::Layout {
                asset,
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
                        kind: entity::Kind::RpcReference,
                        channel: Some(nfs_content::participant_logic::Channel::Scene {
                            content_key: 909,
                            serializer: 1,
                        }),
                    },
                    nfs_content::participant_logic::Field {
                        kind: entity::Kind::FloatProperty,
                        channel: None,
                    },
                ],
            },
            &[3, 77],
        ));
        let before = listener.players.objects().snapshot();
        listener
            .pending
            .resize(MAX_PENDING, super::super::returning::empty_section());
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.players.objects().snapshot(), before);
        assert!(listener.scene_membership.is_none());
        assert!(
            listener
                .players
                .temporary_vehicle(HOST_SELECTOR as u8, listener.persona, participant)
                .is_none()
        );
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert!(
            listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(3, participant)
        );
        let presentation = listener.players.objects().iter().find_map(|(id, object)|
            matches!(&object.initial, replication::Initial::Entity { prefix, .. } if prefix.asset == asset).then_some(id)).unwrap();
        assert!(
            listener
                .pending
                .iter()
                .flat_map(|section| &section.records)
                .any(|record| record.id == presentation && record.initial.is_some())
        );
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
        assert!(
            listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(3, participant)
        );
        assert!(
            !listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(77, participant)
        );
        assert_eq!(listener.players.objects().snapshot(), before);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroSpawning)
        );
        assert!(listener.pending_teleports.is_empty());
        drain(&mut listener);
        assert!(send(&mut listener, 0, 82, glass.span()));
        assert!(
            !listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(3, participant)
        );
        assert!(
            listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(77, participant)
        );
        assert!(
            listener.players.objects().get(presentation).is_some(),
            "a move within the installed list retains the same object"
        );
        assert!(
            !listener
                .pending
                .iter()
                .flat_map(|section| &section.records)
                .any(|record| record.id == presentation)
        );
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroTeleporting)
        );
        let endpoint = listener.participants.intro_bindings().unwrap().teleport;
        drain(&mut listener);
        assert!(send(
            &mut listener,
            1,
            47,
            request(endpoint, participant, 3).span()
        ));
        assert!(
            listener
                .scene_membership
                .as_ref()
                .unwrap()
                .contains(78, participant)
        );
        assert!(listener.players.objects().get(presentation).is_none());
        assert!(
            listener
                .pending
                .iter()
                .any(|section| section.deleted.contains(&presentation)),
            "leaving the list must queue the owned object's deletion"
        );
        drain(&mut listener);
        assert!(send(
            &mut listener,
            2,
            47,
            request(endpoint, participant, 3).span()
        ));
        assert!(
            listener
                .pending
                .iter()
                .all(|section| section.deleted.is_empty()),
            "repeated acknowledgement cannot retire twice"
        );
    });
}

#[test]
fn selected_car_reuses_native_startup_and_waits_for_previous_mode_retirement() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant) = super::selection::tests::selected(true);
        let vehicles = Arc::make_mut(listener.vehicles.as_mut().unwrap());
        vehicles.layout = crate::vehicle_content::population::tests::world_layout(&vehicles.layout);
        let persona = listener.persona;
        let inventory = listener.inventory.clone().unwrap();
        let item = inventory.slots.values()[0].unwrap();
        let garage = listener
            .players
            .owned_vehicle(HOST_SELECTOR as u8, persona, participant, item)
            .unwrap();
        let previous = listener.intro_modes[&participant].0.ghost;
        let canceller = listener.intro_modes[&participant].1.ghost;
        assert!(!listener.intro_modes[&participant].0.has_left());
        let before = listener.players.objects().snapshot();
        listener
            .pending
            .resize(MAX_PENDING, super::super::returning::empty_section());
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.players.objects().snapshot(), before);
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        let car = listener
            .players
            .owned_vehicle(HOST_SELECTOR as u8, persona, participant, item)
            .unwrap();
        assert_ne!(car, garage);
        assert_eq!(
            listener
                .players
                .vehicle_item_of(HOST_SELECTOR as u8, persona, car),
            Some(item)
        );
        assert!(
            listener
                .players
                .temporary_vehicle(HOST_SELECTOR as u8, persona, participant)
                .is_none()
        );
        assert_eq!(listener.world_cars[&car], (participant, false));
        assert!(
            listener
                .pending
                .iter()
                .flat_map(|s| &s.deleted)
                .any(|&id| id == garage)
        );
        drain(&mut listener);
        let world = listener.players.objects().snapshot();
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.players.objects().snapshot(), world);
        let glass = nfs_world::logic::Message::Reached {
            event: 21_578_436,
            target: nfs_world::logic::EntityRef {
                ghost: car,
                entity: 2,
            },
            player: 0,
        }
        .encode()
        .unwrap();
        listener
            .pending
            .resize(MAX_PENDING, super::super::returning::empty_section());
        assert!(!send(&mut listener, 28, 82, glass.span()));
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroSpawning)
        );
        assert!(listener.pending_teleports.is_empty());
        drain(&mut listener);
        assert!(send(&mut listener, 28, 82, glass.span()));
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroTeleporting)
        );
        assert_eq!(
            listener.pending_teleports.front().unwrap().1.position,
            [100., 20., 300.]
        );
        assert_eq!(listener.world_cars[&car], (participant, true));
        assert!(listener.pending_spawns.is_empty());
        drain(&mut listener);
        assert!(send(&mut listener, 29, 82, glass.span()));
        assert_eq!(listener.pending_teleports.len(), 1);
        drain(&mut listener);
        let bindings = listener.participants.intro_bindings().unwrap();
        assert!(send(
            &mut listener,
            30,
            47,
            request(bindings.teleport, participant, 3).span()
        ));
        drain(&mut listener);
        assert!(send(
            &mut listener,
            31,
            47,
            request(bindings.render_ready, participant, 0).span()
        ));
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        let arrival = listener.opening.ghost(participant).unwrap();
        let replication::Initial::Entity { fields, .. } =
            &listener.players.objects().get(arrival).unwrap().initial
        else {
            panic!()
        };
        assert!(matches!(
            fields.first(),
            Some(entity::Initial::SequenceRoot { stopping: true, .. })
        ));
        assert_eq!(
            listener
                .pending_rpcs
                .iter()
                .filter(|r| matches!(r, HostRpc::SequenceStop(s) if s.sequence == arrival))
                .count(),
            0
        );
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroOpening)
        );
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert!(listener.pending.is_empty());
        assert!(listener.pending_rpcs.is_empty());
        let complete = native_completion(Completed {
            sequence: arrival,
            participant,
        });
        assert!(send(&mut listener, 32, 47, complete.span()));
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.intro_modes[&participant].0.ghost, previous);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroStartingDrive)
        );
        let leave = request(
            Endpoint {
                scene: previous,
                selector: 0,
                serial: Serial::new(1).unwrap(),
            },
            participant,
            0,
        );
        assert!(send(&mut listener, 33, 47, leave.span()));
        assert!(listener.intro_modes[&participant].0.has_left());
        drain(&mut listener);
        listener
            .pending
            .resize(MAX_PENDING, super::super::returning::empty_section());
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.intro_modes[&participant].0.ghost, previous);
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_ne!(listener.intro_modes[&participant].0.ghost, previous);
        assert_eq!(listener.intro_modes[&participant].1.ghost, canceller);
        assert_eq!(listener.intro_modes[&participant].2, Chapter::FirstRace);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::FreeRoam)
        );
        assert_eq!(
            *listener.inventory.as_ref().unwrap().items,
            *inventory.items
        );
    });
}

#[test]
fn garage_completion_entries_split_to_frame_size_and_carry_every_call() {
    // E1186: the intro garage-entry completion packed every record into one
    // exit entry; with installed content it exceeded OUTBOUND_FRAME_BITS and
    // the exit-entry check then refused the client's resent completion
    // forever. The entries must split, keep the deletion on the first section
    // and carry every call within the frame and reply-group bounds.
    let record = |id: u16| replication::Record {
        id,
        initial: Some(replication::Initial::Actor(replication::ActorInit {
            participant: 7,
            flag_a: true,
            flag_b: true,
            word_a: 4,
            word_b: 65535,
        })),
        update: replication::Update::Actor(replication::ActorUpdate::default()),
    };
    let call = |i: u16| {
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: Endpoint {
                scene: 2,
                selector: i,
                serial: Serial::new(1).unwrap(),
            },
            participant: 7,
            call: nfs_world::participants::Call::Enter,
        })
    };
    // The session's own admission measurement, timestamp included, not a
    // copy of it.
    let fits = crate::net::players::entry_admitted;
    // Empty output produces no entries at all.
    assert!(
        PlayerListener::garage_entries(Vec::new(), Vec::new(), Vec::new())
            .unwrap()
            .is_empty()
    );
    // Deletions without records ride one entry on their own.
    let entries = PlayerListener::garage_entries(Vec::new(), vec![9, 10], Vec::new()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.deleted, [9, 10]);
    assert!(entries[0].0.records.is_empty());
    assert!(entries[0].1.is_empty());
    // Small output keeps the single-entry wire shape of the old code.
    let entries =
        PlayerListener::garage_entries(vec![record(1), record(2)], vec![5], vec![call(3)]).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.setup, Some(Setup::RawEscape([0; 3])));
    assert_eq!(entries[0].0.deleted, [5]);
    assert_eq!(entries[0].0.records.len(), 2);
    assert_eq!(entries[0].1.len(), 1);
    assert!(fits(&entries[0].0, &entries[0].1));
    // Deployment-sized output splits; nothing is lost or reordered.
    let records = (1..=600u16).map(record).collect::<Vec<_>>();
    let calls = (0..40u16).map(call).collect::<Vec<_>>();
    let deleted = (1000..1030u16).collect::<Vec<_>>();
    let entries =
        PlayerListener::garage_entries(records.clone(), deleted.clone(), calls.clone()).unwrap();
    assert!(entries.len() > 1, "the deployment-sized output must split");
    let mut seen_records = Vec::new();
    let mut seen_calls = Vec::new();
    for (index, (section, chunk)) in entries.iter().enumerate() {
        assert!(fits(section, chunk), "entry {index} must fit one frame");
        assert!(
            chunk.len() <= 15,
            "entry {index} must respect the reply bound"
        );
        assert_eq!(
            section.deleted,
            if index == 0 { deleted.as_slice() } else { &[] }
        );
        seen_records.extend(section.records.iter().map(|r| r.id));
        seen_calls.extend(chunk.iter().copied());
    }
    assert_eq!(seen_records, (1..=600u16).collect::<Vec<_>>());
    assert_eq!(
        seen_calls
            .iter()
            .map(|c| match c {
                HostRpc::Participant(n) => n.endpoint.selector,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>(),
        (0..40u16).collect::<Vec<_>>()
    );
    let sections = entries.len();
    assert!(
        entries[..sections - 1]
            .iter()
            .any(|(s, _)| !s.records.is_empty()),
        "records precede any call-only fallback entry"
    );
    // The same records go through the world loop's flush into a real
    // session: every entry is admitted. The session also checks bindings, so
    // calls (unbound scenes) and deletions (objects it never created) stay out
    // of this part; their fit is measured above.
    let mut listener = PlayerListener::new(0x100000002);
    listener
        .queue_entries(PlayerListener::garage_entries(records, Vec::new(), Vec::new()).unwrap());
    let queued = listener.pending_entries.len();
    let mut host = crate::net::players::test_wire::session_host();
    crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap();
    assert!(listener.pending_entries.is_empty());
    assert!(queued > 1, "the records span several entries");
    assert_eq!(host.replication_progress(), (0, queued));
}

/// Every input record, deletion and call leaves in exactly one entry, in
/// input order, whatever the ratio of calls to sections (the split once
/// returned when the calls ran out and dropped the record sections after the
/// last call).
#[test]
fn garage_completion_entries_keep_every_record_and_call_once_in_order() {
    let record = |id: u16| replication::Record {
        id,
        initial: Some(replication::Initial::Actor(replication::ActorInit {
            participant: 7,
            flag_a: true,
            flag_b: true,
            word_a: 4,
            word_b: 65535,
        })),
        update: replication::Update::Actor(replication::ActorUpdate::default()),
    };
    let call = |i: u16| {
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: Endpoint {
                scene: 2,
                selector: i,
                serial: Serial::new(1).unwrap(),
            },
            participant: 7,
            call: nfs_world::participants::Call::Enter,
        })
    };
    let selector = |c: &HostRpc| match c {
        HostRpc::Participant(n) => n.endpoint.selector,
        _ => unreachable!(),
    };
    for records in [0u16, 1, 219, 220, 600] {
        for calls in [0u16, 1, 15, 16, 40] {
            for deletions in [0u16, 2] {
                let ids = (1..=records).collect::<Vec<_>>();
                let deleted = (1000..1000 + deletions).collect::<Vec<_>>();
                let entries = PlayerListener::garage_entries(
                    ids.iter().copied().map(record).collect(),
                    deleted.clone(),
                    (0..calls).map(call).collect(),
                )
                .unwrap();
                let shape = format!("{records} records, {calls} calls, {deletions} deletions");
                let out_ids = entries
                    .iter()
                    .flat_map(|(s, _)| s.records.iter().map(|r| r.id))
                    .collect::<Vec<_>>();
                assert_eq!(out_ids, ids, "{shape}: records lost, repeated or reordered");
                let out_calls = entries
                    .iter()
                    .flat_map(|(_, c)| c.iter().map(selector))
                    .collect::<Vec<_>>();
                assert_eq!(out_calls, (0..calls).collect::<Vec<_>>(), "{shape}: calls");
                let out_deleted = entries
                    .iter()
                    .flat_map(|(s, _)| s.deleted.iter().copied())
                    .collect::<Vec<_>>();
                assert_eq!(out_deleted, deleted, "{shape}: deletions");
                if let Some((first, _)) = entries.first() {
                    assert_eq!(
                        first.deleted, deleted,
                        "{shape}: deletions ride the first entry"
                    );
                }
                for (index, (section, chunk)) in entries.iter().enumerate() {
                    assert!(chunk.len() <= 15, "{shape}: entry {index} reply bound");
                    assert!(
                        crate::net::players::entry_admitted(section, chunk),
                        "{shape}: entry {index} is not admitted"
                    );
                }
            }
        }
    }
}

/// Presentation adds events to an entry after it was sized: the E1188
/// garage completion went from fifteen calls to eighteen and the session
/// refused it (E1191, found by the offline replay). Over-long entries are
/// re-split: the section keeps the leading calls that fit, call-only entries
/// carry the rest, in order, and every entry is admitted.
#[test]
fn entries_past_a_reply_group_are_resplit_in_order() {
    let record = |id: u16| replication::Record {
        id,
        initial: Some(replication::Initial::Actor(replication::ActorInit {
            participant: 7,
            flag_a: true,
            flag_b: true,
            word_a: 4,
            word_b: 65535,
        })),
        update: replication::Update::Actor(replication::ActorUpdate::default()),
    };
    let call = |i: u16| {
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: Endpoint {
                scene: 10,
                selector: i,
                serial: Serial::new(1).unwrap(),
            },
            participant: 179,
            call: nfs_world::participants::Call::Enter,
        })
    };
    let mut section = returning::empty_section();
    section.setup = Some(Setup::RawEscape([0; 3]));
    section.records = vec![record(181), record(177), record(179)];
    section.deleted = vec![202, 203, 188, 201];
    let calls = (0..18u16).map(call).collect::<Vec<_>>();
    assert!(!crate::net::players::entry_admitted(&section, &calls));
    let entries = crate::net::players::admitted_entries(section.clone(), calls.clone()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, section);
    assert!(entries[1].0.records.is_empty() && entries[1].0.deleted.is_empty());
    assert_eq!(
        entries
            .iter()
            .flat_map(|(_, c)| c.iter().copied())
            .collect::<Vec<_>>(),
        calls,
        "calls lost, repeated or reordered"
    );
    for (section, chunk) in &entries {
        assert!(crate::net::players::entry_admitted(section, chunk));
    }
    // An admitted entry is left as it is.
    let small =
        crate::net::players::admitted_entries(section.clone(), calls[..15].to_vec()).unwrap();
    assert_eq!(small, vec![(section, calls[..15].to_vec())]);
}

/// What the session enforces when it applies entries in order: an entry's
/// calls run before its deletions, so no call may name an object an earlier
/// entry deleted (UnknownObject); nothing is deleted twice.
fn assert_no_call_after_its_deletion(entries: &[(Section, Vec<HostRpc>)]) {
    let mut deleted = std::collections::BTreeSet::new();
    for (index, (section, calls)) in entries.iter().enumerate() {
        for call in calls {
            for object in call.objects() {
                assert!(
                    !deleted.contains(&object),
                    "entry {index} calls {call:?} on object {object} deleted by an earlier entry"
                );
            }
        }
        for &id in &section.deleted {
            assert!(deleted.insert(id), "object {id} deleted twice");
        }
    }
}

/// A split that carries calls past the first entry keeps the first entry's
/// deletions only when no later call names them; the rest move to the end.
#[test]
fn split_entries_never_call_an_object_an_earlier_entry_deleted() {
    let record = |id: u16| replication::Record {
        id,
        initial: Some(replication::Initial::Actor(replication::ActorInit {
            participant: 7,
            flag_a: true,
            flag_b: true,
            word_a: 4,
            word_b: 65535,
        })),
        update: replication::Update::Actor(replication::ActorUpdate::default()),
    };
    let notify = |i: u16| {
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: Endpoint {
                scene: 10,
                selector: i,
                serial: Serial::new(1).unwrap(),
            },
            participant: 179,
            call: nfs_world::participants::Call::Enter,
        })
    };
    let fire = |ghost: u16| {
        HostRpc::Event(nfs_world::logic::Fire {
            event: 2,
            player: None,
            target: nfs_world::logic::EntityRef { ghost, entity: 1 },
        })
    };
    // Presentation pushed three fires on ghost 184 past fifteen calls; the
    // section deletes 184 and 202.
    let mut section = returning::empty_section();
    section.setup = Some(Setup::RawEscape([0; 3]));
    section.records = vec![record(181), record(177)];
    section.deleted = vec![202, 184];
    let mut calls = (0..15u16).map(notify).collect::<Vec<_>>();
    calls.extend([fire(184), fire(184), fire(184)]);
    let entries = crate::net::players::admitted_entries(section, calls.clone()).unwrap();
    assert_no_call_after_its_deletion(&entries);
    assert_eq!(
        entries[0].0.deleted,
        [202],
        "unreferenced deletions keep their place"
    );
    assert_eq!(
        entries
            .iter()
            .flat_map(|(s, _)| s.deleted.iter().copied())
            .collect::<Vec<_>>(),
        [202, 184]
    );
    assert_eq!(
        entries
            .iter()
            .flat_map(|(_, c)| c.iter().copied())
            .collect::<Vec<_>>(),
        calls
    );
    for (section, chunk) in &entries {
        assert!(crate::net::players::entry_admitted(section, chunk));
    }
    // The garage completion split: a call carried to a later entry names a
    // deleted object.
    let records = (1..=300u16).map(record).collect::<Vec<_>>();
    let mut calls = (0..20u16).map(notify).collect::<Vec<_>>();
    calls.push(fire(900));
    let entries = PlayerListener::garage_entries(records, vec![900, 901], calls).unwrap();
    assert!(entries.len() > 1);
    assert_no_call_after_its_deletion(&entries);
    assert_eq!(entries[0].0.deleted, [901]);
}

/// The intro garage entry's presence goes after entering `begin_garage` and
/// before the mode's reactions, as in the official frame (E1193), so the
/// fifteen-message walk frame matches and presentation spills after it.
#[test]
fn intro_garage_presence_precedes_the_mode_reactions() {
    let endpoint = |scene: u16, selector: u16| Endpoint {
        scene,
        selector,
        serial: Serial::new(1).unwrap(),
    };
    let state = |selector: u16, call| {
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: endpoint(10, selector),
            participant: 179,
            call,
        })
    };
    let mode = |selector: u16, method: u32| {
        HostRpc::GameMode(nfs_world::game_mode::Call {
            target: 194,
            selector,
            serial: Serial::new(1).unwrap(),
            method,
            reference: None,
            arguments: nfs_world::game_mode::Arguments::None,
        })
    };
    use nfs_world::participants::Call::{Enter, Leave};
    let presence = nfs_world::garage::presence::Notification {
        endpoint: endpoint(17, 23),
        enabled: true,
    };
    let mut calls = vec![
        state(60, Leave),
        state(94, Enter),
        mode(1, 1),
        mode(0, 7),
        state(94, Leave),
        state(89, Enter),
    ];
    garage::insert_presence(&mut calls, presence);
    assert_eq!(calls[2], HostRpc::GaragePresence(presence));
    assert_eq!(calls[1], state(94, Enter));
    assert_eq!(calls[3], mode(1, 1));
    assert_eq!(calls.len(), 7);
    let mut without_mode = vec![state(94, Enter)];
    garage::insert_presence(&mut without_mode, presence);
    assert_eq!(
        without_mode.last(),
        Some(&HostRpc::GaragePresence(presence))
    );
}

/// An entry the session does not admit stops the world loudly instead of
/// queueing: test builds fail at its producer, and the flush reports which
/// queue refused it and keeps the entry (E1186, E1188).
#[test]
fn unadmitted_entries_fail_at_the_producer_and_stop_the_flush() {
    let record = |id: u16| replication::Record {
        id,
        initial: Some(replication::Initial::Actor(replication::ActorInit {
            participant: 7,
            flag_a: true,
            flag_b: true,
            word_a: 4,
            word_b: 65535,
        })),
        update: replication::Update::Actor(replication::ActorUpdate::default()),
    };
    let mut oversized = returning::empty_section();
    oversized.setup = Some(Setup::RawEscape([0; 3]));
    oversized.records = (1..=600u16).map(record).collect();
    assert!(!crate::net::players::entry_admitted(&oversized, &[]));
    let mut listener = PlayerListener::new(0x100000002);
    let entry = oversized.clone();
    let producer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        listener.queue_entries([(entry, Vec::new())]);
    }))
    .unwrap_err();
    let message = producer.downcast_ref::<String>().expect("panic message");
    assert!(
        message.contains("prologue") && message.contains("tests.rs"),
        "{message}"
    );
    assert!(message.contains("not admitted"), "{message}");
    // A release build queues it with an error log; the flush then refuses it.
    let mut listener = PlayerListener::new(0x100000002);
    listener.pending_entries.push_back((oversized, Vec::new()));
    let mut host = crate::net::players::test_wire::session_host();
    let (queue, error, entry) =
        crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap_err();
    assert_eq!(queue, "entry transition");
    assert_eq!(entry.map(|entry| entry.0.records.len()), Some(600));
    assert!(matches!(
        error,
        Some(nfs_world::session::Error::Application(
            nfs_world::application::Error::Bound
        ))
    ));
    assert_eq!(listener.pending_entries.len(), 1);
    assert_eq!(host.replication_progress(), (0, 0));
}
