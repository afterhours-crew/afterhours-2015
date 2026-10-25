// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn request_zero(endpoint: Endpoint, participant: u16) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(0, 32)
        .put(0, 7);
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
    let (mut listener, participant, car) = return_drive_started();
    let selected_sequence = listener.selection.ghost(participant);
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
    let enter = request_zero(endpoint, participant);
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
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before);
    drain(&mut listener);
    listener.advance_prologue().unwrap();
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
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), staged_world);
    assert!(listener.pending_rpcs.is_empty());
    assert!(send(&mut listener, 40, 47, loaded.span()));
    let complete = Completed {
        sequence: entry,
        participant,
    }
    .encode()
    .unwrap();
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
    listener.advance_prologue().unwrap();
    assert_eq!(listener.players.objects().snapshot(), after);
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::IntroLoadingGarage)
    );
    assert_eq!(listener.selection.ghost(participant), selected_sequence);
}
