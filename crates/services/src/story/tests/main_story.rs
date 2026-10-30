// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn branching() -> Catalog {
    Catalog::from_json(
        &json!({"version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,
        "source":{"path":"constructed","sha256":"00".repeat(32)},"branches":[
        {"name":"intro","objectives":[{"id":21,"active":{"constant":true}}],"ints":[]},
        {"name":"main","objectives":[
            {"id":30,"active":{"completed":21}},
            {"id":31,"active":{"completed":30}},
            {"id":40,"active":{"completed":21}},
            {"id":41,"active":{"all":[{"completed":30},{"completed":40}]}}
        ],"ints":[]}]}),
    )
    .unwrap()
}

fn ready(repo: &dyn InventoryRepository, who: AccountId, p: &Policy<'_>) -> Snapshot {
    let before = init(repo, who);
    apply(
        repo,
        who,
        p.complete(&before, before.generation, 2, 21, vec![])
            .unwrap(),
    )
}

#[test]
fn completion_preserves_concurrent_objectives_and_rewards_are_once_only() {
    let c = branching();
    let p = policy(&c);
    let repo = MemoryRepository::default();
    let who = account(1);
    let before = ready(&repo, who, &p);
    let other = p.objective(&before, 40).unwrap();
    assert!(other.active && !other.completed);
    assert_eq!(
        p.complete(&before, before.generation, 3, 31, vec![]),
        Err(Error::Invalid)
    );
    let saved = apply(
        &repo,
        who,
        p.complete(
            &before,
            before.generation,
            3,
            30,
            vec![Balance(12845).store()],
        )
        .unwrap(),
    );
    assert_eq!(p.objective(&saved, 40).unwrap(), other);
    assert!(p.objective(&saved, 31).unwrap().active);
    assert!(!p.objective(&saved, 41).unwrap().active);
    let played = p.objective(&saved, 30).unwrap();
    assert!(played.completed && played.played && played.reward_claimed);
    assert!(!played.active && !played.existing && !played.edited);
    assert_eq!(p.inspect(&saved).unwrap().position, 1);
    assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(12845));
    assert_eq!(
        p.complete(&saved, before.generation, 3, 30, vec![])
            .unwrap(),
        Prepared::Replayed
    );
    assert_eq!(
        p.complete(
            &saved,
            saved.generation,
            4,
            30,
            vec![Balance(13345).store()]
        ),
        Err(Error::Invalid)
    );
    let next = apply(
        &repo,
        who,
        p.complete(&saved, saved.generation, 4, 40, vec![]).unwrap(),
    );
    assert!(p.objective(&next, 41).unwrap().active);
    assert!(p.objective(&next, 31).unwrap().active);
    assert_eq!(p.objective(&next, 30).unwrap(), played);
    assert_eq!(
        next.tables[&key("Unrelated")],
        before.tables[&key("Unrelated")]
    );
    assert_eq!(
        p.jump(&next, next.generation, 5, Checkpoint::Objective(30)),
        Err(Error::Invalid)
    );
}

#[test]
fn legacy_intro_ledger_migrates_transactionally_without_claiming_existing_story() {
    let c = branching();
    let p = policy(&c);
    let repo = MemoryRepository::default();
    let who = account(1);
    let mut legacy = ready(&repo, who, &p);
    let ledger = legacy.tables.get_mut(&key("AfterhoursStory")).unwrap();
    ledger.rows.retain(|id, _| *id == HEADER || *id == 21);
    ledger
        .rows
        .get_mut(&HEADER)
        .unwrap()
        .insert(FORMAT, Value::Int(INTRO_VERSION));
    legacy
        .tables
        .get_mut(&key("ProgressionObjective"))
        .unwrap()
        .rows
        .insert(
            40,
            Row::from([
                (key("Active"), Value::Int(0)),
                (key("Completed"), Value::Int(1)),
            ]),
        );
    let untouched = legacy.clone();
    assert!(p.inspect(&legacy).is_ok());
    assert!(p.objective(&legacy, 40).unwrap().existing);
    let Prepared::Mutation(batch) = p
        .complete(
            &legacy,
            legacy.generation,
            3,
            30,
            vec![Balance(12845).store()],
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(legacy, untouched); // Preparing/migrating never writes the caller's snapshot.
    for op in batch.ops {
        if let Op::SetTable(key, table) = op {
            legacy.tables.insert(key, table);
        }
    }
    legacy.generation += 1;
    assert_eq!(
        legacy.tables[&key("AfterhoursStory")].rows[&HEADER][&FORMAT],
        Value::Int(VERSION)
    );
    let old = p.objective(&legacy, 40).unwrap();
    assert!(old.completed && old.existing && !old.played && !old.reward_claimed);
    assert!(p.objective(&legacy, 31).unwrap().active && p.objective(&legacy, 41).unwrap().active);
    assert_eq!(
        p.complete(&legacy, legacy.generation, 4, 40, vec![Balance(1).store()]),
        Err(Error::Invalid)
    );
    assert_eq!(
        legacy.tables[&key("AfterhoursStory")].rows[&21],
        untouched.tables[&key("AfterhoursStory")].rows[&21]
    );
}

#[test]
fn failed_and_stale_commits_do_not_publish_progress_or_affect_another_account() {
    let c = branching();
    let p = policy(&c);
    let repo = MemoryRepository::default();
    let who = account(1);
    let before = ready(&repo, who, &p);
    let other = ready(&repo, account(2), &p);
    let Prepared::Mutation(bad) = p
        .complete(
            &before,
            before.generation,
            3,
            30,
            vec![
                Balance(12845).store(),
                Op::Remove(nfs_storage::ItemId::new(999).unwrap()),
            ],
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(repo.apply(who, &bad, Timestamp(3)).is_err());
    assert_eq!(repo.snapshot(who).unwrap(), before);
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
    let Prepared::Mutation(stale) = p
        .complete(&before, before.generation, 4, 40, vec![])
        .unwrap()
    else {
        panic!()
    };
    let saved = apply(
        &repo,
        who,
        p.complete(&before, before.generation, 3, 30, vec![])
            .unwrap(),
    );
    assert_eq!(repo.apply(who, &stale, Timestamp(4)), Err(Error::Conflict));
    assert_eq!(repo.snapshot(who).unwrap(), saved);
    assert!(p.objective(&saved, 40).unwrap().active);
    assert!(
        p.complete(
            &saved,
            saved.generation,
            5,
            40,
            vec![Op::SetTable(key("AfterhoursStory"), Table::default())]
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(account(2)).unwrap(), other);
}

#[test]
fn sqlite_restart_retains_main_completion_and_unrelated_active_objectives() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-main-story-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let c = branching();
    let p = policy(&c);
    let who = account(1);
    let saved = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let before = ready(&repo, who, &p);
        apply(
            &repo,
            who,
            p.complete(
                &before,
                before.generation,
                3,
                30,
                vec![Balance(12845).store()],
            )
            .unwrap(),
        )
    };
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    let restarted = repo.snapshot(who).unwrap();
    assert_eq!(restarted, saved);
    assert!(p.objective(&restarted, 30).unwrap().reward_claimed);
    assert!(p.objective(&restarted, 31).unwrap().active);
    assert!(p.objective(&restarted, 40).unwrap().active);
    assert!(!p.objective(&restarted, 41).unwrap().active);
    assert_eq!(Balance::from_snapshot(&restarted).unwrap(), Balance(12845));
    drop(repo);
    std::fs::remove_dir_all(path).unwrap();
}
