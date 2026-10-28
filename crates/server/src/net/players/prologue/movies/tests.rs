// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{
    drain, native_completion, send, send_all, tick_with_normal_presentation,
};
use super::*;
use nfs_world::{bits::BitWriter, participants::Endpoint};
use std::sync::Arc;
mod returning;

fn request(endpoint: Endpoint, participant: u16, method: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(method.into(), 32)
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
fn fixture() -> (PlayerListener, u16, Endpoint) {
    let (mut listener, participant) = super::super::selection::tests::selected(true);
    let bindings = listener.participants.intro_bindings().unwrap();
    assert!(matches!(
        listener.participants.intro_teleport_begin(participant),
        Outcome::Advanced(_)
    ));
    // Startup acknowledgements carry opaque byte-alignment bits too.
    let startup = |endpoint: Endpoint, method: u32| {
        let mut p = BitWriter::new();
        p.put(endpoint.selector.into(), 9)
            .put(method.into(), 32)
            .put(crate::net::players::test_wire::client_padding(), 7);
        let mut wire = BitWriter::new();
        wire.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(endpoint.scene.into(), 13)
            .put(participant.into(), 13)
            .put(p.bytes().len() as u64, 9)
            .put_span(p.span());
        wire
    };
    listener
        .participants
        .receive_intro(startup(bindings.teleport, 3).span(), |_| true)
        .unwrap();
    listener
        .participants
        .receive_intro(startup(bindings.render_ready, 0).span(), |_| true)
        .unwrap();
    listener.participants.intro_opening_complete(participant);
    listener.participants.intro_drive_ready(participant);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    let sequence = |state| nfs_content::prologue::movies::Sequence {
        blueprint: "test/movie-sequence".into(),
        reference: nfs_content::scoring::events::Identity {
            asset: "test/movie".into(),
            index: 1,
            instances: vec![],
        },
        state: (909, state),
    };
    let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
    content.manu_movie = Some(crate::prologue::movies::Content::for_test(
        nfs_content::prologue::movies::Movie {
            objective: 10,
            blueprint: "test/movie".into(),
            video: nfs_world::fmv::Video {
                bundle: 30,
                index: 1,
                count: 3,
            },
            state: (909, 56),
            before: sequence(55),
            after: sequence(57),
        },
        content.opening.clone(),
        0.,
    ));
    let scene = listener.players.objects().scene(909).unwrap();
    let endpoint = Endpoint {
        scene,
        selector: 55,
        serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
    };
    drain(&mut listener);
    (listener, participant, endpoint)
}

#[test]
fn committed_activation_and_native_movie_gates_are_transactional_and_once_only() {
    crate::net::players::test_wire::with_native_padding(|| {
        finished_movie();
    });
}
fn finished_movie() -> (PlayerListener, u16) {
    let (mut listener, participant, endpoint) = fixture();
    let old_actor = listener
        .players
        .owned_actor(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    // The selected fixture has already completed objective10; no movie replay.
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.intro_movies.is_empty());
    let (_, _, snapshot) = crate::progression::tests::local();
    listener.persistent = Some(Arc::new(
        crate::progression::tests::tables().view(&snapshot).unwrap(),
    ));
    let world = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::super::returning::empty_section());
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.intro_movies.is_empty());
    assert_eq!(listener.players.objects().snapshot(), world);
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroMovie)
    );
    assert!(!listener.participants.in_free_roam().contains(&participant));
    let before = listener.intro_movies[&participant].sequence().unwrap();
    assert_eq!(listener.pending.iter().flat_map(|s| &s.records).count(), 1);
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending.is_empty());
    let done = native_completion(Completed {
        sequence: before,
        participant,
    });
    listener.pending_rpcs.resize(
        nfs_world::session::MAX_RPC_QUEUE,
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint,
            participant,
            call: nfs_world::participants::Call::Enter,
        }),
    );
    let world = listener.players.objects().snapshot();
    assert!(!send(&mut listener, 28, 47, done.span()));
    assert_eq!(listener.players.objects().snapshot(), world);
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Before
    );
    drain(&mut listener);
    assert!(send(&mut listener, 28, 47, done.span()));
    assert_eq!(
        listener
            .players
            .owned_actor(HOST_SELECTOR as u8, listener.persona, participant),
        None
    );
    assert!(listener.players.objects().get(old_actor).is_none());
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Playing
    );
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|r| matches!(r, HostRpc::Movie(_)))
            .count(),
        1
    );
    drain(&mut listener);
    assert!(send(&mut listener, 29, 47, done.span()));
    assert!(listener.pending_rpcs.is_empty());
    assert!(!send(
        &mut listener,
        30,
        47,
        request(endpoint, participant + 1, 3).span()
    ));
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Playing
    );
    listener
        .pending
        .resize(MAX_PENDING, super::super::super::returning::empty_section());
    let world = listener.players.objects().snapshot();
    assert!(!send(
        &mut listener,
        30,
        47,
        request(endpoint, participant, 3).span()
    ));
    assert_eq!(listener.players.objects().snapshot(), world);
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Playing
    );
    drain(&mut listener);
    assert!(send(
        &mut listener,
        30,
        47,
        request(endpoint, participant, 3).span()
    ));
    let after = listener.intro_movies[&participant].sequence().unwrap();
    let replacement = listener
        .players
        .owned_actor(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    assert_ne!(replacement, old_actor);
    assert!(replacement < after);
    assert_ne!(before, after);
    drain(&mut listener);
    assert!(send(
        &mut listener,
        31,
        47,
        request(endpoint, participant, 3).span()
    ));
    assert!(listener.pending.is_empty());
    let done = native_completion(Completed {
        sequence: after,
        participant,
    });
    assert!(send(&mut listener, 32, 47, done.span()));
    assert!(
        matches!(listener.pending_rpcs.front(),Some(HostRpc::Actor(binding)) if binding.actor==replacement && binding.participant==participant)
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Finished
    );
    drain(&mut listener);
    assert!(send(&mut listener, 33, 47, done.span()));
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(
        listener
            .players
            .owned_actor(HOST_SELECTOR as u8, listener.persona, participant),
        Some(replacement)
    );
    (listener, participant)
}

#[test]
fn second_race_reuses_the_selected_car_and_rolls_back_relocation_under_pressure() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant) = finished_movie();
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona: listener.persona,
            participant,
        };
        let vehicles = Arc::make_mut(listener.vehicles.as_mut().unwrap());
        vehicles.layout = crate::vehicle_content::population::tests::world_layout(&vehicles.layout);
        let inventory = listener.inventory.clone().unwrap();
        listener
            .population
            .as_mut()
            .unwrap()
            .spawn_world(
                &mut listener.players,
                crate::vehicle_content::population::Owner {
                    connection: owner.connection,
                    persona: owner.persona,
                    participant,
                },
                &inventory,
                &vehicles.vehicles,
                &vehicles.layout,
            )
            .unwrap();
        let car = listener
            .players
            .owned_vehicle(
                owner.connection,
                owner.persona,
                participant,
                inventory.slots.values()[0].unwrap(),
            )
            .unwrap();
        listener
            .players
            .bind_player_vehicle(owner.connection, owner.persona, participant, Some(car))
            .unwrap();
        listener.world_cars.insert(car, (participant, true));
        let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
        content.second_race = content.first_race.clone();
        content
            .second_race
            .as_mut()
            .unwrap()
            .destination
            .teleport
            .position = [200., 30., 400.];
        let arrival = listener
            .opening
            .start_stopped(
                &content.first_race.as_ref().unwrap().arrival,
                &mut listener.players,
                owner,
            )
            .unwrap()
            .unwrap()
            .id;
        listener
            .opening
            .complete(
                &mut listener.players,
                owner,
                Completed {
                    sequence: arrival,
                    participant,
                },
            )
            .unwrap();
        let previous = listener.intro_modes[&participant].0.ghost;
        let cancel = listener.intro_modes[&participant].1.ghost;
        assert!(send(
            &mut listener,
            34,
            47,
            request(
                Endpoint {
                    scene: previous,
                    selector: 0,
                    serial: nfs_protocol::world::rpc::Serial::new(1).unwrap()
                },
                participant,
                0
            )
            .span()
        ));
        assert!(listener.intro_modes[&participant].0.has_left());
        // The constructed mode is shared by both race fixtures; adopt its completed
        // first-race occurrence after exercising the same native retirement path.
        listener.intro_modes.get_mut(&participant).unwrap().2 = Chapter::FirstRace;
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroRelocating)
        );
        assert_eq!(
            listener.participants.intro_chapter(participant),
            Some(Chapter::SecondRace)
        );
        drain(&mut listener);
        let b = listener.participants.intro_bindings().unwrap();
        let dummy = Command {
            endpoint: b.teleport,
            participant,
            vehicle: car,
            position: [0.; 3],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            speed: 0.,
        };
        listener.pending_teleports.resize(MAX_PENDING, (0, dummy));
        let before = listener.players.objects().snapshot();
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroRelocating)
        );
        assert_eq!(listener.players.objects().snapshot(), before);
        listener.pending_teleports.clear();
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.pending_teleports.len(), 1);
        let teleport = listener.pending_teleports[0].1;
        assert_eq!(teleport.vehicle, car);
        assert_eq!(teleport.position, [200., 30., 400.]);
        assert_eq!(listener.world_cars.len(), 1);
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(listener.pending_teleports.len(), 1);
        let startup = |endpoint: Endpoint, method: u32| {
            let mut payload = BitWriter::new();
            payload
                .put(endpoint.selector.into(), 9)
                .put(method.into(), 32)
                .put(crate::net::players::test_wire::client_padding(), 7);
            let mut wire = BitWriter::new();
            wire.put(0, 32)
                .put(0, 32)
                .put(2, 8)
                .put(endpoint.scene.into(), 13)
                .put(participant.into(), 13)
                .put(payload.bytes().len() as u64, 9)
                .put_span(payload.span());
            wire
        };
        assert!(send(&mut listener, 35, 47, startup(b.teleport, 3).span()));
        assert!(send(
            &mut listener,
            36,
            47,
            startup(b.render_ready, 0).span()
        ));
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        let arrival = listener.opening.ghost(participant).unwrap();
        let done = native_completion(Completed {
            sequence: arrival,
            participant,
        });
        assert!(send(&mut listener, 37, 47, done.span()));
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::FreeRoam)
        );
        assert_eq!(listener.intro_modes[&participant].2, Chapter::SecondRace);
        assert_eq!(listener.intro_modes[&participant].1.ghost, cancel);
        assert_eq!(
            listener.players.owned_vehicle(
                owner.connection,
                owner.persona,
                participant,
                inventory.slots.values()[0].unwrap()
            ),
            Some(car)
        );
    });
}

#[test]
fn diner_replaces_finished_movie_and_timer_waits_for_capacity_and_native_ack() {
    crate::net::players::test_wire::with_native_padding(|| {
        finished_diner();
    });
}
#[test]
fn return_drive_waits_for_retirement_and_keeps_the_owned_car_and_canceller() {
    crate::net::players::test_wire::with_native_padding(|| {
        return_drive_started();
    });
}
fn return_drive_started() -> (PlayerListener, u16, u16) {
    let (listener, participant, car, _) = return_drive_started_with(None);
    (listener, participant, car)
}
/// Also returns the calls queued by the frame that started the return drive.
fn return_drive_started_with(
    launch: Option<nfs_content::events::intro::LaunchEvent>,
) -> (PlayerListener, u16, u16, Vec<HostRpc>) {
    let (mut listener, participant) = finished_diner();
    let owner = Owner {
        connection: HOST_SELECTOR as u8,
        persona: listener.persona,
        participant,
    };
    let vehicles = Arc::make_mut(listener.vehicles.as_mut().unwrap());
    vehicles.layout = crate::vehicle_content::population::tests::world_layout(&vehicles.layout);
    let inventory = listener.inventory.clone().unwrap();
    let entry = vehicles
        .layout
        .occupied(inventory.slots, &inventory.items)
        .unwrap()[0];
    let old_car = listener
        .players
        .owned_vehicle(owner.connection, owner.persona, participant, entry.item)
        .unwrap();
    let (scene, selector) = listener
        .players
        .objects()
        .scene_endpoint(vehicles.layout.scene_key, entry.slot.component_index)
        .unwrap();
    // This fixture skips driving both races, but their original garage load
    // acknowledgement must still exist before that slot can be replaced.
    listener
        .population
        .as_mut()
        .unwrap()
        .acknowledge(
            &listener.players,
            crate::vehicle_content::population::Owner {
                connection: owner.connection,
                persona: owner.persona,
                participant,
            },
            nfs_world::garage::vehicle::Loaded {
                scene,
                selector,
                participant,
                vehicle: old_car,
            },
        )
        .unwrap();
    listener
        .population
        .as_mut()
        .unwrap()
        .spawn_world(
            &mut listener.players,
            crate::vehicle_content::population::Owner {
                connection: owner.connection,
                persona: owner.persona,
                participant,
            },
            &inventory,
            &vehicles.vehicles,
            &vehicles.layout,
        )
        .unwrap();
    let item = inventory.slots.values()[0].unwrap();
    let car = listener
        .players
        .owned_vehicle(owner.connection, owner.persona, participant, item)
        .unwrap();
    listener
        .players
        .bind_player_vehicle(owner.connection, owner.persona, participant, Some(car))
        .unwrap();
    listener.world_cars.insert(car, (participant, true));
    let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
    content.second_race = content.first_race.clone();
    // A self-contained mode exercises listener ownership/ordering; installed
    // return-mode timer and wire checks live in the private reference harness.
    content.return_drive = Some(content.first_race.as_ref().unwrap().mode.clone());
    content.return_drive.as_mut().unwrap().data.launch_event = launch;
    let arrival = listener
        .opening
        .start_stopped(
            &content.second_race.as_ref().unwrap().arrival,
            &mut listener.players,
            owner,
        )
        .unwrap()
        .unwrap()
        .id;
    listener
        .opening
        .complete(
            &mut listener.players,
            owner,
            Completed {
                sequence: arrival,
                participant,
            },
        )
        .unwrap();
    let old = listener.intro_modes[&participant].0.ghost;
    let canceller = listener.intro_modes[&participant].1.ghost;
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(
        listener.participants.intro_chapter(participant),
        Some(Chapter::SecondRace)
    );
    assert_eq!(listener.intro_modes[&participant].0.ghost, old);
    assert!(send(
        &mut listener,
        37,
        47,
        request(
            Endpoint {
                scene: old,
                selector: 0,
                serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
            },
            participant,
            0
        )
        .span()
    ));
    assert!(listener.intro_modes[&participant].0.has_left());
    listener.intro_modes.get_mut(&participant).unwrap().2 = Chapter::SecondRace;
    drain(&mut listener);
    let snapshot = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::super::returning::empty_section());
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), snapshot);
    assert_eq!(
        listener.participants.intro_chapter(participant),
        Some(Chapter::SecondRace)
    );
    drain(&mut listener);
    listener.world_cars.insert(car, (participant, false));
    assert!(tick_with_normal_presentation(&mut listener).is_err());
    assert_eq!(listener.players.objects().snapshot(), snapshot);
    assert_eq!(listener.intro_modes[&participant].0.ghost, old);
    listener.world_cars.insert(car, (participant, true));
    tick_with_normal_presentation(&mut listener).unwrap();
    let started: Vec<HostRpc> = listener.pending_rpcs.iter().copied().collect();
    assert_eq!(
        listener.participants.intro_chapter(participant),
        Some(Chapter::ReturnDrive)
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    assert_eq!(listener.intro_modes[&participant].2, Chapter::ReturnDrive);
    assert_ne!(listener.intro_modes[&participant].0.ghost, old);
    assert_eq!(listener.intro_modes[&participant].1.ghost, canceller);
    assert!(listener.pending_teleports.is_empty());
    assert_eq!(
        listener
            .players
            .owned_vehicle(owner.connection, owner.persona, participant, item),
        Some(car)
    );
    let snapshot = listener.players.objects().snapshot();
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), snapshot);
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    (listener, participant, car, started)
}

fn mode_calls(calls: &[HostRpc]) -> usize {
    calls
        .iter()
        .filter(|c| matches!(c, HostRpc::GameMode(_)))
        .count()
}
fn launch_fire(calls: &[HostRpc]) -> Option<u16> {
    calls.iter().find_map(|c| match c {
        HostRpc::Event(fire) if fire.target.entity == 8 && fire.event == 7 => {
            Some(fire.target.ghost)
        }
        _ => None,
    })
}

#[test]
fn return_drive_launch_waits_for_the_clients_launcher_answer() {
    crate::net::players::test_wire::with_native_padding(|| {
        let event = nfs_content::events::intro::LaunchEvent { bus: 8, ordinal: 7 };
        let (_, _, _, immediate) = return_drive_started_with(None);
        assert_eq!(launch_fire(&immediate), None);
        let (mut listener, participant, _, started) = return_drive_started_with(Some(event));
        // The launch event replaces the launch call until the client answers;
        // the item clear (gameplay s7 m2, official E1026 891.523 s) goes with
        // the launch event only.
        let clears = |calls: &[HostRpc]| {
            calls
                .iter()
                .filter(|c| matches!(c, HostRpc::GameMode(g)
                    if g.method == 2 && g.reference == Some(participant)
                        && matches!(g.arguments, nfs_world::game_mode::Arguments::None)
                        && g.selector == listener.participants.lifecycle_bindings().unwrap().garage[0].selector))
                .count()
        };
        assert_eq!(clears(&started), 1);
        assert_eq!(clears(&immediate), 0);
        assert_eq!(mode_calls(&started), mode_calls(&immediate));
        let parent = launch_fire(&started).expect("launch event on the participant logic");
        let (scene, selector) = listener.intro_modes[&participant]
            .0
            .launcher_endpoint(
                listener
                    .prologue
                    .as_ref()
                    .unwrap()
                    .mode(Chapter::ReturnDrive)
                    .unwrap(),
                &listener.players,
            )
            .unwrap();
        assert_eq!(scene, parent);
        let endpoint = Endpoint {
            scene,
            selector,
            serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        };
        assert!(send(
            &mut listener,
            38,
            47,
            request(endpoint, participant, 6).span()
        ));
        let answered: Vec<HostRpc> = listener.pending_rpcs.iter().copied().collect();
        assert_eq!(mode_calls(&answered), 1);
        assert_eq!(answered.len(), 1);
        drain(&mut listener);
        // Answered once: a repeated answer launches nothing more, and the
        // deadline no longer applies.
        assert!(send(
            &mut listener,
            39,
            47,
            request(endpoint, participant, 6).span()
        ));
        assert_eq!(
            mode_calls(&listener.pending_rpcs.iter().copied().collect::<Vec<_>>()),
            0
        );
        drain(&mut listener);
        listener.world_ms += 10_000;
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            mode_calls(&listener.pending_rpcs.iter().copied().collect::<Vec<_>>()),
            0
        );
    });
}

#[test]
fn return_drive_launches_at_the_handshake_deadline_without_an_answer() {
    crate::net::players::test_wire::with_native_padding(|| {
        let event = nfs_content::events::intro::LaunchEvent { bus: 8, ordinal: 7 };
        let (mut listener, _, _, started) = return_drive_started_with(Some(event));
        assert!(launch_fire(&started).is_some());
        let deadline_ms = (crate::prologue::driving::LAUNCH_HANDSHAKE_SECONDS * 1000.) as u64;
        listener.world_ms += deadline_ms - 1_000;
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            mode_calls(&listener.pending_rpcs.iter().copied().collect::<Vec<_>>()),
            0
        );
        listener.world_ms += 1_000;
        tick_with_normal_presentation(&mut listener).unwrap();
        let launched: Vec<HostRpc> = listener.pending_rpcs.iter().copied().collect();
        assert_eq!(mode_calls(&launched), 1);
        drain(&mut listener);
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            mode_calls(&listener.pending_rpcs.iter().copied().collect::<Vec<_>>()),
            0
        );
    });
}
fn finished_diner() -> (PlayerListener, u16) {
    let (mut listener, participant) = finished_movie();
    let b = listener.participants.intro_bindings().unwrap();
    assert!(matches!(
        listener.participants.intro_second_race_begin(participant),
        Outcome::Advanced(_)
    ));
    listener.participants.intro_relocation_ready(participant);
    for (endpoint, method) in [(b.teleport, 3), (b.render_ready, 0)] {
        let mut p = BitWriter::new();
        p.put(endpoint.selector.into(), 9)
            .put(method, 32)
            .put(crate::net::players::test_wire::client_padding(), 7);
        let mut wire = BitWriter::new();
        wire.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(endpoint.scene.into(), 13)
            .put(participant.into(), 13)
            .put(p.bytes().len() as u64, 9)
            .put_span(p.span());
        listener
            .participants
            .receive_intro(wire.span(), |_| true)
            .unwrap();
    }
    listener.participants.intro_opening_complete(participant);
    listener.participants.intro_drive_ready(participant);
    assert_eq!(
        listener.participants.intro_chapter(participant),
        Some(Chapter::SecondRace)
    );
    let c = Arc::make_mut(listener.prologue.as_mut().unwrap());
    let mut data = c.manu_movie.as_ref().unwrap().data.clone();
    data.objective = 20;
    data.video.index = 0;
    c.diner_movie = Some(crate::prologue::movies::Content::for_test(
        data,
        c.opening.clone(),
        2.,
    ));
    // The next objective is not active until the prior result is committed.
    let started = listener.world_ms + 1000;
    listener.world_ms = started;
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.intro_movies[&participant].objective(), 10);
    use nfs_storage::InventoryRepository;
    let (repo, account, saved) = crate::progression::tests::local();
    let progression = listener.progression.as_ref().unwrap();
    let policy = progression.catalog.story_policy(progression.settings);
    let nfs_services::story::Prepared::Mutation(batch) = policy
        .complete(
            &saved,
            saved.generation,
            policy.next_operation(&saved).unwrap(),
            10,
            vec![],
        )
        .unwrap()
    else {
        panic!()
    };
    repo.apply(account, &batch, nfs_storage::Timestamp(3))
        .unwrap();
    let saved = repo.snapshot(account).unwrap();
    listener.persistent = Some(Arc::new(
        crate::progression::tests::tables().view(&saved).unwrap(),
    ));
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.intro_movies.len(), 1);
    assert_eq!(listener.intro_movies[&participant].objective(), 20);
    let before = listener.intro_movies[&participant].sequence().unwrap();
    drain(&mut listener);
    let done = native_completion(Completed {
        sequence: before,
        participant,
    });
    assert!(send(&mut listener, 34, 47, done.span()));
    drain(&mut listener);
    let endpoint = Endpoint {
        scene: b.alive.scene,
        selector: 55,
        serial: b.alive.serial,
    };
    listener.world_ms = started + 2000;
    assert!(send(
        &mut listener,
        35,
        47,
        request(endpoint, participant, 3).span()
    ));
    assert!(matches!(
        listener.pending_rpcs.back(),
        Some(HostRpc::Actor(_))
    ));
    let after = listener.intro_movies[&participant].sequence().unwrap();
    drain(&mut listener);
    let done = native_completion(Completed {
        sequence: after,
        participant,
    });
    let snapshot = listener.players.objects().snapshot();
    assert!(!send(&mut listener, 36, 47, done.span()));
    assert_eq!(listener.players.objects().snapshot(), snapshot);
    listener.world_ms = started + 3999;
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending_rpcs.is_empty());
    listener.world_ms = started + 4000;
    listener.pending_rpcs.resize(
        nfs_world::session::MAX_RPC_QUEUE,
        HostRpc::Participant(nfs_world::participants::Notification {
            endpoint,
            participant,
            call: nfs_world::participants::Call::Enter,
        }),
    );
    tick_with_normal_presentation(&mut listener).unwrap();
    assert_eq!(listener.players.objects().snapshot(), snapshot);
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(
        matches!(listener.pending_rpcs.front(), Some(HostRpc::SequenceStop(stop)) if stop.sequence == after)
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroMovie)
    );
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending_rpcs.is_empty());
    assert!(send(&mut listener, 36, 47, done.span()));
    assert!(
        !listener
            .pending_rpcs
            .iter()
            .any(|c| matches!(c, HostRpc::Actor(_)))
    );
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    assert_eq!(
        listener.intro_movies[&participant].phase(),
        nfs_world::fmv::Phase::Finished
    );
    drain(&mut listener);
    tick_with_normal_presentation(&mut listener).unwrap();
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    (listener, participant)
}

/// The movie start's `alive` leave reaches the world volumes through the same
/// send path as every notification, so the activity memberships leave with it.
#[test]
fn movie_start_leaves_the_activity_volumes_once_sent() {
    crate::net::players::test_wire::with_native_padding(|| {
        let (mut listener, participant, _) = fixture();
        let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
        content.volumes.activities = (21..=23)
            .map(|serializer| nfs_content::events::discovery::Endpoint {
                scene_key: 909,
                serializer,
            })
            .collect();
        let alive = listener.participants.intro_bindings().unwrap().alive;
        let memberships = |sent: &[HostRpc], method: u32| {
            sent.iter()
                .filter(|rpc| matches!(rpc, HostRpc::GameMode(c) if c.method == method))
                .count()
        };
        listener.pending_rpcs.push_back(HostRpc::Participant(
            nfs_world::participants::Notification {
                endpoint: alive,
                participant,
                call: nfs_world::participants::Call::Enter,
            },
        ));
        send_all(&mut listener);
        listener.advance_world_volumes(0).unwrap();
        assert_eq!(memberships(&send_all(&mut listener), 0), 3);
        let (_, _, snapshot) = crate::progression::tests::local();
        listener.persistent = Some(Arc::new(
            crate::progression::tests::tables().view(&snapshot).unwrap(),
        ));
        tick_with_normal_presentation(&mut listener).unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroMovie)
        );
        // Queued but not yet accepted by the session: nothing changes.
        listener.advance_world_volumes(0).unwrap();
        let sent = send_all(&mut listener);
        assert_eq!(memberships(&sent, 1), 0);
        assert!(sent.iter().any(|rpc| matches!(rpc,
            HostRpc::Participant(n) if n.endpoint == alive
                && n.participant == participant
                && n.call == nfs_world::participants::Call::Leave)));
        // That sent the movie start; the next tick leaves the activities.
        listener.advance_world_volumes(0).unwrap();
        assert_eq!(memberships(&send_all(&mut listener), 1), 3);
    });
}
