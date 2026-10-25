// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::replication::players::{Players, Request};

fn bindings() -> Bindings {
    let e = |scene, selector| Endpoint {
        scene,
        selector,
        serial: Serial::new(0).unwrap(),
    };
    Bindings {
        manager: e(2, 0),
        preparing: e(4, 5),
        inventory: e(4, 10),
        persistent: e(4, 6),
        check_player: e(4, 7),
        persistent_loaded: e(4, 24),
        garage: [7, 8, 9, 10, 11].map(|selector| e(2, selector)),
        recovery: e(4, 20),
        progression_loaded: e(4, 17),
        entered_game: e(4, 18),
        enter_garage_from_boot: e(4, 113),
        begin_garage: e(4, 94),
        loading_garage: e(4, 89),
        composite: e(4, 88),
        initial_teleport: e(4, 85),
        customization: e(4, 86),
    }
}

#[test]
fn garage_loading_chain_follows_persistent_loaded_once_in_captured_order() {
    let mut model = ready();
    model.begin(8).unwrap();
    model.begin(9).unwrap();
    assert!(model.enter_garage_loading(8).is_empty());
    assert!(model.waiting_progression().is_empty());
    model.set_garage([None; 5]).unwrap();
    model.receive(ack(8, 3).span(), |p| p == 8).unwrap();
    model.take_garage_bindings();
    model.finish_persistent(|p| p == 8);
    assert_eq!(model.waiting_progression(), vec![8]);
    let before = model.clone();
    assert!(model.enter_garage_loading(8).is_empty());
    assert!(!model.begin_progression(9));
    assert!(!model.begin_progression(77));
    assert_eq!(model, before);
    assert!(model.begin_progression(8));
    assert!(!model.begin_progression(8));
    assert_eq!(model.stage(8), Some(Stage::Progressing));
    assert!(model.waiting_progression().is_empty());
    assert!(model.enter_garage_loading(9).is_empty());
    let chain = model.enter_garage_loading(8);
    let b = bindings();
    assert_eq!(
        chain
            .iter()
            .map(|n| (n.endpoint.selector, n.call.method(), n.participant))
            .collect::<Vec<_>>(),
        [
            (b.persistent_loaded, 1),
            (b.recovery, 0),
            (b.recovery, 1),
            (b.progression_loaded, 0),
            (b.progression_loaded, 1),
            (b.entered_game, 0),
            (b.entered_game, 1),
            (b.enter_garage_from_boot, 0),
            (b.enter_garage_from_boot, 1),
            (b.begin_garage, 0),
            (b.begin_garage, 1),
            (b.loading_garage, 0),
        ]
        .map(|(endpoint, method)| (endpoint.selector, method, 8))
    );
    assert_eq!(model.stage(8), Some(Stage::LoadingGarage));
    assert!(model.waiting_progression().is_empty());
    assert!(model.enter_garage_loading(8).is_empty());
    assert_eq!(model.stage(8), Some(Stage::LoadingGarage));
    assert_eq!(
        model.receive(ack(8, 3).span(), |_| true).unwrap(),
        Outcome::Repeated
    );
    for n in chain {
        assert!(n.encode().is_ok());
    }
}
pub(super) fn ready() -> Lifecycle {
    Lifecycle {
        bindings: Some(bindings()),
        stages: BTreeMap::new(),
        ..Lifecycle::default()
    }
}

#[test]
fn garage_waits_for_both_committed_inventory_and_owned_ack_and_repeats_silently() {
    let mut a = ready();
    a.begin(8).unwrap();
    a.begin(9).unwrap();
    a.set_garage([Some(u64::MAX), None, Some(42), None, None])
        .unwrap();
    assert!(a.take_garage_bindings().is_empty());
    a.receive(ack(8, 3).span(), |p| p == 8).unwrap();
    let out = a.take_garage_bindings();
    assert_eq!(
        out.iter()
            .map(|v| (v.endpoint.selector, v.item, v.participant))
            .collect::<Vec<_>>(),
        vec![(7, u64::MAX, 8), (9, 42, 8)]
    );
    assert_eq!(a.stage(8), Some(Stage::LoadingPersistentData));
    assert!(a.take_garage_bindings().is_empty());
    a.receive(ack(8, 3).span(), |_| true).unwrap();
    assert!(a.take_garage_bindings().is_empty());
    let saved = a.clone();
    assert!(a.set_garage([Some(44), None, None, None, None]).is_err());
    assert_eq!(a, saved);
    let mut b = ready();
    b.begin(10).unwrap();
    b.receive(ack(10, 3).span(), |_| true).unwrap();
    assert!(b.take_garage_bindings().is_empty());
    b.set_garage([Some(77), None, None, None, None]).unwrap();
    assert_eq!(b.take_garage_bindings()[0].item, 77);
    a.receive(ack(9, 3).span(), |_| true).unwrap();
    assert!(a.take_garage_bindings().iter().all(|v| v.participant == 9));
}
fn ack(participant: u16, method: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(10, 9).put(method.into(), 32).put(127, 7);
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(4, 13)
        .put(participant.into(), 13)
        .put(6, 9)
        .put_span(payload.span());
    body
}

#[test]
fn persistent_completion_waits_for_ack_and_ordered_garage_then_repeats_silently() {
    let mut model = ready();
    model.begin(8).unwrap();
    model.begin(9).unwrap();
    model
        .set_garage([Some(45), None, None, None, None])
        .unwrap();
    assert!(model.finish_persistent(|_| true).is_empty());
    model.receive(ack(8, 3).span(), |p| p == 8).unwrap();
    assert!(model.finish_persistent(|_| true).is_empty());
    let garage = model.take_garage_bindings();
    assert_eq!((garage.len(), garage[0].participant), (1, 8));
    let before = model.clone();
    assert!(model.finish_persistent(|_| false).is_empty());
    assert_eq!(model, before);
    let finished = model.finish_persistent(|id| id == 8);
    assert_eq!(
        finished,
        [
            (bindings().persistent, Call::Leave),
            (bindings().check_player, Call::Enter),
            (bindings().check_player, Call::Leave),
            (bindings().persistent_loaded, Call::Enter),
        ]
        .into_iter()
        .map(|(endpoint, call)| Notification {
            endpoint,
            participant: 8,
            call,
        })
        .collect::<Vec<_>>()
    );
    assert_eq!(model.stage(8), Some(Stage::PersistentDataLoaded));
    assert_eq!(model.stage(9), Some(Stage::LoadingInventory));
    assert!(model.finish_persistent(|_| true).is_empty());
    assert!(model.take_garage_bindings().is_empty());
    assert_eq!(
        model.receive(ack(8, 3).span(), |_| true).unwrap(),
        Outcome::Repeated
    );
}

#[test]
fn connected_check_is_per_participant_and_waits_for_progression_after_entry() {
    let mut model = ready();
    model.set_garage([None; 5]).unwrap();
    for id in [8, 9] {
        model.begin(id).unwrap();
        model.receive(ack(id, 3).span(), |p| p == id).unwrap();
    }
    assert!(model.take_garage_bindings().is_empty());
    let first = model.finish_persistent(|p| p == 8);
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|n| n.participant == 8));
    assert_eq!(model.stage(9), Some(Stage::LoadingPersistentData));
    assert!(model.finish_persistent(|p| p == 8).is_empty());
    let second = model.finish_persistent(|p| p == 9);
    assert_eq!(second.len(), 4);
    assert!(second.iter().all(|n| n.participant == 9));
    assert!(model.finish_persistent(|_| true).is_empty());
    assert_eq!(model.stage(8), Some(Stage::PersistentDataLoaded));
    assert_eq!(model.stage(9), Some(Stage::PersistentDataLoaded));
    let fresh = ready();
    assert_eq!(fresh.stage(8), None);
}

#[test]
fn owned_join_copies_current_identity_isolates_players_and_repeats() {
    let mut players = Players::default();
    let req = || Request {
        name: b"Local".to_vec(),
        flag: false,
        slot: 0,
    };
    let one = players.create(1, 100, req()).unwrap().unwrap();
    let two = players.create(2, 200, req()).unwrap().unwrap();
    assert_eq!(players.join(1, 200, two.id), Err(Error::UnknownObject));
    assert_eq!(players.join(2, 100, one.id), Err(Error::UnknownObject));
    let participant = players.join(1, 100, one.id).unwrap().unwrap();
    assert_eq!(participant.id, 3);
    assert!(players.owns_participant(1, 100, 3));
    assert!(!players.owns_participant(2, 200, 3));
    let Some(Initial::Participant(p)) = &participant.initial else {
        panic!()
    };
    assert_eq!((p.player, p.identity.identity.persona), (one.id, 100));
    assert_eq!(p.identity.name, b"Local");
    assert_eq!(players.join(1, 100, one.id).unwrap(), None);
    assert_eq!(players.objects().snapshot()[2], participant);
    assert_eq!(
        Record::decode(participant.encode().unwrap().span(), None)
            .unwrap()
            .record,
        participant
    );
}

#[test]
fn inventory_ack_advances_only_its_owned_participant_once() {
    let mut a = ready();
    let begin = a.begin(8).unwrap();
    assert_eq!(
        begin.iter().map(|n| n.call).collect::<Vec<_>>(),
        vec![
            Call::Add,
            Call::Enter,
            Call::Leave,
            Call::Enter,
            Call::LoadInventory
        ]
    );
    assert!(a.begin(8).unwrap().is_empty());
    assert_eq!(a.stage(8), Some(Stage::LoadingInventory));
    let mut b = ready();
    b.begin(9).unwrap();
    let wire = ack(8, 3);
    assert_eq!(a.receive(wire.span(), |_| false), Err(Error::UnknownObject));
    assert_eq!(b.receive(wire.span(), |_| true), Err(Error::UnknownObject));
    let Outcome::Advanced(out) = a.receive(wire.span(), |id| id == 8).unwrap() else {
        panic!()
    };
    assert_eq!(
        out.iter().map(|n| n.call).collect::<Vec<_>>(),
        vec![Call::Leave, Call::Enter]
    );
    assert_eq!(a.stage(8), Some(Stage::LoadingPersistentData));
    assert_eq!(b.stage(9), Some(Stage::LoadingInventory));
    assert_eq!(a.receive(wire.span(), |_| true).unwrap(), Outcome::Repeated);
}

#[test]
fn rejected_and_truncated_acknowledgements_leave_state_unchanged() {
    let mut model = ready();
    model.begin(8).unwrap();
    let original = model.clone();
    let wire = ack(8, 3);
    for bits in 0..wire.len() {
        assert!(
            model
                .receive(wire.span().slice(0, bits).unwrap(), |_| true)
                .is_err()
        );
        assert_eq!(model, original);
    }
    let mut trailing = wire.clone();
    trailing.put(0, 1);
    assert!(model.receive(trailing.span(), |_| true).is_err());
    assert_eq!(model, original);
    assert_eq!(
        model.receive(ack(8, 9).span(), |_| true).unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(model, original);
}

#[test]
fn unknown_methods_and_resource_limits_cannot_advance_to_persistence() {
    let mut model = ready();
    for id in 1..=128 {
        model.begin(id).unwrap();
    }
    assert_eq!(model.begin(129), Err(Error::Bound));
    assert_eq!(model.begin(0), Err(Error::Bound));
    assert_eq!(
        model.receive(ack(1, 4).span(), |_| true).unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(model.stage(1), Some(Stage::LoadingInventory));
    assert_eq!(model.stage(2), Some(Stage::LoadingInventory));
}

#[test]
fn notification_envelope_contains_current_references_and_no_inline_arguments() {
    let mut model = ready();
    for n in model.begin(8).unwrap() {
        let bits = n.encode().unwrap();
        let e = Envelope::decode(
            bits.span(),
            Limits {
                max_input_bits: 4096,
                max_references: 2,
                max_payload_bytes: 7,
            },
        )
        .unwrap();
        assert_eq!(e.references(), [n.endpoint.scene, 8]);
        let route = e.route(RouteProfile::ClientReceive).unwrap();
        assert_eq!(route.selector(), n.endpoint.selector);
        assert_eq!(route.serial(), Some(n.endpoint.serial));
        assert_eq!(route.method_index(), n.call.method());
        assert_eq!(route.arguments().len(), 5);
    }
}

fn loading_garage(model: &mut Lifecycle, participant: u16) {
    model.begin(participant).unwrap();
    model.set_garage([None; 5]).unwrap();
    model
        .receive(ack(participant, 3).span(), |p| p == participant)
        .unwrap();
    model.take_garage_bindings();
    model.finish_persistent(|p| p == participant);
    assert!(model.begin_progression(participant));
    assert_eq!(model.enter_garage_loading(participant).len(), 12);
    assert_eq!(model.stage(participant), Some(Stage::LoadingGarage));
}
#[test]
fn garage_replacement_is_owned_atomic_and_encodes_an_empty_slot() {
    use crate::garage::{Binding, Slots};
    let mut model = ready();
    loading_garage(&mut model, 8);
    model.enter_customization(8);
    let empty = Slots::new([None; 5]).unwrap();
    let one = Slots::new([Some(10), Some(20), None, None, None]).unwrap();
    let before = model.clone();
    assert!(model.replace_garage(empty, one, |_| false).is_err());
    assert_eq!(model, before);
    let added = model.replace_garage(empty, one, |p| p == 8).unwrap();
    assert_eq!(added.len(), 2);
    let sold = Slots::new([Some(10), None, None, None, None]).unwrap();
    let removed = model.replace_garage(one, sold, |p| p == 8).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].item, 0);
    assert_eq!(
        Binding::decode(removed[0].encode().unwrap().span()),
        Ok(removed[0])
    );
    assert!(model.replace_garage(one, sold, |_| true).is_err());
    assert!(
        model
            .replace_garage(sold, sold, |_| true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn customization_entry_follows_loading_garage_once_in_captured_order() {
    let mut model = ready();
    model.begin(9).unwrap();
    assert_eq!(model.enter_customization(9), Outcome::Repeated);
    assert_eq!(model.enter_customization(77), Outcome::Repeated);
    assert_eq!(model.stage(9), Some(Stage::LoadingInventory));
    loading_garage(&mut model, 8);
    let before = model.clone();
    assert_eq!(model.enter_customization(9), Outcome::Repeated);
    assert_eq!(model, before);
    let Outcome::Advanced(chain) = model.enter_customization(8) else {
        panic!("owned entry");
    };
    let b = bindings();
    assert_eq!(
        chain
            .iter()
            .map(|n| (n.endpoint.selector, n.call.method(), n.participant))
            .collect::<Vec<_>>(),
        [
            (b.loading_garage, 1),
            (b.composite, 0),
            (b.composite, 1),
            (b.initial_teleport, 0),
            (b.initial_teleport, 1),
            (b.customization, 0),
        ]
        .map(|(endpoint, method)| (endpoint.selector, method, 8))
    );
    assert_eq!(model.stage(8), Some(Stage::Customization));
    assert_eq!(model.in_customization(), vec![8]);
    assert!(model.waiting_garage().is_empty());
    assert_eq!(model.enter_customization(8), Outcome::Repeated);
    assert_eq!(model.stage(8), Some(Stage::Customization));
    assert_eq!(model.enter_garage_loading(8), Vec::new());
    assert_eq!(
        model.receive(ack(8, 3).span(), |_| true).unwrap(),
        Outcome::Repeated
    );
    assert_eq!(
        model.set_entry(8, Entry::default()),
        Err(Error::Unsupported)
    );
    for n in chain {
        assert!(n.encode().is_ok());
    }
}

#[test]
fn unmodeled_entry_inputs_block_without_sending_a_partial_chain() {
    for entry in [
        Entry {
            pending_videos: 1,
            pending_item_updates: false,
        },
        Entry {
            pending_videos: 0,
            pending_item_updates: true,
        },
    ] {
        let mut model = ready();
        loading_garage(&mut model, 8);
        loading_garage(&mut model, 9);
        assert_eq!(model.set_entry(0, entry), Err(Error::Bound));
        assert_eq!(model.set_entry(8192, entry), Err(Error::Bound));
        model.set_entry(8, entry).unwrap();
        assert_eq!(model.entry(8), entry);
        assert_eq!(model.entry(9), Entry::default());
        assert_eq!(model.enter_customization(8), Outcome::Unsupported);
        assert_eq!(model.stage(8), Some(Stage::EntryBlocked));
        assert_eq!(model.enter_customization(8), Outcome::Repeated);
        assert_eq!(
            model.set_entry(8, Entry::default()),
            Err(Error::Unsupported)
        );
        assert!(model.in_customization().is_empty());
        assert_eq!(model.waiting_garage(), vec![9]);
        assert!(matches!(model.enter_customization(9), Outcome::Advanced(c) if c.len() == 6));
        assert_eq!(
            model.receive(ack(8, 3).span(), |_| true).unwrap(),
            Outcome::Repeated
        );
    }
}

fn exit_bindings() -> ExitBindings {
    let e = |scene, selector| Endpoint {
        scene,
        selector,
        serial: Serial::new(0).unwrap(),
    };
    ExitBindings {
        exit_request: e(4, 96),
        garage_exit_request: e(17, 8),
        state_94: e(4, 93),
        state_4: e(4, 3),
        state_3: e(4, 2),
        state_77: e(4, 76),
        state_83: e(4, 82),
        state_78: e(4, 77),
        ready_request: e(4, 80),
        state_79: e(4, 78),
        state_2: e(4, 1),
        garage_calls: [0, 3, 4, 5, 6].map(|selector| e(17, selector)),
    }
}
fn request(scene: u16, participant: u16, selector: u16, method: u32, bits: usize) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    if bits == 0 || bits == 7 {
        payload.put(0b0110010, 7);
    } else {
        payload.put(0, bits).align();
    }
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
fn in_customization(participant: u16) -> Lifecycle {
    let mut model = ready();
    model.exit = Some(exit_bindings());
    loading_garage(&mut model, participant);
    assert!(matches!(
        model.enter_customization(participant),
        Outcome::Advanced(_)
    ));
    model
}
fn calls(outcome: Outcome) -> Vec<(u16, u16, u32, u8)> {
    let Outcome::Advanced(chain) = outcome else {
        panic!("advanced: {outcome:?}");
    };
    chain
        .iter()
        .map(|n| {
            assert!(n.encode().is_ok());
            (
                n.endpoint.scene,
                n.endpoint.selector,
                n.call.method(),
                n.call.tail(),
            )
        })
        .collect()
}

#[test]
fn three_round_trips_keep_return_order_retries_and_foreign_owners_isolated() {
    let mut model = in_customization(8);
    let ep = |selector| Endpoint {
        scene: 4,
        selector,
        serial: Serial::new(2).unwrap(),
    };
    model.returning = Some(ReturnBindings {
        request: ep(100),
        free_roam: ep(1),
        preparing: [61, 63, 65, 73].map(ep),
        sequence: ep(67),
        finishing: [68, 70, 60].map(ep),
    });
    model.stages.insert(9, Stage::Customization);
    for _ in 0..3 {
        assert!(matches!(
            model
                .receive(request(4, 8, 96, 0, 7).span(), |p| p == 8)
                .unwrap(),
            Outcome::Advanced(_)
        ));
        model.take_exited();
        model
            .receive(request(4, 8, 76, 3, 7).span(), |p| p == 8)
            .unwrap();
        model
            .receive(request(4, 8, 80, 0, 7).span(), |p| p == 8)
            .unwrap();
        model.take_arriving();
        model.arrived(8);
        let before = model.clone();
        assert!(
            model
                .receive(request(4, 8, 100, 0, 7).span(), |_| false)
                .is_err()
        );
        assert!(
            model
                .receive(request(4, 8, 100, 0, 15).span(), |_| true)
                .is_err()
        );
        assert_eq!(model, before);
        let route = request(4, 8, 100, 0, 7);
        // The first request of a zone visit only shows the prompt; the second
        // (PageDown) starts the return. Leaving the zone resets the prompt.
        assert_eq!(
            model.receive(route.span(), |p| p == 8).unwrap(),
            Outcome::Advanced(Vec::new())
        );
        assert_eq!(model.stage(8), Some(Stage::ReturnPrompted));
        assert_eq!(model.take_prompted(), [8]);
        assert!(model.take_returning().is_empty());
        assert_eq!(model.leave_prompt(8), Outcome::Advanced(Vec::new()));
        assert_eq!(model.stage(8), Some(Stage::FreeRoam));
        assert_eq!(model.leave_prompt(8), Outcome::Repeated);
        assert_eq!(model.leave_prompt(9), Outcome::Unsupported);
        assert_eq!(
            model.receive(route.span(), |p| p == 8).unwrap(),
            Outcome::Advanced(Vec::new())
        );
        assert_eq!(model.take_prompted(), [8]);
        assert!(model.in_free_roam().contains(&8));
        // The five garage slots are re-sent between the fourth preparing
        // state's enter and leave; without items each is method 2 tail 19.
        assert_eq!(
            calls(model.receive(route.span(), |p| p == 8).unwrap()),
            [
                (4, 1, 1, 0),
                (4, 61, 0, 0),
                (4, 61, 1, 0),
                (4, 63, 0, 0),
                (4, 63, 1, 31),
                (4, 65, 0, 0),
                (4, 65, 1, 31),
                (4, 73, 0, 0),
                (2, 7, 2, 19),
                (2, 8, 2, 19),
                (2, 9, 2, 19),
                (2, 10, 2, 19),
                (2, 11, 2, 19),
                (4, 73, 1, 0),
                (4, 67, 0, 0)
            ]
        );
        assert_eq!(model.take_returning(), [8]);
        assert!(model.take_prompted().is_empty());
        assert!(!model.in_free_roam().contains(&8));
        assert_eq!(
            model.receive(route.span(), |p| p == 8).unwrap(),
            Outcome::Repeated
        );
        assert_eq!(model.leave_prompt(8), Outcome::Unsupported);
        assert!(model.take_returning().is_empty());
        assert_eq!(
            calls(model.returned_to_garage(8)),
            [
                (4, 67, 1, 0),
                (4, 68, 0, 0),
                (4, 68, 1, 31),
                (4, 70, 0, 0),
                (4, 70, 1, 31),
                (4, 60, 0, 0),
                (4, 60, 1, 31),
                (4, 94, 0, 0),
                (4, 94, 1, 0),
                (4, 89, 0, 0)
            ]
        );
        assert_eq!(model.returned_to_garage(8), Outcome::Repeated);
        assert!(matches!(model.enter_customization(8), Outcome::Advanced(_)));
        assert_eq!(model.stage(9), Some(Stage::Customization));
    }
}

#[test]
fn garage_exit_and_world_entry_follow_the_official_order() {
    let mut model = in_customization(8);
    let owns = |p| p == 8;
    assert_eq!(
        model.receive(request(17, 8, 8, 0, 7).span(), owns).unwrap(),
        Outcome::Repeated
    );
    assert_eq!(model.stage(8), Some(Stage::Customization));
    assert_eq!(
        calls(model.receive(request(4, 8, 96, 0, 7).span(), owns).unwrap()),
        [
            (4, 86, 1, 0),
            (4, 93, 0, 0),
            (4, 93, 1, 0),
            (17, 0, 2, 0),
            (17, 3, 2, 0),
            (17, 4, 2, 0),
            (17, 5, 2, 0),
            (17, 6, 2, 0),
            (4, 3, 0, 0),
            (4, 3, 1, 31),
            (4, 2, 0, 0),
            (4, 2, 1, 0),
            (4, 76, 0, 0),
        ]
    );
    assert_eq!(model.stage(8), Some(Stage::ExitingGarage));
    assert_eq!(model.take_exited(), vec![8]);
    assert!(model.take_exited().is_empty());
    for repeat in [request(4, 8, 96, 0, 7), request(17, 8, 8, 0, 7)] {
        assert_eq!(
            model.receive(repeat.span(), owns).unwrap(),
            Outcome::Repeated
        );
    }
    assert!(model.take_exited().is_empty());
    assert_eq!(
        calls(model.receive(request(4, 8, 76, 3, 7).span(), owns).unwrap()),
        [(4, 76, 1, 0), (4, 82, 0, 0), (4, 82, 1, 0), (4, 77, 0, 0)]
    );
    assert_eq!(model.stage(8), Some(Stage::EnteringWorld));
    // World ready enters the arrive state; it is left only when the arrive
    // sequence completes (E101, E742).
    assert_eq!(
        calls(model.receive(request(4, 8, 80, 0, 7).span(), owns).unwrap()),
        [(4, 77, 1, 0), (4, 78, 0, 0)]
    );
    assert_eq!(model.stage(8), Some(Stage::Arriving));
    assert_eq!(model.take_arriving(), vec![8]);
    assert!(model.take_arriving().is_empty());
    assert!(model.in_free_roam().is_empty());
    assert_eq!(
        model.receive(request(4, 8, 80, 0, 7).span(), owns).unwrap(),
        Outcome::Repeated
    );
    assert_eq!(model.arrived(9), Outcome::Unsupported);
    assert_eq!(calls(model.arrived(8)), [(4, 78, 1, 0), (4, 1, 0, 0)]);
    assert_eq!(model.arrived(8), Outcome::Repeated);
    assert_eq!(model.stage(8), Some(Stage::FreeRoam));
    assert_eq!(model.in_free_roam(), vec![8]);
    for repeat in [
        request(4, 8, 96, 0, 7),
        request(4, 8, 76, 3, 7),
        request(4, 8, 80, 0, 7),
    ] {
        assert_eq!(
            model.receive(repeat.span(), owns).unwrap(),
            Outcome::Repeated
        );
    }
    assert_eq!(
        model.receive(ack(8, 3).span(), owns).unwrap(),
        Outcome::Repeated
    );
}

#[test]
fn relocation_reserves_only_free_roam_and_is_isolated_and_repeat_safe() {
    let mut model = in_customization(8);
    let before = model.clone();
    assert_eq!(model.begin_relocation(8), Err(Error::Shape));
    assert_eq!(model.begin_relocation(9), Err(Error::UnknownObject));
    assert_eq!(model, before);
    for (selector, method) in [(96, 0), (76, 3), (80, 0)] {
        model
            .receive(request(4, 8, selector, method, 7).span(), |p| p == 8)
            .unwrap();
    }
    model.take_arriving();
    model.arrived(8);
    model.begin_relocation(8).unwrap();
    assert!(model.in_free_roam().is_empty());
    let reserved = model.clone();
    assert_eq!(model.begin_relocation(8), Err(Error::Shape));
    assert_eq!(model.finish_relocation(9), Err(Error::UnknownObject));
    assert_eq!(model, reserved);
    model.finish_relocation(8).unwrap();
    assert_eq!(model.in_free_roam(), vec![8]);
    assert_eq!(model.finish_relocation(8), Err(Error::Shape));
}

#[test]
fn exit_requests_out_of_order_foreign_or_malformed_change_nothing() {
    let model = in_customization(8);
    for early in [request(4, 8, 76, 3, 7), request(4, 8, 80, 0, 7)] {
        let mut m = model.clone();
        assert_eq!(
            m.receive(early.span(), |p| p == 8).unwrap(),
            Outcome::Unsupported
        );
        assert_eq!(m, model);
    }
    let mut m = model.clone();
    assert_eq!(
        m.receive(request(4, 8, 96, 0, 7).span(), |_| false),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        m.receive(request(4, 9, 96, 0, 7).span(), |_| true),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        m.receive(request(4, 8, 96, 0, 15).span(), |_| true),
        Err(Error::Shape)
    );
    assert_eq!(m, model);
    // A participant still loading cannot exit; the exit route answers nothing.
    let mut loading = ready();
    loading.exit = Some(exit_bindings());
    loading_garage(&mut loading, 9);
    let before = loading.clone();
    assert_eq!(
        loading
            .receive(request(4, 9, 96, 0, 7).span(), |p| p == 9)
            .unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(loading, before);
    // Without exit bindings the request is not an exit route.
    let mut unbound = ready();
    loading_garage(&mut unbound, 8);
    unbound.enter_customization(8);
    assert_eq!(
        unbound
            .receive(request(4, 8, 96, 0, 7).span(), |p| p == 8)
            .unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(unbound.stage(8), Some(Stage::Customization));
}

#[test]
fn tagged_leave_carries_its_tail_and_plain_calls_keep_zero_padding() {
    let e = Endpoint {
        scene: 4,
        selector: 3,
        serial: Serial::new(44).unwrap(),
    };
    for (call, tail) in [
        (Call::LeaveTagged(31), 31),
        (Call::Leave, 0),
        (Call::Enter, 0),
        (Call::Notify, 0),
    ] {
        let bits = Notification {
            endpoint: e,
            participant: 8,
            call,
        }
        .encode()
        .unwrap();
        let envelope = Envelope::decode(
            bits.span(),
            Limits {
                max_input_bits: 4096,
                max_references: 2,
                max_payload_bytes: 7,
            },
        )
        .unwrap();
        let route = envelope.route(RouteProfile::ClientReceive).unwrap();
        assert_eq!(route.method_index(), call.method());
        assert_eq!(route.arguments().len(), 5);
        assert_eq!(route.arguments().read_u32(0, 5).unwrap(), tail);
    }
    assert_eq!(
        Notification {
            endpoint: e,
            participant: 8,
            call: Call::LeaveTagged(32),
        }
        .encode()
        .err(),
        Some(Error::Bound)
    );
    assert_eq!(ExitBindings::from_records(&[], 1, 2), Ok(None));
    assert_eq!(ExitBindings::from_records(&[], 1, 1), Err(Error::Shape));
}

#[test]
fn arrival_requests_carry_the_prompt_into_free_roam_across_three_lifetimes() {
    // Preserve a pre-arrival request until confirmation or zone release.
    // Only a repeated request in free roam starts the return.
    let mut model = in_customization(8);
    let ep = |selector| Endpoint {
        scene: 4,
        selector,
        serial: Serial::new(2).unwrap(),
    };
    model.returning = Some(ReturnBindings {
        request: ep(100),
        free_roam: ep(1),
        preparing: [61, 63, 65, 73].map(ep),
        sequence: ep(67),
        finishing: [68, 70, 60].map(ep),
    });
    model.stages.insert(9, Stage::Customization);
    let owns = |p| p == 8 || p == 9;
    let route = request(4, 8, 100, 0, 7);
    let other = request(4, 9, 100, 0, 7);
    let prompt_only = Outcome::Advanced(Vec::new());
    let exit_to_arriving = |model: &mut Lifecycle, request_while_exiting: bool| {
        assert!(matches!(
            model.receive(request(4, 8, 96, 0, 7).span(), owns).unwrap(),
            Outcome::Advanced(_)
        ));
        model.take_exited();
        assert_eq!(model.stage(8), Some(Stage::ExitingGarage));
        if request_while_exiting {
            // A request before the world-loaded report cannot return yet.
            assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
            assert_eq!(model.take_prompted(), [8]);
            assert!(model.prompted(8));
            assert_eq!(model.stage(8), Some(Stage::ExitingGarage));
        }
        model.receive(request(4, 8, 76, 3, 7).span(), owns).unwrap();
        model.receive(request(4, 8, 80, 0, 7).span(), owns).unwrap();
        model.take_arriving();
        assert_eq!(model.stage(8), Some(Stage::Arriving));
    };
    let return_chain = |model: &mut Lifecycle| {
        assert_eq!(calls(model.receive(route.span(), owns).unwrap()).len(), 15);
        assert_eq!(model.take_returning(), [8]);
        assert!(model.take_prompted().is_empty());
        assert!(!model.prompted(8));
        assert_eq!(model.leave_prompt(8), Outcome::Unsupported);
        assert!(matches!(model.returned_to_garage(8), Outcome::Advanced(_)));
        assert!(matches!(model.enter_customization(8), Outcome::Advanced(_)));
    };
    // Lifetime 1: request while exiting, a repeat during the
    // arrival is the same prompt, the rollout release ends it, the re-entry
    // request starts it again, free roam begins prompted and PageDown
    // returns.
    exit_to_arriving(&mut model, true);
    assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
    assert!(model.take_prompted().is_empty());
    assert!(model.take_returning().is_empty());
    assert_eq!(model.leave_prompt(8), prompt_only);
    assert!(!model.prompted(8));
    assert_eq!(model.leave_prompt(8), Outcome::Unsupported);
    assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
    assert_eq!(model.take_prompted(), [8]);
    assert_eq!(calls(model.arrived(8)).len(), 2);
    assert_eq!(model.stage(8), Some(Stage::ReturnPrompted));
    assert_eq!(model.arrived(8), Outcome::Repeated);
    assert!(model.prompted(8));
    assert!(model.in_free_roam().contains(&8));
    // A prompt that began during the arrival never confirms by itself: the
    // repeat in free roam does.
    return_chain(&mut model);
    // Lifetime 2: the prompt carried into free roam is cancelled by driving
    // away; the next visit prompts again and returns on the repeat.
    exit_to_arriving(&mut model, false);
    assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
    assert_eq!(model.take_prompted(), [8]);
    assert_eq!(calls(model.arrived(8)).len(), 2);
    assert_eq!(model.stage(8), Some(Stage::ReturnPrompted));
    assert_eq!(model.leave_prompt(8), prompt_only);
    assert_eq!(model.stage(8), Some(Stage::FreeRoam));
    assert!(!model.prompted(8));
    assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
    assert_eq!(model.take_prompted(), [8]);
    return_chain(&mut model);
    // Lifetime 3: no arrival request; the other participant's request is
    // unsupported in the garage and leaves the prompt of 8 alone.
    exit_to_arriving(&mut model, false);
    assert!(!model.prompted(8));
    assert_eq!(calls(model.arrived(8)).len(), 2);
    assert_eq!(model.stage(8), Some(Stage::FreeRoam));
    assert_eq!(model.receive(route.span(), owns).unwrap(), prompt_only);
    assert_eq!(model.take_prompted(), [8]);
    assert_eq!(
        model.receive(other.span(), owns).unwrap(),
        Outcome::Unsupported
    );
    assert_eq!(model.stage(9), Some(Stage::Customization));
    assert!(!model.prompted(9));
    assert_eq!(model.leave_prompt(9), Outcome::Unsupported);
    assert!(model.prompted(8));
    assert!(model.take_prompted().is_empty());
    return_chain(&mut model);
    assert_eq!(model.stage(9), Some(Stage::Customization));
}

#[test]
fn the_return_rebinds_the_car_slot_with_its_item_and_tags_the_empty_slots() {
    // The occupied slot carries a two-word item; empty slots carry tail 19.
    let mut model = ready();
    model.exit = Some(exit_bindings());
    model.begin(8).unwrap();
    model
        .set_garage([Some(0x1234_5678_9abc_def0), None, None, None, None])
        .unwrap();
    model.receive(ack(8, 3).span(), |p| p == 8).unwrap();
    let login = model.take_garage_bindings();
    assert_eq!(login.len(), 1);
    model.finish_persistent(|p| p == 8);
    assert!(model.begin_progression(8));
    assert_eq!(model.enter_garage_loading(8).len(), 12);
    assert!(matches!(model.enter_customization(8), Outcome::Advanced(_)));
    let ep = |selector| Endpoint {
        scene: 4,
        selector,
        serial: Serial::new(2).unwrap(),
    };
    model.returning = Some(ReturnBindings {
        request: ep(100),
        free_roam: ep(1),
        preparing: [61, 63, 65, 73].map(ep),
        sequence: ep(67),
        finishing: [68, 70, 60].map(ep),
    });
    let owns = |p| p == 8;
    for (endpoint, method) in [(96u16, 0u32), (76, 3), (80, 0)] {
        model
            .receive(request(4, 8, endpoint, method, 7).span(), owns)
            .unwrap();
    }
    model.take_exited();
    model.take_arriving();
    assert!(matches!(model.arrived(8), Outcome::Advanced(_)));
    let route = request(4, 8, 100, 0, 7);
    assert_eq!(
        model.receive(route.span(), owns).unwrap(),
        Outcome::Advanced(Vec::new())
    );
    let Outcome::Advanced(chain) = model.receive(route.span(), owns).unwrap() else {
        panic!("return chain expected");
    };
    assert_eq!(chain.len(), 15);
    let slot = chain[8];
    assert_eq!(slot.call, Call::GarageItem(0x1234_5678_9abc_def0));
    assert_eq!((slot.endpoint.scene, slot.endpoint.selector), (2, 7));
    // Same wire form as the login binding: the garage codec decodes it.
    let decoded = crate::garage::Binding::decode(slot.encode().unwrap().span()).unwrap();
    assert_eq!(decoded.item, 0x1234_5678_9abc_def0);
    assert_eq!(decoded.participant, 8);
    assert_eq!(decoded.endpoint, slot.endpoint);
    assert_eq!(decoded, login[0]);
    for (i, n) in chain[9..13].iter().enumerate() {
        assert_eq!(n.call, Call::NotifyTagged(19));
        assert_eq!(n.endpoint.selector, 8 + i as u16);
        assert_eq!(n.encode().unwrap().len(), slot.encode().unwrap().len() - 64);
    }
    // A zero item has no wire form.
    assert!(
        Notification {
            endpoint: ep(7),
            participant: 8,
            call: Call::GarageItem(0),
        }
        .encode()
        .is_err()
    );
}
