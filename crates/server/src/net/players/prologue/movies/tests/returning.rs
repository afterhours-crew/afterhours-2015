// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn request_no_args(endpoint: Endpoint, participant: u16) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(0, 32)
        .put(crate::net::players::test_wire::client_padding(), 7);
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

#[test]
fn final_garage_stages_the_owned_car_and_waits_for_native_entry_without_selecting_again() {
    crate::net::players::test_wire::with_native_padding(|| {
        final_garage_loaded();
    });
}
/// The return drive after its arrive sequence: the car drives, no
/// destination crossing yet.
fn return_drive_driving() -> (PlayerListener, u16, u16) {
    let (mut listener, participant, car) = return_drive_started();
    let content = listener.prologue.clone().unwrap();
    let mode = content.return_drive.as_ref().unwrap();
    let inventory = listener.inventory.clone().unwrap();
    let item = inventory.slots.values()[0].unwrap();
    let definition = inventory.items.items[&item].definition;
    let car_id = crate::event_logic::car_id(
        &listener
            .vehicles
            .as_ref()
            .unwrap()
            .vehicles
            .definition(&definition)
            .unwrap()
            .blueprint,
    );
    let active = &mut listener.intro_modes.get_mut(&participant).unwrap().0;
    let sequence = listener
        .players
        .objects()
        .iter()
        .find_map(|(id, _)| active.sequence_owner(id, 0).then_some(id))
        .unwrap();
    active
        .signal(
            mode,
            &listener.players,
            &nfs_world::logic::Message::Reached {
                event: 43,
                target: nfs_world::logic::EntityRef {
                    ghost: sequence,
                    entity: 1,
                },
                player: 0,
            },
        )
        .unwrap()
        .unwrap();
    active
        .completed(
            mode,
            &mut listener.players,
            Completed {
                sequence,
                participant,
            },
            car_id,
        )
        .unwrap();
    (listener, participant, car)
}
fn final_garage_loaded() -> (PlayerListener, u16) {
    let (mut listener, participant, car) = return_drive_driving();
    let content = listener.prologue.clone().unwrap();
    let mode = content.return_drive.as_ref().unwrap();
    let inventory = listener.inventory.clone().unwrap();
    let item = inventory.slots.values()[0].unwrap();
    let selected_sequence = listener.selection.ghost(participant);
    let &(key, serializer) = &mode.data.game_mode.checkpoints[0];
    let (scene, selector) = listener
        .players
        .objects()
        .scene_endpoint(key, serializer)
        .unwrap();
    let seconds = listener.event_seconds(listener.world_ms).unwrap() + 1.;
    let active = &mut listener.intro_modes.get_mut(&participant).unwrap().0;
    active
        .crossing(
            mode,
            &mut listener.players,
            nfs_world::game_mode::Crossing {
                scene,
                selector,
                vehicle: car,
                seconds,
                inside: true,
            },
            seconds,
        )
        .unwrap();
    active
        .advance(mode, &mut listener.players, seconds)
        .unwrap();
    assert!(active.destination_reached());
    listener.world_ms += 1000;
    let endpoint = listener.participants.return_bindings().unwrap().request;
    let enter = request_no_args(endpoint, participant);
    listener
        .returns
        .as_mut()
        .unwrap()
        .poll_capacity(listener.world_ms, 0)
        .unwrap();
    assert!(listener.story_pending.is_empty());
    assert!(send(&mut listener, 38, 47, enter.span()));
    assert!(
        listener.story_pending.is_empty(),
        "the first-drive reward must not replay"
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
    drain(&mut listener);
    let before = listener.players.objects().snapshot();
    listener.pending.resize(
        MAX_PENDING,
        super::super::super::super::returning::empty_section(),
    );
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), before);
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    let owner = Owner {
        connection: HOST_SELECTOR as u8,
        persona: listener.persona,
        participant,
    };
    let staged = listener
        .players
        .staged_vehicle(owner.connection, owner.persona, participant, item)
        .unwrap();
    assert_ne!(staged, car);
    let loaded = listener
        .pending_rpcs
        .iter()
        .find_map(|r| match r {
            HostRpc::Vehicle(b) => Some(b.expected().encode().unwrap()),
            _ => None,
        })
        .unwrap();
    assert!(listener.players.objects().get(car).is_some());
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|r| matches!(r, HostRpc::Vehicle(_)))
            .count(),
        1
    );
    let staged_world = listener.players.objects().snapshot();
    drain(&mut listener);
    assert!(send(&mut listener, 39, 47, enter.span()));
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), staged_world);
    assert!(listener.pending_rpcs.is_empty());
    assert!(send(&mut listener, 40, 47, loaded.span()));
    let complete = native_completion(Completed {
        sequence: entry,
        participant,
    });
    assert!(!send(&mut listener, 41, 47, complete.span()));
    assert!(listener.players.objects().get(car).is_some());
    listener.world_ms += 7500;
    assert_eq!(
        listener
            .returns
            .as_mut()
            .unwrap()
            .poll_capacity(listener.world_ms, 1)
            .unwrap()
            .len(),
        1
    );
    listener.pending_entries.resize(
        MAX_PENDING,
        (
            super::super::super::super::returning::empty_section(),
            vec![],
        ),
    );
    assert!(!send(&mut listener, 41, 47, complete.span()));
    assert_eq!(listener.players.objects().snapshot(), staged_world);
    drain(&mut listener);
    assert!(send(&mut listener, 41, 47, complete.span()));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    assert_eq!(
        listener
            .players
            .owned_vehicle(owner.connection, owner.persona, participant, item),
        Some(staged)
    );
    assert!(listener.players.objects().get(car).is_none());
    assert!(!listener.world_cars.contains_key(&car));
    assert_eq!(listener.inventory.as_ref().unwrap().slots, inventory.slots);
    assert_eq!(listener.inventory.as_ref().unwrap().items, inventory.items);
    assert!(listener.story_pending.is_empty());
    drain(&mut listener);
    let after = listener.players.objects().snapshot();
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), after);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    assert_eq!(listener.selection.ghost(participant), selected_sequence);
    (listener, participant)
}

#[test]
fn final_composite_is_owned_and_completion_survives_frame_ordering_and_queue_pressure() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant) = final_garage_loaded();
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona: listener.persona,
            participant,
        };
        let parent_asset = listener
            .progression
            .as_ref()
            .unwrap()
            .construction
            .blueprint(crate::progression::Branch::Intro)
            .unwrap()
            .asset();
        let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
        let definition = nfs_content::prologue::composite::Composite {
            objective: 20,
            sequence: nfs_content::prologue::movies::Sequence {
                blueprint: "test/final".into(),
                reference: nfs_content::scoring::events::Identity {
                    asset: "test/final".into(),
                    index: 1,
                    instances: vec![],
                },
                state: (909, 89),
            },
            completion: nfs_world::scoring::event_sync::Binding {
                entity: 9,
                event: 71,
                node: 0,
            },
        };
        content.final_composite = Some(crate::prologue::composite::Content::for_test(
            definition,
            content.opening.clone(),
            parent_asset,
        ));
        listener
            .intro_modes
            .get_mut(&participant)
            .unwrap()
            .0
            .leave(
                content.return_drive.as_ref().unwrap(),
                &mut listener.players,
                participant,
            )
            .unwrap();
        let waiting = listener
            .sequences
            .as_ref()
            .unwrap()
            .ghost(participant)
            .unwrap();
        listener.world_ms += 300_000;
        assert_eq!(
            listener
                .sequences
                .as_mut()
                .unwrap()
                .poll(listener.world_ms, 1)
                .unwrap()
                .len(),
            1
        );
        assert!(
            listener
                .sequences
                .as_mut()
                .unwrap()
                .complete(
                    &listener.players,
                    owner,
                    Completed {
                        sequence: waiting,
                        participant
                    }
                )
                .unwrap()
        );
        let before = listener.players.objects().snapshot();
        listener.pending.resize(
            MAX_PENDING,
            super::super::super::super::returning::empty_section(),
        );
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.players.objects().snapshot(), before);
        assert!(listener.intro_composites.is_empty());
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroComposite)
        );
        let sequence = listener.intro_composites[&participant].ghost();
        assert!(listener.players.objects().get(waiting).is_none());
        let parent=listener.players.objects().iter().find_map(|(id,o)| match &o.initial {
        replication::Initial::Entity { prefix,fields } if prefix.asset==parent_asset && matches!(fields.first(),Some(nfs_world::replication::entity::Initial::Root { reference,.. }) if *reference==participant)=>Some(id),_=>None,
    }).unwrap();
        let signal = |ghost, entity, event| {
            nfs_world::logic::Message::Reached {
                event,
                target: nfs_world::logic::EntityRef { ghost, entity },
                player: 0,
            }
            .encode()
            .unwrap()
        };
        drain(&mut listener);
        // The parent signal may be in a frame before the native completion RPC.
        let final_signal = signal(parent, 9, 71);
        assert!(send(&mut listener, 42, 82, final_signal.span()));
        tick_with_normal_presentation(&mut listener).unwrap();
        assert!(listener.story_pending.is_empty());
        let ack = native_completion(Completed {
            sequence,
            participant,
        });
        assert!(!send(&mut listener, 43, 47, ack.span()));
        let stop = signal(sequence, 1, 37);
        assert!(send(&mut listener, 43, 82, stop.span()));
        assert!(
            listener
                .pending_rpcs
                .iter()
                .any(|r| matches!(r,HostRpc::SequenceStop(s) if s.sequence==sequence))
        );
        drain(&mut listener);
        listener.pending.resize(
            MAX_PENDING,
            super::super::super::super::returning::empty_section(),
        );
        assert!(!send(&mut listener, 44, 47, ack.span()));
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroComposite)
        );
        assert!(listener.players.objects().get(sequence).is_some());
        drain(&mut listener);
        assert!(send(&mut listener, 44, 47, ack.span()));
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::Customization)
        );
        assert!(listener.intro_composites[&participant].finished());
        assert!(listener.players.objects().get(sequence).is_none());
        assert_eq!(
            listener
                .pending_rpcs
                .iter()
                .filter_map(|r| match r {
                    HostRpc::Participant(n) => Some((n.endpoint.selector, n.call)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                (88, nfs_world::participants::Call::Leave),
                (85, nfs_world::participants::Call::Enter),
                (85, nfs_world::participants::Call::Leave),
                (86, nfs_world::participants::Call::Enter)
            ]
        );
        drain(&mut listener);
        let job = crate::net::items::story::Request {
            participant,
            player_index: 1,
            progression: listener.progression.clone().unwrap(),
            action: crate::net::items::story::Action::Complete { objective: 20 },
        };
        listener.story_pending.resize(MAX_PENDING, job);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.story_pending.len(), MAX_PENDING);
        listener.story_pending.clear();
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.story_pending.len(), 1);
        assert!(send(&mut listener, 45, 47, ack.span()));
        assert!(send(&mut listener, 46, 82, final_signal.span()));
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.story_pending.len(), 1);
        let job = listener.take_story().unwrap();
        assert_eq!(
            job.action,
            crate::net::items::story::Action::Complete { objective: 20 }
        );
        use nfs_storage::InventoryRepository;
        let (repo, account, mut saved) = crate::progression::tests::local();
        let policy = job
            .progression
            .catalog
            .story_policy(job.progression.settings);
        for objective in [10, 20] {
            let nfs_services::story::Prepared::Mutation(mut batch) = policy
                .complete(
                    &saved,
                    saved.generation,
                    policy.next_operation(&saved).unwrap(),
                    objective,
                    vec![],
                )
                .unwrap()
            else {
                panic!()
            };
            if objective == 10 {
                let inventory = listener.inventory.as_ref().unwrap();
                let item = inventory.slots.values()[0].unwrap();
                let definition = inventory.items.items[&item].definition;
                let item = nfs_storage::ItemId::new(item).unwrap();
                batch.ops.extend([
                    nfs_storage::Op::Insert(nfs_storage::ItemRecord {
                        id: item,
                        definition: nfs_storage::DefinitionGuid(definition),
                        owner: None,
                        state: nfs_world::items::OWNED,
                        buy_price: 0,
                        sell_price: 0,
                        default_items: vec![],
                        sub_items: vec![],
                        derived: vec![],
                    }),
                    nfs_storage::Op::SetGarage(nfs_storage::GarageSlots([
                        Some(item),
                        None,
                        None,
                        None,
                        None,
                    ])),
                ]);
            }
            repo.apply(account, &batch, nfs_storage::Timestamp(5))
                .unwrap();
            saved = repo.snapshot(account).unwrap();
            if objective == 10 {
                // Model an active final objective with the already selected car.
                // The reused garage fixture completed this synthetic objective
                // earlier for selection, unlike the installed separate objectives.
                listener.persistent = Some(Arc::new(
                    crate::progression::tests::tables().view(&saved).unwrap(),
                ));
            }
        }
        let committed = crate::net::items::story::Committed {
            loaded: Arc::new(crate::progression::tests::tables().view(&saved).unwrap()),
            items: listener.inventory.as_ref().unwrap().items.clone(),
            notifications: vec![],
            reward: None,
        };
        let canceller = listener.intro_modes[&participant].1.ghost;
        crate::prologue::driving::tests::configure_shutdown(
            &mut Arc::make_mut(listener.prologue.as_mut().unwrap())
                .driving
                .as_mut()
                .unwrap()
                .1,
            &mut listener.intro_modes.get_mut(&participant).unwrap().1,
            20,
        );
        Arc::make_mut(listener.prologue.as_mut().unwrap())
            .driving
            .as_mut()
            .unwrap()
            .1
            .data
            .shutdown_row = Some(nfs_content::events::intro::LaunchEvent {
            bus: 11,
            ordinal: 0,
        });
        let leave = request_no_args(
            Endpoint {
                scene: canceller,
                selector: 0,
                serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
            },
            participant,
        );
        assert!(!send(&mut listener, 47, 47, leave.span()));
        let placeholder = HostRpc::Participant(nfs_world::participants::Notification {
            endpoint: Endpoint {
                scene: 1,
                selector: 1,
                serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
            },
            participant,
            call: nfs_world::participants::Call::Enter,
        });
        listener
            .pending_rpcs
            .resize(nfs_world::session::MAX_RPC_QUEUE, placeholder);
        let before = listener.players.objects().snapshot();
        assert_eq!(
            listener.story_committed(&job, committed.clone()),
            Err(replication::Error::Bound)
        );
        assert_eq!(listener.players.objects().snapshot(), before);
        drain(&mut listener);
        listener.story_committed(&job, committed.clone()).unwrap();
        assert!(
            listener
                .pending_rpcs
                .iter()
                .any(|r| matches!(r,HostRpc::GameMode(c) if c.target==canceller && c.method==1))
        );
        // The shutdown comparison's networked row follows the mode's calls.
        let row = listener
            .pending_rpcs
            .iter()
            .position(|r| matches!(r, HostRpc::Event(f) if f.target.entity == 11 && f.event == 0));
        let last_mode_call = listener
            .pending_rpcs
            .iter()
            .rposition(|r| matches!(r,HostRpc::GameMode(c) if c.target==canceller));
        assert!(row.is_some() && row > last_mode_call);
        drain(&mut listener);
        listener.story_committed(&job, committed).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.intro_chapter(participant),
            Some(Chapter::ReturnDrive)
        );
        listener.pending.resize(
            MAX_PENDING,
            super::super::super::super::returning::empty_section(),
        );
        assert!(!send(&mut listener, 47, 47, leave.span()));
        assert!(!listener.intro_modes[&participant].1.has_left());
        drain(&mut listener);
        assert!(send(&mut listener, 47, 47, leave.span()));
        assert!(listener.intro_modes[&participant].1.has_left());
        assert!(send(&mut listener, 48, 47, leave.span()));
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.participants.intro_chapter(participant), None);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::Customization)
        );
        assert!(
            job.progression
                .catalog
                .restore(
                    listener.persistent.as_deref().unwrap(),
                    job.progression.settings
                )
                .unwrap()
                .branch(crate::progression::Branch::Intro)
                .objectives()[&20]
                .completed
        );
    });
}

#[test]
fn a_garage_request_before_the_destination_crossing_begins_the_final_return() {
    // E1248: the client's RaceBoxTrigger request (automatic on box entry)
    // reached the host before the route's destination crossing; the host
    // answered nothing, the prompt closed and the return never began.
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant, _) = return_drive_driving();
        assert!(!listener.intro_modes[&participant].0.destination_reached());
        listener.world_ms += 1000;
        let endpoint = listener.participants.return_bindings().unwrap().request;
        listener
            .returns
            .as_mut()
            .unwrap()
            .poll_capacity(listener.world_ms, 0)
            .unwrap();
        let accepted = send(
            &mut listener,
            38,
            47,
            request_no_args(endpoint, participant).span(),
        );
        assert!(accepted, "refused: {:?}", listener.last_error);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroEnteringGarage)
        );
        assert!(
            listener
                .returns
                .as_ref()
                .unwrap()
                .ghost(participant)
                .is_some()
        );
    });
}
