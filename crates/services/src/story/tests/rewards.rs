// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, InventoryCatalog, Layout, OwnershipLevel,
};
const OBJECTIVE: u64 = 9007199254740993;
fn inventory() -> InventoryCatalog {
    let car = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        buy_price: 100,
        sell_price: 60,
        quantity: u32::MAX,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags {
            uses_scope: true,
            ..Default::default()
        },
        sub_items: vec![[2; 16]],
        additional_items: vec![[3; 16]],
        default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
    };
    let part = Definition {
        class: DefinitionClass::AirFilterItemData,
        buy_price: 10,
        sell_price: 5,
        quantity: 1,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags {
            requires_owner: true,
            ..Default::default()
        },
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::decode(Layout::Empty, &[]).unwrap()),
    };
    let wrap = Definition {
        class: DefinitionClass::LiveryCustomizationItemData,
        flags: DefinitionFlags::default(),
        default_derived: Some(Derived::LiveryCustomization {
            byte_vault_data_id: 0,
            library_preset_id: 0,
            dynamic_flags: 0,
        }),
        ..part.clone()
    };
    InventoryCatalog::new(
        [
            ([1; 16], car.clone()),
            ([4; 16], car),
            ([2; 16], part),
            ([3; 16], wrap),
        ],
        vec![],
    )
    .unwrap()
}
fn grant(
    repo: &dyn InventoryRepository,
    who: AccountId,
    p: &Policy<'_>,
    inv: &InventoryCatalog,
) -> Snapshot {
    let before = repo.snapshot(who).unwrap();
    let grant = p
        .grant_vehicle(&before, before.generation, OBJECTIVE, [1; 16], inv)
        .unwrap();
    repo.apply(who, &grant.batch.unwrap(), Timestamp(3))
        .unwrap();
    repo.snapshot(who).unwrap()
}
#[test]
fn award_precedes_completion_and_survives_rewind_without_a_second_grant() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let c = catalog();
    let p = policy(&c);
    let inv = inventory();
    let before = init(&repo, who);
    let saved = grant(&repo, who, &p, &inv);
    assert_eq!(saved.items.len(), 3);
    assert_eq!(Balance::from_snapshot(&saved).unwrap(), Balance(12345));
    assert_eq!(saved.garage, before.garage);
    let view = p.inspect(&saved).unwrap();
    assert_eq!(view.position, 0);
    assert!(
        view.milestones[0].active
            && view.milestones[0].reward_claimed
            && !view.milestones[0].played
    );
    assert!(
        p.grant_vehicle(&saved, before.generation, OBJECTIVE, [1; 16], &inv)
            .unwrap()
            .batch
            .is_none()
    );
    assert!(
        p.grant_vehicle(&saved, saved.generation, OBJECTIVE, [4; 16], &inv)
            .is_err()
    );
    let done = apply(
        &repo,
        who,
        p.complete(
            &saved,
            saved.generation,
            p.next_operation(&saved).unwrap(),
            OBJECTIVE,
            vec![],
        )
        .unwrap(),
    );
    assert_eq!(p.inspect(&done).unwrap().position, 1);
    assert_eq!(done.items, saved.items);
    let rewind = apply(
        &repo,
        who,
        p.jump(
            &done,
            done.generation,
            p.next_operation(&done).unwrap(),
            Checkpoint::Objective(OBJECTIVE),
        )
        .unwrap(),
    );
    assert!(
        p.grant_vehicle(&rewind, rewind.generation, OBJECTIVE, [1; 16], &inv)
            .unwrap()
            .batch
            .is_none()
    );
    assert!(
        p.complete(
            &rewind,
            rewind.generation,
            p.next_operation(&rewind).unwrap(),
            OBJECTIVE,
            vec![Balance(1).store()]
        )
        .is_err()
    );
}
#[test]
fn invalid_choice_generation_and_failed_batch_do_not_mutate_any_account() {
    let repo = MemoryRepository::default();
    let a = account(1);
    let b = account(2);
    let c = catalog();
    let p = policy(&c);
    let inv = inventory();
    let before = init(&repo, a);
    let other = init(&repo, b);
    for (generation, objective, definition) in [
        (0, OBJECTIVE, [1; 16]),
        (before.generation, 7, [1; 16]),
        (before.generation, OBJECTIVE, [2; 16]),
        (before.generation, OBJECTIVE, [0; 16]),
    ] {
        assert!(
            p.grant_vehicle(&before, generation, objective, definition, &inv)
                .is_err()
        );
    }
    let mut batch = p
        .grant_vehicle(&before, before.generation, OBJECTIVE, [1; 16], &inv)
        .unwrap()
        .batch
        .unwrap();
    batch
        .ops
        .push(Op::Remove(nfs_storage::ItemId::new(999).unwrap()));
    assert!(repo.apply(a, &batch, Timestamp(4)).is_err());
    assert_eq!(repo.snapshot(a).unwrap(), before);
    assert_eq!(repo.snapshot(b).unwrap(), other);
    let first = p
        .grant_vehicle(&before, before.generation, OBJECTIVE, [1; 16], &inv)
        .unwrap()
        .batch
        .unwrap();
    let competing = p
        .grant_vehicle(&before, before.generation, OBJECTIVE, [4; 16], &inv)
        .unwrap()
        .batch
        .unwrap();
    repo.apply(a, &first, Timestamp(5)).unwrap();
    assert!(repo.apply(a, &competing, Timestamp(6)).unwrap().replayed);
    // Repository retry IDs are generation based. The service rechecks the
    // semantic receipt instead of acknowledging a different competing choice.
    assert!(
        p.claim_vehicle(&repo, a, OBJECTIVE, [4; 16], &inv, Timestamp(7))
            .is_err()
    );
    assert!(
        p.claim_vehicle(&repo, a, OBJECTIVE, [1; 16], &inv, Timestamp(8))
            .unwrap()
            .replayed
    );
    assert_eq!(repo.snapshot(b).unwrap(), other);
}
#[test]
fn corrupt_or_removed_reward_cannot_be_granted_again() {
    let repo = MemoryRepository::default();
    let who = account(1);
    let c = catalog();
    let p = policy(&c);
    let inv = inventory();
    init(&repo, who);
    let saved = grant(&repo, who, &p, &inv);
    let mut corrupt = saved.clone();
    corrupt
        .tables
        .get_mut(&key("AfterhoursStoryRewards"))
        .unwrap()
        .rows
        .get_mut(&0)
        .unwrap()
        .insert(0, Value::Int(2));
    assert!(p.inspect(&corrupt).is_err());
    let mut missing = saved.clone();
    missing.items.clear();
    assert!(
        p.grant_vehicle(&missing, missing.generation, OBJECTIVE, [1; 16], &inv)
            .is_err()
    );
    for value in ["00".repeat(16), "FF".repeat(16), "not-a-guid".into()] {
        let mut bad = saved.clone();
        bad.tables
            .get_mut(&key("AfterhoursStoryRewards"))
            .unwrap()
            .rows
            .get_mut(&OBJECTIVE)
            .unwrap()
            .insert(0, Value::String(value));
        assert!(p.inspect(&bad).is_err());
    }
}
#[test]
fn sqlite_restart_between_award_and_completion_retains_choice() {
    let path = std::env::temp_dir().join(format!(
        "afterhours-story-reward-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = account(1);
    let c = catalog();
    let p = policy(&c);
    let inv = inventory();
    let saved = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        init(&repo, who);
        grant(&repo, who, &p, &inv)
    };
    let repo = SqliteRepository::open_owned_directory(&path).unwrap();
    let restored = repo.snapshot(who).unwrap();
    assert_eq!(saved, restored);
    assert_eq!(p.inspect(&restored).unwrap().position, 0);
    assert!(
        p.grant_vehicle(&restored, restored.generation, OBJECTIVE, [1; 16], &inv)
            .unwrap()
            .batch
            .is_none()
    );
    drop(repo);
    std::fs::remove_dir_all(path).unwrap();
}
