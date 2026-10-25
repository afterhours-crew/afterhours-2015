// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn endpoint(selector: u16) -> Endpoint {
    Endpoint {
        scene: 4,
        selector,
        serial: Serial::new(12).unwrap(),
    }
}
fn bindings() -> Bindings {
    Bindings {
        parameters: endpoint(3),
        spawning: endpoint(2),
        teleport: endpoint(76),
        check_player: endpoint(82),
        render_wait: endpoint(77),
        render_ready: endpoint(80),
        opening: endpoint(78),
        alive: endpoint(1),
        pending_items: endpoint(93),
        relocating: endpoint(84),
    }
}
fn ready() -> Lifecycle {
    let mut model = super::super::tests::ready();
    model.intro = Some(bindings());
    model.stages.insert(8, Stage::Progressing);
    model.stages.insert(9, Stage::Progressing);
    model
        .set_garage([Some(123), None, None, None, None])
        .unwrap();
    model
}
fn actor() -> crate::actors::Binding {
    crate::actors::Binding {
        endpoint: Endpoint {
            scene: 2,
            selector: 22,
            serial: Serial::new(8).unwrap(),
        },
        participant: 8,
        actor: 19,
    }
}

#[test]
fn movie_suspends_only_its_story_participant_until_owned_completion() {
    let mut model = ready();
    assert_eq!(model.intro_movie_begin(8), Outcome::Unsupported);
    model.intro_started.insert(8, Chapter::FirstRace);
    model.stages.insert(8, Stage::FreeRoam);
    assert_eq!(calls(model.intro_movie_begin(8)), vec![(1, Call::Leave)]);
    assert_eq!(model.intro_movie_begin(8), Outcome::Repeated);
    assert_eq!(model.intro_movie_complete(9), Outcome::Unsupported);
    assert_eq!(model.stage(8), Some(Stage::IntroMovie));
    assert_eq!(model.stage(9), Some(Stage::Progressing));
    assert_eq!(calls(model.intro_movie_complete(8)), vec![(1, Call::Enter)]);
    assert_eq!(model.intro_movie_complete(8), Outcome::Repeated);
}
#[test]
fn second_race_uses_in_world_relocation_and_native_readiness_without_respawn() {
    let mut model = ready();
    assert_eq!(model.intro_second_race_begin(8), Outcome::Unsupported);
    model.intro_started.insert(8, Chapter::FirstRace);
    model.stages.insert(8, Stage::IntroMovie);
    assert_eq!(model.intro_second_race_begin(8), Outcome::Unsupported);
    model.intro_movie_complete(8);
    assert_eq!(
        calls(model.intro_second_race_begin(8)),
        vec![(1, Call::Leave), (84, Call::Enter)]
    );
    assert_eq!(model.intro_second_race_begin(8), Outcome::Repeated);
    assert_eq!(model.intro_chapter(8), Some(Chapter::SecondRace));
    assert_eq!(model.intro_relocation_ready(9), Outcome::Unsupported);
    assert_eq!(
        calls(model.intro_relocation_ready(8)),
        vec![(84, Call::Leave), (76, Call::Enter)]
    );
    assert_eq!(model.intro_relocation_ready(8), Outcome::Repeated);
    assert!(
        model
            .receive_intro(request(8, 80, 0, 0).span(), |_| true)
            .is_err()
    );
    model
        .receive_intro(request(8, 76, 3, 0).span(), |_| true)
        .unwrap();
    model
        .receive_intro(request(8, 80, 0, 0).span(), |_| true)
        .unwrap();
    assert_eq!(model.stage(8), Some(Stage::IntroOpening));
    model.intro_opening_complete(8);
    assert_eq!(calls(model.intro_drive_ready(8)), vec![(1, Call::Enter)]);
    assert_eq!(model.stage(9), Some(Stage::Progressing));
}
fn request(participant: u16, selector: u16, method: u32, tail: u8) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(selector.into(), 9)
        .put(method.into(), 32)
        .put(tail.into(), 7);
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(4, 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
fn calls(out: Outcome) -> Vec<(u16, Call)> {
    let Outcome::Advanced(chain) = out else {
        panic!("expected one committed transition");
    };
    chain
        .into_iter()
        .map(|n| {
            n.encode().unwrap();
            assert_eq!(n.participant, 8);
            (n.endpoint.selector, n.call)
        })
        .collect()
}

#[test]
fn completed_selection_enters_the_owned_car_spawn_once_without_restarting_the_opening_drive() {
    let mut model = ready();
    assert_eq!(model.intro_selection_complete(8), Outcome::Unsupported);
    model.enter_intro_with_actor(actor()).unwrap();
    assert_eq!(model.intro_selection_complete(8), Outcome::Unsupported);
    model.stages.insert(8, Stage::IntroCarSelection);
    assert_eq!(
        calls(model.intro_selection_complete(8)),
        vec![
            (88, Call::Leave),
            (85, Call::Enter),
            (85, Call::Leave),
            (93, Call::Enter),
            (93, Call::Leave),
            (3, Call::Enter),
            (3, Call::LeaveTagged(31)),
            (2, Call::Enter),
        ]
    );
    assert_eq!(model.intro_chapter(8), Some(Chapter::FirstRace));
    assert_eq!(model.stage(8), Some(Stage::IntroSpawning));
    assert_eq!(model.intro_selection_complete(8), Outcome::Repeated);
    assert_eq!(model.intro_chapter(9), None);
    assert_eq!(model.intro_selection_complete(9), Outcome::Unsupported);
    assert_eq!(
        calls(model.intro_vehicle_ready(8)),
        vec![(2, Call::Leave), (76, Call::Enter)]
    );
    assert_eq!(model.intro_chapter(8), Some(Chapter::FirstRace));
}

#[test]
fn actor_and_empty_runtime_item_precede_spawning_and_leave_the_owned_garage_intact() {
    let mut model = ready();
    let before = model.clone();
    let mut bad = actor();
    bad.actor = 0;
    assert!(model.enter_intro_with_actor(bad).is_err());
    assert_eq!(model, before);
    let mut bad = actor();
    bad.endpoint.scene = 77;
    assert!(model.enter_intro_with_actor(bad).is_err());
    assert_eq!(model, before);
    let out = model.enter_intro_with_actor(actor()).unwrap();
    assert_eq!(model.garage, before.garage);
    assert_eq!(model.stage(9), Some(Stage::Progressing));
    assert_eq!(model.stage(8), Some(Stage::IntroSpawning));
    let start = out
        .iter()
        .take(6)
        .map(|rpc| match rpc {
            HostRpc::Participant(n) => (n.endpoint.selector, n.call),
            _ => panic!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        start,
        [
            (24, Call::Leave),
            (20, Call::Enter),
            (20, Call::Leave),
            (17, Call::Enter),
            (17, Call::Leave),
            (18, Call::Enter),
        ]
    );
    assert!(matches!(
        out[6],
        HostRpc::Garage(crate::garage::Binding {
            participant: 8,
            item: 0,
            ..
        })
    ));
    assert!(matches!(
        out[7],
        HostRpc::Participant(Notification {
            call: Call::Leave,
            ..
        })
    ));
    assert!(matches!(
        out[8],
        HostRpc::Participant(Notification {
            call: Call::Enter,
            ..
        })
    ));
    assert_eq!(out[9], HostRpc::Actor(actor()));
    assert!(matches!(
        out[10],
        HostRpc::Participant(Notification {
            call: Call::LeaveTagged(31),
            ..
        })
    ));
    assert!(matches!(
        out[11],
        HostRpc::Participant(Notification {
            call: Call::Enter,
            ..
        })
    ));
    for rpc in out {
        rpc.encode().unwrap();
    }
    assert!(model.enter_intro_with_actor(actor()).is_err());
    assert_eq!(model.intro_drive_ready(8), Outcome::Unsupported);
    assert_eq!(model.intro_opening_complete(8), Outcome::Unsupported);
    assert_eq!(
        calls(model.intro_vehicle_ready(8)),
        [(2, Call::Leave), (76, Call::Enter)]
    );
    assert_eq!(model.intro_vehicle_ready(8), Outcome::Repeated);
}

#[test]
fn owned_acknowledgements_gate_render_opening_and_first_mode_without_duplicate_output() {
    let mut model = ready();
    model.enter_intro_with_actor(actor()).unwrap();
    let teleport = request(8, 76, 3, 0);
    let rendering = request(8, 80, 0, 0);
    let before = model.clone();
    assert!(model.receive(teleport.span(), |p| p == 8).is_err());
    assert_eq!(model, before);
    model.intro_vehicle_ready(8);
    let before = model.clone();
    assert!(model.receive(rendering.span(), |p| p == 8).is_err());
    assert_eq!(model, before);
    assert!(model.receive(teleport.span(), |p| p == 9).is_err());
    assert_eq!(model, before);
    for tail in [1, 127] {
        assert!(
            model
                .receive(request(8, 76, 3, tail).span(), |_| true)
                .is_err()
        );
        assert_eq!(model, before);
    }
    assert_eq!(
        calls(model.receive(teleport.span(), |p| p == 8).unwrap()),
        [
            (76, Call::Leave),
            (82, Call::Enter),
            (82, Call::Leave),
            (77, Call::Enter)
        ]
    );
    assert_eq!(model.stage(8), Some(Stage::IntroRendering));
    assert!(model.in_free_roam().is_empty());
    assert_eq!(
        model.receive(teleport.span(), |p| p == 8).unwrap(),
        Outcome::Repeated
    );
    assert_eq!(
        calls(model.receive(rendering.span(), |p| p == 8).unwrap()),
        [(77, Call::Leave), (78, Call::Enter)]
    );
    assert_eq!(
        model.receive(rendering.span(), |p| p == 8).unwrap(),
        Outcome::Repeated
    );
    assert!(model.in_free_roam().is_empty());
    assert_eq!(model.intro_drive_ready(8), Outcome::Unsupported);
    assert_eq!(calls(model.intro_opening_complete(8)), [(78, Call::Leave)]);
    assert_eq!(model.intro_opening_complete(8), Outcome::Repeated);
    assert!(model.in_free_roam().is_empty());
    assert_eq!(calls(model.intro_drive_ready(8)), [(1, Call::Enter)]);
    assert_eq!(model.in_free_roam(), [8]);
    assert_eq!(model.stage(9), Some(Stage::Progressing));
    assert_eq!(model.intro_drive_ready(8), Outcome::Repeated);
    assert_eq!(
        model.receive(teleport.span(), |p| p == 8).unwrap(),
        Outcome::Repeated
    );
    assert_eq!(
        model.receive(rendering.span(), |p| p == 8).unwrap(),
        Outcome::Repeated
    );
    assert!(
        model
            .receive(request(10, 76, 3, 0).span(), |_| true)
            .is_err()
    );
}

#[test]
fn early_intro_router_does_not_consume_another_participants_shared_teleport() {
    let mut model = ready();
    model.enter_intro_with_actor(actor()).unwrap();
    model.intro_vehicle_ready(8);
    model.stages.insert(9, Stage::FreeRoam);
    assert_eq!(
        model
            .receive_intro(request(9, 76, 3, 0).span(), |_| true)
            .unwrap(),
        None
    );
    let before = model.clone();
    assert!(
        model
            .receive_intro(request(8, 76, 3, 0).span(), |_| false)
            .is_err()
    );
    assert_eq!(model, before);
    assert!(matches!(
        model
            .receive_intro(request(8, 76, 3, 0).span(), |_| true)
            .unwrap(),
        Some(Outcome::Advanced(_))
    ));
    assert_eq!(model.stage(8), Some(Stage::IntroRendering));
    assert_eq!(model.stage(9), Some(Stage::FreeRoam));
}

fn scene() -> Record {
    let mut fields: Vec<_> = (0..100)
        .map(|index| sublevel::Initial::RpcReferences {
            rpc: sublevel::Rpc {
                selector: index * 2,
                serial: Serial::new(index).unwrap(),
            },
            value: vec![],
        })
        .collect();
    fields[0] = sublevel::Initial::Empty;
    let mut kinds = [sublevel::Kind::RpcReferences; 100];
    kinds[0] = sublevel::Kind::Noop;
    Record {
        id: 7,
        initial: Some(Initial::SubLevel {
            prefix: sublevel::Prefix {
                level_id: 0,
                content_key: 222,
                blueprint: None,
                word_a: 0,
                word_b: 0,
                constructor_word: None,
            },
            fields,
        }),
        update: crate::replication::Update::SubLevel {
            profile: sublevel::Profile::new(false, &kinds).unwrap(),
            fields: vec![Some(sublevel::Update::Noop); 100],
        },
    }
}
#[test]
fn binding_uses_live_scene_identity_and_rejects_missing_wrong_and_duplicate_fields() {
    let record = scene();
    let bound = Bindings::from_records(std::slice::from_ref(&record), 222)
        .unwrap()
        .unwrap();
    assert_eq!(
        bound.teleport,
        Endpoint {
            scene: 7,
            selector: 154,
            serial: Serial::new(77).unwrap()
        }
    );
    assert_eq!(bound.parameters.selector, 8);
    assert_eq!(bound.opening.selector, 158);
    assert_eq!(
        Bindings::from_records(std::slice::from_ref(&record), 111),
        Ok(None)
    );
    assert_eq!(
        Bindings::from_records(&[record.clone(), record.clone()], 222),
        Err(Error::DuplicateObject)
    );
    let mut malformed = record.clone();
    let Some(Initial::SubLevel { fields, .. }) = &mut malformed.initial else {
        panic!()
    };
    fields[77] = sublevel::Initial::Empty;
    assert_eq!(
        Bindings::from_records(&[malformed.clone()], 222),
        Err(Error::TypeMismatch)
    );
    let mut model = Lifecycle::default();
    assert!(model.bind_intro(&[malformed], 222).is_err());
    assert!(model.intro_bindings().is_none());
    assert!(
        model
            .bind_intro(std::slice::from_ref(&record), 222)
            .unwrap()
    );
    assert_eq!(
        model.bind_intro(&[record], 222),
        Err(Error::DuplicateObject)
    );
    assert_eq!(model.intro_bindings(), Some(bound));
}

#[test]
fn ordinary_garage_progression_still_uses_its_own_route_when_intro_is_bound() {
    let mut model = ready();
    let garage = model.enter_garage_loading(8);
    assert_eq!(model.stage(8), Some(Stage::LoadingGarage));
    assert!(
        garage
            .iter()
            .any(|n| n.endpoint == model.bindings.unwrap().enter_garage_from_boot)
    );
    assert_eq!(model.intro_vehicle_ready(8), Outcome::Unsupported);
    let before = model.clone();
    assert_eq!(
        model
            .receive(request(8, 76, 3, 0).span(), |_| true)
            .unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(model, before);
}

#[test]
fn first_drive_returns_automatically_and_waits_in_the_selection_composite() {
    let mut model = ready();
    model.returning = Some(ReturnBindings {
        request: endpoint(100),
        free_roam: endpoint(1),
        preparing: [61, 63, 65, 73].map(endpoint),
        sequence: endpoint(67),
        finishing: [68, 70, 60].map(endpoint),
    });
    model.stages.insert(8, Stage::FreeRoam);
    assert_eq!(model.begin_intro_return(8), Outcome::Unsupported);
    model.intro_started.insert(8, Chapter::FirstDrive);
    let Outcome::Advanced(chain) = model.begin_intro_return(8) else {
        panic!()
    };
    assert_eq!(
        chain
            .iter()
            .map(|n| (n.endpoint.selector, n.call))
            .collect::<Vec<_>>(),
        vec![
            (1, Call::Leave),
            (61, Call::Enter),
            (61, Call::Leave),
            (63, Call::Enter),
            (63, Call::LeaveTagged(31)),
            (65, Call::Enter),
            (65, Call::LeaveTagged(31)),
            (73, Call::Enter),
            (73, Call::Leave),
            (67, Call::Enter)
        ]
    );
    assert_eq!(model.stage(8), Some(Stage::IntroEnteringGarage));
    assert!(model.take_returning().is_empty());
    assert_eq!(model.begin_intro_return(8), Outcome::Repeated);
    assert_eq!(model.intro_garage_ready(8), Outcome::Unsupported);
    assert!(matches!(model.returned_to_garage(8), Outcome::Advanced(_)));
    assert_eq!(model.stage(8), Some(Stage::IntroLoadingGarage));
    assert!(model.waiting_garage().is_empty());
    assert_eq!(model.enter_customization(8), Outcome::Repeated);
    let Outcome::Advanced(chain) = model.intro_garage_ready(8) else {
        panic!()
    };
    let b = model.bindings.unwrap();
    assert_eq!(
        chain,
        notifications(
            8,
            &[(b.loading_garage, Call::Leave), (b.composite, Call::Enter)]
        )
    );
    assert_eq!(model.stage(8), Some(Stage::IntroCarSelection));
    assert_eq!(model.intro_garage_ready(8), Outcome::Repeated);
    assert_eq!(model.stage(9), Some(Stage::Progressing));
}
