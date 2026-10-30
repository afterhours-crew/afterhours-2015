// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    progression::{Catalog, Settings},
    story::{Policy, Prepared},
};
use nfs_storage::{Applied, Error};
use std::sync::atomic::{AtomicBool, Ordering};

fn catalog() -> Catalog {
    Catalog::from_json(&serde_json::json!({
        "version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,
        "source":{"path":"constructed","sha256":"00".repeat(32)},"branches":[
        {"name":"intro","objectives":[{"id":21,"active":{"constant":true}}],"ints":[]},
        {"name":"main","objectives":[
            {"id":30,"active":{"completed":21}},
            {"id":31,"active":{"completed":30}},
            {"id":32,"active":{"completed":31}},
            {"id":40,"active":{"completed":21}}
        ],"ints":[]}
    ]}))
    .unwrap()
}
fn policy(catalog: &Catalog) -> Policy<'_> {
    Policy {
        catalog,
        settings: Settings {
            consumer_beta: false,
        },
    }
}
fn finish(repo: &dyn InventoryRepository, who: AccountId, p: &Policy<'_>, id: u64) {
    let before = repo.snapshot(who).unwrap();
    let Prepared::Mutation(batch) = p
        .complete(
            &before,
            before.generation,
            p.next_operation(&before).unwrap(),
            id,
            vec![],
        )
        .unwrap()
    else {
        panic!("new completion")
    };
    repo.apply(who, &batch, Timestamp(4)).unwrap();
}

struct RefusesOnce<'a> {
    inner: &'a dyn InventoryRepository,
    refuse: AtomicBool,
}
impl InventoryRepository for RefusesOnce<'_> {
    fn open(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.open(a)
    }
    fn snapshot(&self, a: AccountId) -> Result<Snapshot, Error> {
        self.inner.snapshot(a)
    }
    fn receipt(&self, a: AccountId, id: u64) -> Result<Option<Applied>, Error> {
        self.inner.receipt(a, id)
    }
    fn apply(&self, a: AccountId, b: &Batch, t: Timestamp) -> Result<Applied, Error> {
        if self.refuse.swap(false, Ordering::SeqCst) {
            return Err(Error::Storage);
        }
        self.inner.apply(a, b, t)
    }
}

fn contract(repo: &dyn InventoryRepository) -> Snapshot {
    let f = Fixture::new();
    let catalog = catalog();
    let p = policy(&catalog);
    let story = |objective| {
        Some(Story {
            policy: policy(&catalog),
            objective,
        })
    };
    let who = account(21);
    let isolated = f.initialize(repo, account(22));
    let initial = f.initialize(repo, who);
    assert!(
        commit(
            repo,
            who,
            &f.policy(),
            &completion(301, 2),
            story(30),
            Timestamp(4)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(who).unwrap(), initial);
    finish(repo, who, &p, 21);
    let ready = repo.snapshot(who).unwrap();
    let independent = p.objective(&ready, 40).unwrap();
    // A failed commit exposes neither the payment nor the next objective.
    let refuses = RefusesOnce {
        inner: repo,
        refuse: AtomicBool::new(true),
    };
    assert!(matches!(
        commit(
            &refuses,
            who,
            &f.policy(),
            &completion(301, 2),
            story(30),
            Timestamp(5)
        ),
        Err(inventory::Error::Store(Error::Storage))
    ));
    assert_eq!(repo.snapshot(who).unwrap(), ready);
    assert_eq!(repo.receipt(who, 301).unwrap(), None);
    let result = commit(
        &refuses,
        who,
        &f.policy(),
        &completion(301, 2),
        story(30),
        Timestamp(6),
    )
    .unwrap();
    assert!(!result.replayed);
    assert_eq!(result.generation, ready.generation + 1);
    let first = repo.snapshot(who).unwrap();
    let completed = p.objective(&first, 30).unwrap();
    assert!(completed.played && completed.completed && completed.reward_claimed);
    assert!(!completed.active && !completed.existing && !completed.edited);
    assert!(p.objective(&first, 31).unwrap().active);
    assert!(!p.objective(&first, 32).unwrap().active);
    assert_eq!(p.objective(&first, 40).unwrap(), independent);
    assert_eq!(Balance::from_snapshot(&first).unwrap(), Balance(2000));
    assert_eq!(f.earned(&first), 2000);
    assert_eq!(f.rep(&first), scores(9000));
    assert_eq!(awards::State::load(&first).unwrap().events[0].1.attempts, 1);
    // Later story operations replace the latest story header, but do not
    // invalidate this result's separate durable receipt.
    finish(repo, who, &p, 40);
    commit(
        repo,
        who,
        &f.policy(),
        &completion(302, 0),
        story(31),
        Timestamp(7),
    )
    .unwrap();
    let second = repo.snapshot(who).unwrap();
    assert!(p.objective(&second, 32).unwrap().active);
    assert!(p.objective(&second, 40).unwrap().completed);
    assert_eq!(Balance::from_snapshot(&second).unwrap(), Balance(4000));
    assert_eq!(f.earned(&second), 4000);
    assert_eq!(f.rep(&second), scores(18_000));
    assert_eq!(
        awards::State::load(&second).unwrap().events[0].1.attempts,
        2
    );
    assert_eq!(
        commit(
            repo,
            who,
            &f.policy(),
            &completion(301, 2),
            story(30),
            Timestamp(8)
        )
        .unwrap(),
        Applied {
            replayed: true,
            ..result
        }
    );
    assert_eq!(repo.snapshot(who).unwrap(), second);
    assert!(
        commit(
            repo,
            who,
            &f.policy(),
            &completion(303, 0),
            story(30),
            Timestamp(9)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(who).unwrap(), second);
    assert_eq!(repo.snapshot(account(22)).unwrap(), isolated);
    second
}

#[test]
fn story_result_payment_handoff_and_retry_are_one_atomic_change() {
    contract(&MemoryRepository::new());
    let path = std::env::temp_dir().join(format!(
        "nfs-story-result-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let expected = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        contract(&repo)
    };
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let f = Fixture::new();
        let c = catalog();
        assert_eq!(repo.snapshot(account(21)).unwrap(), expected);
        assert!(
            commit(
                &repo,
                account(21),
                &f.policy(),
                &completion(301, 2),
                Some(Story {
                    policy: policy(&c),
                    objective: 30
                }),
                Timestamp(10)
            )
            .unwrap()
            .replayed
        );
        assert_eq!(repo.snapshot(account(21)).unwrap(), expected);
        // An ordinary later attempt can still earn its repeatable event reward.
        assert!(
            !commit(
                &repo,
                account(21),
                &f.policy(),
                &completion(304, 1),
                None,
                Timestamp(11)
            )
            .unwrap()
            .replayed
        );
        let after = repo.snapshot(account(21)).unwrap();
        assert_eq!(Balance::from_snapshot(&after).unwrap(), Balance(6000));
        assert_eq!(f.rep(&after), scores(27_000));
        assert_eq!(awards::State::load(&after).unwrap().events[0].1.attempts, 3);
        assert_eq!(
            policy(&c).objective(&after, 32).unwrap(),
            policy(&c).objective(&expected, 32).unwrap()
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn an_evicted_story_result_cannot_pay_or_complete_again() {
    let repo = MemoryRepository::new();
    let saved = contract(&repo);
    let who = account(21);
    for n in 0..nfs_storage::MAX_BATCH_HISTORY as u64 {
        repo.apply(
            who,
            &Batch {
                id: 10_000 + n,
                expected_generation: saved.generation + n,
                ops: vec![Op::SetTable(key("Unrelated"), Default::default())],
            },
            Timestamp(20 + n),
        )
        .unwrap();
    }
    assert_eq!(repo.receipt(who, 301).unwrap(), None);
    let before = repo.snapshot(who).unwrap();
    let f = Fixture::new();
    let c = catalog();
    assert!(
        commit(
            &repo,
            who,
            &f.policy(),
            &completion(301, 2),
            Some(Story {
                policy: policy(&c),
                objective: 30
            }),
            Timestamp(90_000)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(who).unwrap(), before);
}
