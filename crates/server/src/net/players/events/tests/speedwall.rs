// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn finished() -> Harness {
    let mut h = started();
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
