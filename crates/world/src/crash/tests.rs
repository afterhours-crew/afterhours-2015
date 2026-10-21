// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::replication::{Initial, entity::creation::Catalog, players::Request, sublevel};

fn setup() -> (Players, Crashes, Owner) {
    let mut players = Players::default();
    let scenes = sublevel::Content::new(vec![(
        701,
        sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::Rpc,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    let scene = sublevel::passive::creation(1, 701, Serial::new(8).unwrap(), &scenes).unwrap();
    players.create_scenes(&[1], vec![scene], &scenes).unwrap();
    let asset = Asset {
        bundle: 9,
        type_id: 77,
        local_index: 3,
    };
    let content = Content::new(
        vec![Catalog::new(9, &[(77, 6)]).unwrap()],
        vec![(asset, profile())],
    )
    .unwrap();
    let crashes = Crashes::new(
        Definition {
            asset,
            manager: Component {
                scene_key: 701,
                index: 2,
            },
            embedded_bus: 5,
        },
        content,
        4_500,
    )
    .unwrap();
    let owner = join(&mut players, 4, 400);
    (players, crashes, owner)
}
fn join(players: &mut Players, connection: u8, persona: u64) -> Owner {
    let player = players
        .create(
            connection,
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
    Owner {
        connection,
        persona,
        participant: players
            .join(connection, persona, player)
            .unwrap()
            .unwrap()
            .id,
    }
}
fn completion(owner: Owner, sequence: u16) -> Completed {
    Completed {
        sequence,
        participant: owner.participant,
    }
}

#[test]
fn short_pulse_survives_clear_and_backpressure_then_completes_without_stop() {
    let (mut players, mut crashes, owner) = setup();
    crashes.poll(100, 0).unwrap();
    crashes.observe(&players, owner, 10, true, true).unwrap();
    crashes.observe(&players, owner, 11, false, true).unwrap();
    crashes.poll(5_000, 0).unwrap();
    assert_eq!(crashes.pending(), vec![owner]);
    let record = crashes.start(&mut players, owner).unwrap().unwrap();
    assert!(crashes.start(&mut players, owner).unwrap().is_none());
    assert!(crashes.poll(8_500, 1).unwrap().is_empty());
    assert_eq!(
        crashes
            .complete(&mut players, owner, completion(owner, record.id))
            .unwrap(),
        Some(record.id)
    );
    assert!(players.objects().get(record.id).is_none());
    assert_eq!(
        crashes
            .complete(&mut players, owner, completion(owner, record.id))
            .unwrap(),
        None
    );
    assert!(crashes.poll(20_000, 1).unwrap().is_empty());
}

#[test]
fn held_signal_cannot_repeat_and_stale_clear_cannot_rearm() {
    let (mut players, mut crashes, owner) = setup();
    crashes.observe(&players, owner, 10, true, true).unwrap();
    let first = crashes.start(&mut players, owner).unwrap().unwrap();
    crashes
        .complete(&mut players, owner, completion(owner, first.id))
        .unwrap();
    crashes.observe(&players, owner, 9, false, true).unwrap();
    crashes.observe(&players, owner, 11, true, true).unwrap();
    assert!(crashes.pending().is_empty());
    crashes.observe(&players, owner, 12, false, true).unwrap();
    crashes.observe(&players, owner, 13, true, true).unwrap();
    let second = crashes.start(&mut players, owner).unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert!(!crashes.has_completion(first.id, 0));
    assert!(
        crashes
            .complete(&mut players, owner, completion(owner, first.id))
            .is_err()
    );
    assert!(players.objects().get(second.id).is_some());
    let Some(Initial::Entity { fields, .. }) = second.initial else {
        panic!("sequence creation")
    };
    assert!(matches!(
        &fields[0],
        entity::Initial::SequenceRoot { sequence: 2, .. }
    ));
}

#[test]
fn timer_preserves_due_stop_until_capacity_and_only_sends_once() {
    let (mut players, mut crashes, owner) = setup();
    crashes.observe(&players, owner, 1, true, true).unwrap();
    let record = crashes.start(&mut players, owner).unwrap().unwrap();
    assert!(crashes.poll(4_499, 1).unwrap().is_empty());
    assert!(crashes.poll(4_500, 0).unwrap().is_empty());
    assert_eq!(crashes.poll(4_501, 1).unwrap()[0].sequence, record.id);
    assert!(crashes.poll(4_502, 1).unwrap().is_empty());
    assert!(crashes.poll(4_000, 1).is_err());
    assert!(crashes.retire(owner.participant).is_err());
    crashes
        .complete(&mut players, owner, completion(owner, record.id))
        .unwrap();
    crashes.retire(owner.participant).unwrap();
    assert!(!crashes.has_completion(record.id, 0));
}

#[test]
fn independent_owners_cannot_complete_each_others_sequences() {
    let (mut players, mut crashes, a) = setup();
    let b = join(&mut players, 5, 500);
    for owner in [a, b] {
        crashes.observe(&players, owner, 1, true, true).unwrap();
    }
    let first = crashes.start(&mut players, a).unwrap().unwrap();
    let second = crashes.start(&mut players, b).unwrap().unwrap();
    assert!(
        crashes
            .complete(&mut players, b, completion(a, first.id))
            .is_err()
    );
    assert!(
        crashes
            .observe(&players, Owner { persona: 500, ..a }, 2, false, true)
            .is_err()
    );
    crashes
        .complete(&mut players, a, completion(a, first.id))
        .unwrap();
    assert!(players.objects().get(second.id).is_some());
    assert!(crashes.active(b.participant));
    assert!(!crashes.active(a.participant));
}

#[test]
fn ineligible_reports_do_not_defer_a_crash_until_garage_exit() {
    let (players, mut crashes, owner) = setup();
    crashes.observe(&players, owner, 1, true, false).unwrap();
    crashes.observe(&players, owner, 2, true, true).unwrap();
    assert!(crashes.pending().is_empty());
    crashes.observe(&players, owner, 3, false, true).unwrap();
    crashes.observe(&players, owner, 4, true, true).unwrap();
    assert_eq!(crashes.pending(), vec![owner]);
}

#[test]
fn garage_retirement_discards_old_revision_and_a_fresh_session_has_no_pending_crash() {
    let (mut players, mut crashes, owner) = setup();
    crashes.observe(&players, owner, 100, true, true).unwrap();
    crashes.discard_pending(owner.participant);
    assert!(crashes.pending().is_empty());
    crashes.retire(owner.participant).unwrap();
    // A replacement car begins its own revision lifetime.
    crashes.observe(&players, owner, 1, true, true).unwrap();
    assert!(crashes.start(&mut players, owner).unwrap().is_some());
    drop((players, crashes));
    let (players, mut fresh, owner) = setup();
    assert!(fresh.pending().is_empty());
    assert!(fresh.poll(30_000, 128).unwrap().is_empty());
    fresh.observe(&players, owner, 1, false, true).unwrap();
    assert!(!fresh.active(owner.participant));
}

#[test]
fn full_participant_capacity_and_timer_bounds_preserve_existing_state() {
    let (mut players, mut crashes, first) = setup();
    for (duration, valid) in [
        (0, false),
        (1, true),
        (MAX_DURATION_MS, true),
        (MAX_DURATION_MS + 1, false),
    ] {
        assert_eq!(
            Crashes::new(crashes.definition, crashes.content.clone(), duration).is_ok(),
            valid
        );
    }
    crashes.observe(&players, first, 1, false, true).unwrap();
    for connection in (1..=MAX_PARTICIPANTS as u8).filter(|c| *c != first.connection) {
        let owner = join(&mut players, connection, u64::from(connection) + 1000);
        crashes.observe(&players, owner, 1, false, true).unwrap();
    }
    crashes.observe(&players, first, 2, true, true).unwrap();
    assert_eq!(crashes.pending(), vec![first]);
}
