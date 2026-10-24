// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::bank::Balance;
use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, SqliteRepository, Timestamp};
use serde_json::json;
mod rewards;

fn catalog() -> Catalog {
    Catalog::from_json(
        &json!({"version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,
            "source":{"path":"constructed","sha256":"00".repeat(32)},"branches":[
            {"name":"intro","objectives":[
                {"id":7,"active":{"completed":9007199254740993u64}},
                {"id":9007199254740993u64,"active":{"constant":true}},
                {"id":21,"active":{"completed":7}}],"ints":[]},
            {"name":"main","objectives":[{"id":30,"active":{"completed":21}}],"ints":[]}
        ]}),
    )
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
fn account(byte: u8) -> AccountId {
    AccountId::from_owned_config([byte; 16]).unwrap()
}
fn init(repo: &dyn InventoryRepository, who: AccountId) -> Snapshot {
    repo.open(who).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: vec![
                Op::SetTable(key("ProgressionObjective"), Table::default()),
                Op::SetTable(key("ProgressionStats"), Table::default()),
                Balance(12345).store(),
                Op::SetTable(
                    key("Unrelated"),
                    Table {
                        rows: BTreeMap::from([(
                            19,
                            Row::from([(8, Value::String("keep".into()))]),
                        )]),
                    },
                ),
            ],
        },
        Timestamp(1),
    )
    .unwrap();
    repo.snapshot(who).unwrap()
}
fn apply(repo: &dyn InventoryRepository, who: AccountId, prepared: Prepared) -> Snapshot {
    if let Prepared::Mutation(batch) = prepared {
        repo.apply(who, &batch, Timestamp(2)).unwrap();
    }
    repo.snapshot(who).unwrap()
}

#[test]
fn intro_order_follows_dependencies_not_id_or_catalog_order() {
    let catalog = catalog();
    assert_eq!(
        catalog.intro_order(policy(&catalog).settings).unwrap(),
        [9007199254740993, 7, 21]
    );
}

#[test]
fn operation_ids_advance_past_both_generation_and_previous_receipt() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let c = catalog();
    let p = policy(&c);
    let before = init(&repo, who);
    assert_eq!(p.next_operation(&before).unwrap(), before.generation + 1);
    let saved = apply(
        &repo,
        who,
        p.jump(&before, before.generation, 100, Checkpoint::Objective(7))
            .unwrap(),
    );
    assert_eq!(p.next_operation(&saved).unwrap(), 101);
    let mut later = saved.clone();
    later.generation = 200;
    assert_eq!(p.next_operation(&later).unwrap(), 201);
    later.generation = OPERATION_MASK;
    assert_eq!(p.next_operation(&later), Err(Error::Bounds));
}

#[test]
fn catalog_refuses_ambiguous_and_cyclic_slider_order() {
    for cyclic in [false, true] {
        let first = if cyclic {
            json!({"completed":2})
        } else {
            json!({"constant":true})
        };
        let catalog=Catalog::from_json(&json!({"version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,
            "source":{"path":"constructed","sha256":"00".repeat(32)},"branches":[
            {"name":"intro","objectives":[{"id":1,"active":first},{"id":2,"active":if cyclic{json!({"completed":1})}else{json!({"constant":true})}}],"ints":[]},
            {"name":"main","objectives":[{"id":3,"active":{"completed":2}}],"ints":[]}]})).unwrap();
        assert!(
            catalog
                .intro_order(Settings {
                    consumer_beta: false
                })
                .is_err()
        );
    }
}

#[test]
fn ordered_completions_reconcile_state_and_publish_next_objective_once() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    let before = init(&repo, who);
    assert_eq!(p.inspect(&before).unwrap().position, 0);
    assert!(
        p.complete(&before, before.generation, 1, 7, vec![])
            .is_err()
    );
    let prepared = p
        .complete(
            &before,
            before.generation,
            1,
            9007199254740993,
            vec![Balance(20000).store()],
        )
        .unwrap();
    let Prepared::Mutation(batch) = prepared.clone() else {
        panic!("expected mutation")
    };
    let saved = apply(&repo, who, prepared);
    let view = p.inspect(&saved).unwrap();
    assert_eq!(view.position, 1);
    assert!(view.milestones[0].played && view.milestones[0].reward_claimed);
    assert!(view.milestones[1].active && !view.milestones[2].active);
    assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(20000));
    assert!(repo.apply(who, &batch, Timestamp(3)).unwrap().replayed);
    assert_eq!(
        p.complete(&saved, before.generation, 1, 9007199254740993, vec![])
            .unwrap(),
        Prepared::Replayed
    );
    assert_eq!(repo.snapshot(who).unwrap(), saved);
}

#[test]
fn jump_and_rewind_preserve_money_other_tables_and_earned_receipts() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    let before = init(&repo, who);
    let earned = apply(
        &repo,
        who,
        p.complete(
            &before,
            before.generation,
            1,
            9007199254740993,
            vec![Balance(20000).store()],
        )
        .unwrap(),
    );
    let skipped = apply(
        &repo,
        who,
        p.jump(&earned, earned.generation, 2, Checkpoint::AfterIntro)
            .unwrap(),
    );
    let view = p.inspect(&skipped).unwrap();
    assert_eq!(view.position, 3);
    assert!(view.stats_consistent);
    assert!(view.milestones[0].played && view.milestones[1].edited && !view.milestones[1].played);
    assert_eq!(
        skipped.tables[&key("Unrelated")],
        before.tables[&key("Unrelated")]
    );
    assert_eq!(
        skipped.tables[&key("ProgressionObjective")].rows[&30][&key("Active")],
        Value::Int(1)
    );
    let rewound = apply(
        &repo,
        who,
        p.jump(
            &skipped,
            skipped.generation,
            3,
            Checkpoint::Objective(9007199254740993),
        )
        .unwrap(),
    );
    assert_eq!(p.inspect(&rewound).unwrap().position, 0);
    assert!(
        p.complete(
            &rewound,
            rewound.generation,
            4,
            9007199254740993,
            vec![Balance(25000).store()]
        )
        .is_err()
    );
    let replayed = apply(
        &repo,
        who,
        p.complete(&rewound, rewound.generation, 4, 9007199254740993, vec![])
            .unwrap(),
    );
    assert_eq!(Balance::from_snapshot(&replayed).unwrap(), Balance(20000));
    assert!(p.inspect(&replayed).unwrap().milestones[0].reward_claimed);
}

#[test]
fn legacy_post_intro_is_reported_as_existing_and_migrated_without_reward_replay() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    let mut old = init(&repo, who);
    old.tables.insert(
        key("ProgressionObjective"),
        Table {
            rows: catalog
                .post_intro(p.settings)
                .into_iter()
                .map(|r| {
                    (
                        r.id,
                        Row::from([
                            (key("Active"), Value::Int(r.active.into())),
                            (key("Completed"), Value::Int(r.completed.into())),
                        ]),
                    )
                })
                .collect(),
        },
    );
    let view = p.inspect(&old).unwrap();
    assert_eq!(view.position, 3);
    assert!(!view.stats_consistent);
    assert!(view.milestones.iter().all(|m| m.existing && !m.played));
    let Prepared::Mutation(batch) = p
        .jump(
            &old,
            old.generation,
            1,
            Checkpoint::Objective(9007199254740993),
        )
        .unwrap()
    else {
        panic!()
    };
    for op in batch.ops {
        if let Op::SetTable(k, t) = op {
            old.tables.insert(k, t);
        }
    }
    old.generation += 1;
    assert!(p.inspect(&old).unwrap().stats_consistent);
    assert!(
        p.complete(
            &old,
            old.generation,
            2,
            9007199254740993,
            vec![Balance(1).store()]
        )
        .is_err()
    );
}

#[test]
fn reject_partial_nonprefix_stale_conflicting_and_reused_operations() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    let before = init(&repo, who);
    assert!(p.jump(&before, 0, 1, Checkpoint::AfterIntro).is_err());
    assert!(
        p.jump(&before, before.generation, 0, Checkpoint::AfterIntro)
            .is_err()
    );
    assert!(
        p.jump(&before, before.generation, 1, Checkpoint::Objective(0))
            .is_err()
    );
    assert!(
        p.complete(
            &before,
            before.generation,
            1,
            9007199254740993,
            vec![Op::SetTable(key("ProgressionObjective"), Table::default())]
        )
        .is_err()
    );
    let saved = apply(
        &repo,
        who,
        p.jump(&before, before.generation, 2, Checkpoint::AfterIntro)
            .unwrap(),
    );
    assert_eq!(
        p.jump(&saved, before.generation, 2, Checkpoint::AfterIntro)
            .unwrap(),
        Prepared::Replayed
    );
    assert!(
        p.jump(&saved, saved.generation, 2, Checkpoint::AfterIntro)
            .is_err()
    );
    assert!(
        p.jump(&saved, saved.generation, 1, Checkpoint::AfterIntro)
            .is_err()
    );
    let mut partial = before.clone();
    partial
        .tables
        .get_mut(&key("ProgressionObjective"))
        .unwrap()
        .rows
        .insert(7, Row::from([(key("Active"), Value::Int(1))]));
    assert!(p.inspect(&partial).is_err());
    let mut gap = before;
    gap.tables
        .get_mut(&key("ProgressionObjective"))
        .unwrap()
        .rows
        .insert(
            7,
            Row::from([
                (key("Active"), Value::Int(0)),
                (key("Completed"), Value::Int(1)),
            ]),
        );
    assert!(p.inspect(&gap).is_err());
}

#[test]
fn malformed_receipts_never_turn_into_accepted_retries_or_partial_provenance() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    let before = init(&repo, who);
    let saved = apply(
        &repo,
        who,
        p.jump(&before, before.generation, 1, Checkpoint::AfterIntro)
            .unwrap(),
    );
    for column in [BEFORE, TARGET, ACTION] {
        let mut corrupt = saved.clone();
        let header = corrupt
            .tables
            .get_mut(&key("AfterhoursStory"))
            .unwrap()
            .rows
            .get_mut(&HEADER)
            .unwrap();
        header.insert(
            column,
            match column {
                BEFORE => Value::String(saved.generation.to_string()),
                TARGET => Value::String("30".into()), // main-story objective, not an intro stop
                _ => Value::Int(1), // objective completion cannot target AfterIntro
            },
        );
        assert!(p.inspect(&corrupt).is_err());
        assert!(
            p.jump(&corrupt, before.generation, 1, Checkpoint::AfterIntro)
                .is_err()
        );
    }
    let mut partial = saved.clone();
    partial
        .tables
        .get_mut(&key("AfterhoursStory"))
        .unwrap()
        .rows
        .remove(&7);
    assert!(p.inspect(&partial).is_err());
    let mut later = saved.clone();
    later.generation += 10; // unrelated durable commits are allowed
    assert!(p.inspect(&later).is_ok());
}

#[test]
fn failed_effect_is_atomic_and_accounts_are_isolated() {
    let repo = MemoryRepository::default();
    let a = account(1);
    let b = account(2);
    let catalog = catalog();
    let p = policy(&catalog);
    let before = init(&repo, a);
    let other = init(&repo, b);
    let bad_id = nfs_storage::ItemId::new(999).unwrap();
    let Prepared::Mutation(batch) = p
        .complete(
            &before,
            before.generation,
            1,
            9007199254740993,
            vec![Op::Remove(bad_id)],
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(repo.apply(a, &batch, Timestamp(2)).is_err());
    assert_eq!(repo.snapshot(a).unwrap(), before);
    assert_eq!(repo.snapshot(b).unwrap(), other);
}

#[test]
fn sqlite_restart_retains_checkpoint_and_provenance() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-story-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = account(1);
    let catalog = catalog();
    let p = policy(&catalog);
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let snapshot = init(&repo, who);
        apply(
            &repo,
            who,
            p.jump(&snapshot, snapshot.generation, 1, Checkpoint::Objective(21))
                .unwrap(),
        );
    }
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    let view = p.inspect(&repo.snapshot(who).unwrap()).unwrap();
    assert_eq!(view.position, 2);
    assert!(view.milestones[0].edited && view.milestones[1].edited && view.milestones[2].active);
    drop(repo);
    std::fs::remove_dir_all(path).unwrap();
}
