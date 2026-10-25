// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{drain, send};
use super::*;
use nfs_world::{bits::BitWriter, participants::Endpoint};
use std::sync::Arc;

fn request(endpoint: Endpoint, participant: u16, method: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(method.into(), 32)
        .put(127, 7);
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
        listener.participants.intro_vehicle_ready(participant),
        Outcome::Advanced(_)
    ));
    // Startup acknowledgements use zero padding in this constructed fixture.
    let startup = |endpoint: Endpoint, method: u32| {
        // Build the native zero-aligned startup form separately.
        let mut p = BitWriter::new();
        p.put(endpoint.selector.into(), 9)
            .put(method.into(), 32)
            .put(0, 7);
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
    finished_movie();
}
fn finished_movie() -> (PlayerListener, u16) {
    let (mut listener, participant, endpoint) = fixture();
    let old_actor = listener
        .players
        .owned_actor(HOST_SELECTOR as u8, listener.persona, participant)
        .unwrap();
    // The selected fixture has already completed objective10; no movie replay.
    listener.advance_story_movies().unwrap();
    assert!(listener.intro_movies.is_empty());
    let (_, _, snapshot) = crate::progression::tests::local();
    listener.persistent = Some(Arc::new(
        crate::progression::tests::tables().view(&snapshot).unwrap(),
    ));
    let world = listener.players.objects().snapshot();
    listener
        .pending
        .resize(MAX_PENDING, super::super::super::returning::empty_section());
    listener.advance_story_movies().unwrap();
    assert!(listener.intro_movies.is_empty());
    assert_eq!(listener.players.objects().snapshot(), world);
    drain(&mut listener);
    listener.advance_story_movies().unwrap();
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroMovie)
    );
    assert!(!listener.participants.in_free_roam().contains(&participant));
    let before = listener.intro_movies[&participant].sequence().unwrap();
    assert_eq!(listener.pending.iter().flat_map(|s| &s.records).count(), 1);
    drain(&mut listener);
    listener.advance_story_movies().unwrap();
    assert!(listener.pending.is_empty());
    let done = Completed {
        sequence: before,
        participant,
    }
    .encode()
    .unwrap();
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
    let done = Completed {
        sequence: after,
        participant,
    }
    .encode()
    .unwrap();
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
    listener.advance_story_movies().unwrap();
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
    listener.advance_prologue().unwrap();
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
    listener.advance_prologue().unwrap();
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroRelocating)
    );
    assert_eq!(listener.players.objects().snapshot(), before);
    listener.pending_teleports.clear();
    listener.advance_prologue().unwrap();
    assert_eq!(listener.pending_teleports.len(), 1);
    let teleport = listener.pending_teleports[0].1;
    assert_eq!(teleport.vehicle, car);
    assert_eq!(teleport.position, [200., 30., 400.]);
    assert_eq!(listener.world_cars.len(), 1);
    drain(&mut listener);
    listener.advance_prologue().unwrap();
    assert_eq!(listener.pending_teleports.len(), 1);
    let startup = |endpoint: Endpoint, method: u32| {
        let mut payload = BitWriter::new();
        payload
            .put(endpoint.selector.into(), 9)
            .put(method.into(), 32)
            .put(0, 7);
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
    listener.advance_prologue().unwrap();
    let arrival = listener.opening.ghost(participant).unwrap();
    let done = Completed {
        sequence: arrival,
        participant,
    }
    .encode()
    .unwrap();
    assert!(send(&mut listener, 37, 47, done.span()));
    drain(&mut listener);
    listener.advance_prologue().unwrap();
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
}
