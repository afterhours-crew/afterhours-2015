// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::{
    bits::BitWriter,
    participants::{Bindings, Outcome},
};
use std::sync::Arc;

fn point(x: f32) -> Destination {
    Destination {
        position: [x, 2., 3.],
        basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
    }
}
fn request(
    scene: u16,
    selector: u16,
    method: u32,
    references: &[u16],
    arguments: &[bool],
) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for &bit in arguments {
        payload.put_bool(bit);
    }
    payload.align();
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put((references.len() + 1) as u64, 8)
        .put(scene.into(), 13);
    for &id in references {
        body.put(id.into(), 13);
    }
    body.put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
fn fixture() -> (PlayerListener, u16, Definitions) {
    let (mut listener, participant, _) = super::super::prologue::tests::setup();
    listener.prologue = None;
    let records = listener.players.objects().snapshot();
    let item_key = listener.vehicles.as_ref().unwrap().layout.item_scene_key;
    let bindings = Bindings::from_records(&records, item_key, 909)
        .unwrap()
        .unwrap();
    let mut lifecycle = Lifecycle::default();
    lifecycle.bind(&records, item_key, 909).unwrap();
    lifecycle.begin(participant).unwrap();
    lifecycle.set_garage([None; 5]).unwrap();
    lifecycle
        .receive(
            request(
                bindings.inventory.scene,
                bindings.inventory.selector,
                3,
                &[participant],
                &[false; 7],
            )
            .span(),
            |_| true,
        )
        .unwrap();
    lifecycle.take_garage_bindings();
    lifecycle.finish_persistent(|_| true);
    assert!(lifecycle.begin_progression(participant));
    assert!(!lifecycle.enter_garage_loading(participant).is_empty());
    assert!(matches!(
        lifecycle.enter_customization(participant),
        Outcome::Advanced(_)
    ));
    listener.participants = lifecycle;
    let definitions = Definitions {
        manager: Some(Site {
            scene_key: 909,
            serializer: 4,
        }),
        locations: BTreeMap::from([(
            Site {
                scene_key: 909,
                serializer: 8,
            },
            vec![point(123.), point(456.)],
        )]),
    };
    let event = crate::event_logic::tests::content();
    let availability =
        crate::event_logic::availability::teleport_fixture(&event, definitions.clone());
    listener.events = Some(Arc::new(
        crate::event_logic::catalog::Catalog::new_gated(vec![event], availability).unwrap(),
    ));
    (listener, participant, definitions)
}
fn selection(
    listener: &PlayerListener,
    defs: &Definitions,
    participant: u16,
    selector_offset: u16,
) -> BitWriter {
    let world = listener.players.objects();
    let manager = defs.manager.unwrap();
    let (manager, method) = world
        .scene_endpoint(manager.scene_key, manager.serializer)
        .unwrap();
    let site = *defs.locations.keys().next().unwrap();
    let (scene, selector) = world
        .scene_endpoint(site.scene_key, site.serializer)
        .unwrap();
    assert_ne!(
        usize::from(selector),
        site.serializer,
        "fixture must distinguish selector from serializer"
    );
    let arguments = (0..9)
        .rev()
        .map(|i| ((selector + selector_offset) >> i) & 1 != 0)
        .chain([false; 6])
        .collect::<Vec<_>>();
    request(manager, method, 0, &[participant, scene], &arguments)
}
fn send(listener: &mut PlayerListener, inputs: &[BitWriter], sequence: u8) -> bool {
    let wire = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages: inputs
                    .iter()
                    .map(|input| frame::Message::Opaque {
                        index: 47,
                        body: input.span(),
                    })
                    .collect(),
            }],
        )
        .build()
        .unwrap();
    listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: wire.span(),
    })
}
#[test]
fn framed_selection_uses_current_selector_and_rolls_back_on_later_foreign_input() {
    let (mut listener, participant, defs) = fixture();
    let valid = selection(&listener, &defs, participant, 0);
    let foreign = selection(&listener, &defs, 8000, 0);
    assert!(!send(&mut listener, &[valid, foreign], 1));
    assert!(listener.selected_destinations.selected.is_empty());
    assert!(listener.selected_destinations.cursors.is_empty());
    let valid = selection(&listener, &defs, participant, 0);
    assert!(send(&mut listener, &[valid], 2));
    assert_eq!(
        listener.selected_destinations.position(participant),
        Some(point(123.))
    );
    assert_eq!(listener.unsupported_rpcs, 0);
    let repeat = selection(&listener, &defs, participant, 0);
    assert!(send(&mut listener, &[repeat], 3));
    assert_eq!(
        listener.selected_destinations.position(participant),
        Some(point(123.))
    );
    let wrong = selection(&listener, &defs, participant, 1);
    assert!(!send(&mut listener, &[wrong], 4));
    assert_eq!(
        listener.selected_destinations.position(participant),
        Some(point(123.))
    );
}
#[test]
fn owned_selection_rejects_foreign_account_and_wrong_phase_without_mutation() {
    let (listener, participant, defs) = fixture();
    let input = selection(&listener, &defs, participant, 0);
    let mut state = State::default();
    assert!(
        state
            .receive(
                Some(&defs),
                &listener.players,
                &listener.participants,
                listener.persona + 1,
                input.span()
            )
            .is_err()
    );
    assert!(
        state
            .receive(
                Some(&defs),
                &listener.players,
                &Lifecycle::default(),
                listener.persona,
                input.span()
            )
            .is_err()
    );
    assert!(state.selected.is_empty() && state.cursors.is_empty());
}
#[test]
fn selection_is_per_participant_repeat_safe_and_cleared_after_departure() {
    let site = Site {
        scene_key: 123,
        serializer: 8,
    };
    let points = [point(1.), point(2.)];
    let mut state = State::default();
    state
        .select(20, site, Some(Stage::Customization), &points)
        .unwrap();
    state
        .select(20, site, Some(Stage::ExitingGarage), &points)
        .unwrap();
    state
        .select(21, site, Some(Stage::Customization), &points)
        .unwrap();
    assert_eq!(state.position(20), Some(point(1.)));
    assert_eq!(state.position(21), Some(point(2.)));
    assert_eq!(state.position(22), None);
    assert!(
        state
            .select(
                20,
                Site {
                    serializer: 9,
                    ..site
                },
                Some(Stage::ExitingGarage),
                &points
            )
            .is_err()
    );
    state.retain(&Lifecycle::default());
    assert_eq!(state.position(20), None);
    assert_eq!(state.position(21), None);
}

#[test]
fn selected_destination_survives_loading_and_arrival_then_clears_in_free_roam() {
    use nfs_world::replication::sublevel;
    let (mut listener, participant, defs) = fixture();
    let input = selection(&listener, &defs, participant, 0);
    assert!(send(&mut listener, &[input], 1));
    let mut kinds = vec![sublevel::Kind::Rpc; 10];
    kinds[0] = sublevel::Kind::Noop;
    let scenes =
        sublevel::Content::new(vec![(910, sublevel::Profile::new(false, &kinds).unwrap())])
            .unwrap();
    let creation = sublevel::passive::creation(
        3000,
        910,
        nfs_protocol::world::rpc::Serial::new(1).unwrap(),
        &scenes,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[3000], vec![creation], &scenes)
        .unwrap();
    listener
        .participants
        .bind_exit(&listener.players.objects().snapshot(), 909, 910)
        .unwrap();
    let exit = listener.participants.exit_bindings().unwrap();
    for (endpoint, method, stage) in [
        (exit.exit_request, 0, Stage::ExitingGarage),
        (exit.state_77, 3, Stage::EnteringWorld),
        (exit.ready_request, 0, Stage::Arriving),
    ] {
        let input = request(
            endpoint.scene,
            endpoint.selector,
            method,
            &[participant],
            &[false; 7],
        );
        assert!(matches!(
            listener
                .participants
                .receive(input.span(), |id| id == participant)
                .unwrap(),
            Outcome::Advanced(_)
        ));
        assert_eq!(listener.participants.stage(participant), Some(stage));
        listener
            .selected_destinations
            .retain(&listener.participants);
        assert_eq!(
            listener.selected_destinations.position(participant),
            Some(point(123.))
        );
    }
    assert!(matches!(
        listener.participants.arrived(participant),
        Outcome::Advanced(_)
    ));
    assert_eq!(
        listener.participants.stage(participant),
        Some(Stage::FreeRoam)
    );
    listener
        .selected_destinations
        .retain(&listener.participants);
    assert_eq!(listener.selected_destinations.position(participant), None);
}
