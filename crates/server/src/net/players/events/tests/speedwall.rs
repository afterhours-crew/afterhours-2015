// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn finished() -> Harness {
    finish(started())
}

pub(super) fn finish(mut h: Harness) -> Harness {
    h.run(6000, vec![h.checkpoint(6, 30.)]);
    h.run(7000, vec![h.checkpoint(7, 60.)]);
    let batch = h
        .listener
        .scoring_state
        .pending_awards
        .back()
        .unwrap()
        .batch_id;
    let event = h.content.record_id();
    let race = h
        .listener
        .event_state
        .entries
        .get_mut(&h.participant)
        .unwrap()
        .race
        .as_mut()
        .unwrap();
    race.speedwall = Some(nfs_world::speedwall::Progress {
        score_bits: 0,
        time_ms_bits: (race.finish.unwrap().race_ms as f32).to_bits(),
        rep_bits: 9000f32.to_bits(),
        position: race.finish.unwrap().position as u32,
        event_id: event,
        vehicle: b"ID_CAR_TEST".to_vec(),
    });
    assert_eq!(race.credit_batch, Some(batch));
    let outro = race.sequence.unwrap().1;
    h.run(8000, vec![logic(OUTRO_READY, outro)]);
    h.run(8100, vec![h.completed(outro)]);
    h
}

#[test]
fn result_handoff_waits_for_its_own_committed_credit_then_publishes_once() {
    let mut h = finished();
    let frozen = h.race().speedwall.clone().unwrap();
    let gm = h.race().game_mode;
    let batch = h.race().credit_batch.unwrap();
    let delayed = h.run(8300, vec![]);
    assert!(!delayed.calls().contains(&(gm, 0, 1, None)));
    assert!(h.listener.pending_speedwalls.is_empty());
    assert!(!h.race().awaiting_leave);
    h.listener.event_credit_committed(h.participant, batch + 1);
    h.listener.event_credit_committed(h.participant + 1, batch);
    assert!(!h.race().credit_committed);
    h.listener.event_credit_committed(h.participant, batch);
    let ending = h.run(8500, vec![]);
    assert!(ending.calls().contains(&(gm, 0, 1, None)));
    assert_eq!(h.listener.pending_speedwalls.pop_front(), Some(frozen));
    assert!(h.race().speedwall.is_none());
    h.run(8600, vec![h.leave()]);
    h.listener.event_credit_committed(h.participant, batch);
    h.run(8700, vec![]);
    assert!(h.listener.pending_speedwalls.is_empty());
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
}

#[test]
fn full_output_queue_rolls_back_progress_and_leave_together() {
    let mut h = finished();
    let batch = h.race().credit_batch.unwrap();
    h.listener.event_credit_committed(h.participant, batch);
    let frozen = h.race().speedwall.clone();
    let dummy = HostRpc::GameMode(
        h.content
            .state_call(
                h.world(),
                h.race().game_mode,
                Gm::Root,
                2,
                None,
                Arguments::None,
            )
            .unwrap(),
    );
    h.listener.pending_rpcs =
        std::iter::repeat_n(dummy, nfs_world::session::MAX_RPC_QUEUE).collect();
    h.run(8300, vec![]);
    assert_eq!(h.race().speedwall, frozen);
    assert!(!h.race().awaiting_leave);
    assert!(h.listener.pending_speedwalls.is_empty());
    h.run(8400, vec![]);
    assert_eq!(h.listener.pending_speedwalls.len(), 1);
    let leave = h.leave();
    h.run(8500, vec![leave]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_some(),
        "later leave waits for the ordered handoff to be queued"
    );
    h.listener.pending_speedwalls.pop_front();
    h.run(8600, vec![]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
}

/// A listener whose owned car has installed vehicle information, as the
/// configured runtime resolves the raced car for Speedwall progress.
fn raced_listener() -> (PlayerListener, u16) {
    use crate::vehicle_content::{
        GarageContent,
        population::{Inventory, Owner, tests as population_tests},
    };
    use nfs_world::replication::players::Request;
    let (mut population, mut players, _, layout, items, _) = population_tests::fixture();
    let (mut document, classes, _) = crate::vehicle_content::tests::fixture();
    document["vehicles"][0]["vehicle_info"] = serde_json::json!({"bundle": 0xf123_4567u32, "short_name": "ID_CAR_CONSTRUCTED_SHORT_NAME"});
    let vehicles = crate::vehicle_content::Content::from_json(&document, &classes).unwrap();
    let persona = fixture::PERSONA;
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"Driver".to_vec(),
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
    let inventory = Inventory {
        slots: nfs_world::garage::Slots::new([Some(1), None, None, None, None]).unwrap(),
        items,
    };
    let spawned = population
        .spawn(
            &mut players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            },
            inventory.clone(),
            &vehicles,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let car = spawned.bindings[0].vehicle;
    let mut listener = PlayerListener::new(persona);
    listener.players = players;
    listener.inventory = Some(inventory);
    listener.vehicles = Some(Arc::new(GarageContent { vehicles, layout }));
    (listener, car)
}

/// E1280: the client's speed-wall store is keyed by the standard event of the
/// installed NarrativeToStandardEventMap, the ID it requests through 2050/20.
/// After the narrative "In The Driver's Seat" (`sprint_race_007_nar`, record
/// 3196290035) the official host sent progress for 1761118161; progress under
/// the narrative ID is a null-wall dereference in the client (RVA 0x4428b75).
#[test]
fn narrative_progress_is_keyed_by_the_standard_event_the_client_holds() {
    use nfs_content::events::recommendations::NarrativeEventMap;
    let mut event = fixture::event();
    event.game_mode_id = "gameplay/gamemodes/sprint_race_007_nar".into();
    let content = event_logic::Content::new(event, "sprint".into()).unwrap();
    assert_eq!(content.record_id(), 3_196_290_035);
    assert_eq!(
        content.speedwall_event(),
        3_196_290_035,
        "unmapped until prepared"
    );
    let map = NarrativeEventMap::from_pairs([(3_196_290_035, 1_761_118_161)]).unwrap();
    let content = content.with_speedwall_event(&map);
    assert_eq!(
        content.record_id(),
        3_196_290_035,
        "statistics keep the record ID"
    );
    let (listener, car) = raced_listener();
    let properties = std::collections::BTreeMap::new();
    let results = |position| event_logic::Results {
        finish: Finish {
            race_ms: 147_062,
            position,
            participants: 4,
        },
        properties: &properties,
        awards: None,
    };
    let progress = listener
        .event_speedwall(&content, &listener.players, car, results(1))
        .unwrap()
        .unwrap();
    assert_eq!(progress.event_id, 1_761_118_161);
    assert_eq!(progress.position, 1);
    assert_eq!(f32::from_bits(progress.time_ms_bits), 147_062.);
    assert_eq!(progress.vehicle, b"ID_CAR_CONSTRUCTED_SHORT_NAME".to_vec());
    progress.encode().unwrap();
    // The standard event and any event outside the map keep their record ID;
    // the native lookup converts once and never chains.
    let standard = fixture::content().with_speedwall_event(&map);
    assert_eq!(standard.record_id(), 1_761_118_161);
    assert_eq!(standard.speedwall_event(), 1_761_118_161);
    let chained = NarrativeEventMap::from_pairs([(1_761_118_161, 7)]).unwrap();
    assert_eq!(standard.with_speedwall_event(&chained).speedwall_event(), 7);
    assert_eq!(
        content.with_speedwall_event(&chained).speedwall_event(),
        3_196_290_035,
        "re-resolution starts from the record ID"
    );
    let unrelated =
        fixture::content().with_speedwall_event(&NarrativeEventMap::from_pairs([(5, 6)]).unwrap());
    assert_eq!(unrelated.speedwall_event(), 1_761_118_161);
    let unconfigured = PlayerListener::new(fixture::PERSONA);
    assert_eq!(
        unconfigured
            .event_speedwall(&unrelated, &unconfigured.players, car, results(2))
            .unwrap(),
        None,
        "reference harnesses without inventory send no progress"
    );
}
