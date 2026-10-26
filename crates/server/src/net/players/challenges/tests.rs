// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_storage::{AccountId, Batch, InventoryRepository, MemoryRepository, Timestamp};
use nfs_world::bits::BitWriter;

fn catalog() -> Arc<Catalog> {
    let entries: Vec<_> = (1..=10)
        .map(|id| {
            serde_json::json!({
                "id":id,"car_id":2,"count_one":3,"count_two":4,"day":"local",
                "event_id":5,"kind":6,"weekly":false,"type_override_id":0
            })
        })
        .collect();
    Arc::new(
        Catalog::from_json(&serde_json::json!({
            "version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
            "day_id":7,"monthly_start":-11,"awards":[],"ranks":[],"challenges":entries
        }))
        .unwrap(),
    )
}

fn request(persona: u64, ids: Vec<u32>) -> frame::ChallengePlayerSync {
    frame::ChallengePlayerSync {
        persona,
        monthly_start: (-11i64) as u64,
        day_id: 7,
        challenge_ids: ids,
    }
}

fn wire(sequence: u8, request: frame::ChallengePlayerSync, player: Option<bool>) -> BitWriter {
    let mut messages = vec![frame::Message::ChallengePlayerSync(request)];
    if let Some(flag) = player {
        messages.push(frame::Message::CreatePlayer {
            name: b"local".to_vec(),
            flag,
            slot: 0,
        });
    }
    frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages,
            }],
        )
        .build()
        .unwrap()
}

fn receive(listener: &mut PlayerListener, data: &BitWriter) -> bool {
    listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: data.span(),
    })
}

fn current(repo: &MemoryRepository, account: u8, count: u32) -> Current {
    let who = AccountId::from_owned_config([account; 16]).unwrap();
    let initial =
        nfs_services::challenges::ensure_empty(repo, who, repo.open(who).unwrap(), Timestamp(1))
            .unwrap();
    let state = nfs_services::challenges::State {
        progress: BTreeMap::from([(
            (7, 1),
            nfs_services::challenges::Progress {
                complete: false,
                count_one: count,
                count_two: 0,
            },
        )]),
        ..Default::default()
    };
    repo.apply(
        who,
        &Batch {
            id: 99,
            expected_generation: initial.generation,
            ops: state.operations().unwrap(),
        },
        Timestamp(2),
    )
    .unwrap();
    Current::from_snapshot(&repo.snapshot(who).unwrap()).unwrap()
}

#[test]
fn challenge_reads_commit_only_with_the_entire_frame_and_retry_correctly() {
    let mut listener = PlayerListener::new(7).with_challenges(Some(catalog()));
    let good = wire(0, request(7, vec![1]), Some(false));
    let mut trailing = good.clone();
    trailing.put(255, 8);
    assert!(!receive(&mut listener, &trailing));
    assert!(!listener.has_challenge_request());
    assert!(!receive(
        &mut listener,
        &wire(0, request(7, vec![1]), Some(true))
    ));
    assert!(!listener.has_challenge_request());
    assert!(listener.players.objects().is_empty());
    assert!(receive(&mut listener, &good));
    assert_eq!(listener.challenges.pending.len(), 1);
    assert!(receive(&mut listener, &good));
    assert_eq!(listener.challenges.pending.len(), 1);
    assert_eq!(listener.unsupported.total(), 0);
    assert!(listener.observation(0, 0, 0, false).items_working);
    super::super::test_wire::flush(&mut listener, &mut super::super::test_wire::session_host())
        .unwrap();
}

#[test]
fn identity_period_unknown_ids_and_queue_bounds_do_not_advance_receive_history() {
    let mut listener = PlayerListener::new(7).with_challenges(Some(catalog()));
    for bad in [
        request(8, vec![1]),
        frame::ChallengePlayerSync {
            day_id: 8,
            ..request(7, vec![1])
        },
        frame::ChallengePlayerSync {
            monthly_start: 0,
            ..request(7, vec![1])
        },
        request(7, vec![11]),
        request(7, vec![1, 1]),
    ] {
        assert!(!receive(&mut listener, &wire(0, bad, None)));
        assert!(!listener.has_challenge_request());
    }
    for sequence in 0..MAX_READS as u8 {
        assert!(receive(
            &mut listener,
            &wire(sequence, request(7, vec![u32::from(sequence) + 1]), None)
        ));
    }
    assert_eq!(listener.challenges.pending.len(), MAX_READS);
    assert!(!receive(&mut listener, &wire(8, request(7, vec![9]), None)));
    let repo = MemoryRepository::default();
    listener.challenges_loaded(current(&repo, 1, 2)).unwrap();
    assert!(receive(&mut listener, &wire(8, request(7, vec![9]), None)));
    assert_eq!(listener.challenges.pending.len(), MAX_READS);
}

#[test]
fn cache_reads_are_bounded_isolated_replace_repeats_and_preserve_other_subsets() {
    let repo = MemoryRepository::default();
    let a = current(&repo, 1, 2);
    let b = current(&repo, 2, 1);
    let before_a = repo
        .snapshot(AccountId::from_owned_config([1; 16]).unwrap())
        .unwrap();
    let mut first = PlayerListener::new(7).with_challenges(Some(catalog()));
    let mut second = PlayerListener::new(8).with_challenges(Some(catalog()));
    for (listener, persona, snapshot) in [(&mut first, 7, &a), (&mut second, 8, &b)] {
        assert!(receive(listener, &wire(0, request(persona, vec![1]), None)));
        listener.challenges.loaded(snapshot).unwrap();
        assert!(!listener.has_challenge_request());
        assert!(listener.pending_content.is_empty());
        assert!(listener.pending_rpcs.is_empty());
    }
    assert_eq!(first.challenges.entries[&1].progress.count_one, 2);
    assert_eq!(second.challenges.entries[&1].progress.count_one, 1);
    for sequence in 1..20 {
        assert!(receive(
            &mut first,
            &wire(sequence, request(7, vec![2]), None)
        ));
        first.challenges.loaded(&a).unwrap();
        assert_eq!(first.challenges.entries.len(), 2);
    }
    assert_eq!(first.challenges.entries[&1].progress.count_one, 2);
    assert_eq!(
        repo.snapshot(AccountId::from_owned_config([1; 16]).unwrap())
            .unwrap(),
        before_a
    );
    assert!(PlayerListener::new(7).challenges.entries.is_empty());
}
