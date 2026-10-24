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
fn setup() -> (PlayerListener, u16, Arc<nfs_world::items::Collection>) {
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
    listener.population = Some(population);
    listener.participants = participants;
    (listener, participant, items)
}

#[test]
fn opening_runs_after_car_and_render_readiness_and_waits_for_native_completion() {
    let (mut listener, participant, items) = setup();
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
    assert!(send(&mut listener, 0, 82, glass.span()));
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
