// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_services::persistent;
use nfs_storage::account::{Kind, Repository};
use nfs_world::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, InventoryCatalog, Layout, OwnershipLevel,
};

const OPENING: u64 = 9007199254740993;

fn fixture() -> Fixture {
    let mut f = Fixture::new();
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
        additional_items: vec![],
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
    f.rules.items = Some(
        InventoryCatalog::new(
            [([9; 16], car.clone()), ([4; 16], car), ([2; 16], part)],
            vec![[9; 16]],
        )
        .unwrap(),
    );
    let garage = [
        "PrimaryVehicleItem",
        "Vehicle1",
        "Vehicle2",
        "Vehicle3",
        "Vehicle4",
    ];
    let tables: Vec<_> = persistent::required_tables().iter().map(|&(table, secondary, count)| {
        let columns: Vec<_> = (0..count).map(|i| {
            let name = if table == "GarageItemsTable" { garage[i].into() }
                else if table == "ProgressionObjective" { ["Active", "Completed"][i].into() }
                else { format!("Column{i}") };
            json!({"key":key(&name),"name":name,"value_type":if table=="GarageItemsTable" {"string"}else{"int"},"default":"0"})
        }).collect();
        json!({"name":table,"key":key(table),"secondary_key":secondary,"using_stores":secondary.is_none(),"write_whole_rows":false,"columns":columns})
    }).collect();
    f.rules.persistent = Some(persistent::Catalog::from_json(&json!({"version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"source":{"path":"constructed-reset-test","sha256":"00".repeat(32)},"tables":tables})).unwrap());
    let before = f.repo.snapshot(account(1)).unwrap();
    let mut item = before.items.values().next().unwrap().clone();
    item.derived = vec![0; 68];
    f.repo
        .apply(
            account(1),
            &Batch {
                id: 2,
                expected_generation: before.generation,
                ops: vec![
                    Op::Update(item),
                    Op::SetTable(
                        key("AfterhoursGarageSessions"),
                        Table {
                            rows: BTreeMap::from([(0, Row::from([(0, Value::Int(42))]))]),
                        },
                    ),
                ],
            },
            Timestamp(2),
        )
        .unwrap();
    let docs = Repository::open_owned_directory(&f.root.join("artifacts/account")).unwrap();
    docs.prepare(account(1)).unwrap();
    for kind in Kind::ALL {
        docs.compare_exchange(account(1), kind, None, b"previous-domain-state")
            .unwrap();
    }
    f
}

fn editor(f: &Fixture) -> Editor<'_> {
    Editor {
        restart: true,
        ..f.editor(true)
    }
}
fn proof(f: &Fixture) -> Proof {
    let preview = editor(f).preview(Checkpoint::Objective(OPENING)).unwrap();
    assert!(
        preview.restart && preview.can_apply,
        "{:?}",
        preview.reasons
    );
    Proof {
        target: Checkpoint::Objective(OPENING),
        expected: decimal(&preview.generation).unwrap(),
        plan_id: preview.plan_id,
    }
}

#[test]
fn full_restart_at_opening_clears_car_later_career_and_records_with_restorable_backup() {
    let f = fixture();
    let before = f.repo.inspect(account(1)).unwrap();
    let other = f.repo.inspect(account(2)).unwrap();
    let p = proof(&f);
    let applied = editor(&f).apply(&p, "restart", Timestamp(3)).unwrap();
    assert!(applied.restart && !applied.replayed);
    let after = f.repo.inspect(account(1)).unwrap();
    assert_eq!(after.snapshot.generation, before.snapshot.generation + 1);
    assert!(after.snapshot.items.is_empty());
    assert_eq!(after.snapshot.garage, Some(GarageSlots::default()));
    assert_eq!(
        nfs_services::bank::Balance::from_snapshot(&after.snapshot).unwrap(),
        nfs_services::bank::Balance(0)
    );
    assert!(!after.snapshot.tables.contains_key(&key("Reputation")));
    assert_eq!(
        after.snapshot.tables[&key("ProgressionObjective")].rows[&30][&key("Completed")],
        Value::Int(0)
    );
    assert_eq!(
        after.snapshot.tables[&key("ProgressionObjective")].rows[&30][&key("Active")],
        Value::Int(0)
    );
    assert_eq!(
        after.snapshot.tables[&key("AfterhoursGarageSessions")],
        before.snapshot.tables[&key("AfterhoursGarageSessions")]
    );
    let state = f.rules.policy().inspect(&after.snapshot).unwrap();
    assert_eq!(state.position, 0);
    assert!(
        state
            .milestones
            .iter()
            .all(|m| !m.completed && !m.played && !m.reward_claimed && !m.existing)
    );
    for kind in [
        Kind::Identity,
        Kind::Settings,
        Kind::Entitlements,
        Kind::Kickback,
    ] {
        assert_eq!(after.documents[&kind], before.documents[&kind]);
    }
    assert_eq!(after.documents[&Kind::Speedwall].revision, 2);
    let records: serde_json::Value =
        serde_json::from_slice(&after.documents[&Kind::Speedwall].bytes).unwrap();
    assert_eq!(records["rows"], json!([]));
    let backup = SqliteRepository::open_existing_directory(
        &f.root
            .join("artifacts/story-backups")
            .join(&applied.backup.id),
    )
    .unwrap();
    assert_eq!(backup.inspect(account(1)).unwrap(), before);
    assert!(
        editor(&f)
            .apply(&p, "retry", Timestamp(4))
            .unwrap()
            .replayed
    );
    assert_eq!(f.backups(), 1);
    assert_eq!(f.repo.inspect(account(1)).unwrap(), after);
    assert_eq!(f.repo.inspect(account(2)).unwrap(), other);
    // Reopening storage must see the same committed fresh career.
    let reopened =
        SqliteRepository::open_existing_directory(&f.root.join("artifacts/account")).unwrap();
    assert_eq!(reopened.inspect(account(1)).unwrap(), after);
}

#[test]
fn a_second_restart_allows_a_different_authored_starter_once() {
    let f = fixture();
    editor(&f).apply(&proof(&f), "first", Timestamp(3)).unwrap();
    let policy = f.rules.policy();
    let items = f.rules.items.as_ref().unwrap();
    let first = policy
        .claim_selection(&f.repo, account(1), OPENING, [9; 16], items, Timestamp(4))
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.snapshot.items.len(), 2);
    editor(&f).apply(&proof(&f), "again", Timestamp(5)).unwrap();
    let next = policy
        .claim_selection(&f.repo, account(1), OPENING, [4; 16], items, Timestamp(6))
        .unwrap();
    assert!(!next.replayed);
    assert_eq!(next.snapshot.items.len(), 2);
    assert_eq!(
        next.snapshot.items[&next.receipt.item].definition.0,
        [4; 16]
    );
    assert!(
        next.snapshot
            .items
            .values()
            .any(|item| item.owner == Some(next.receipt.item) && item.definition.0 == [2; 16])
    );
    assert!(
        policy
            .claim_selection(&f.repo, account(1), OPENING, [4; 16], items, Timestamp(7))
            .unwrap()
            .replayed
    );
    assert_eq!(f.repo.snapshot(account(1)).unwrap(), next.snapshot);
}

#[test]
fn failed_reset_rolls_back_deleted_inventory_tables_documents_and_receipt() {
    let f = fixture();
    let p = proof(&f);
    let before = f.repo.inspect(account(1)).unwrap();
    let sql = rusqlite::Connection::open(f.repo.path(account(1))).unwrap();
    sql.execute_batch(&format!("CREATE TRIGGER reject_restart BEFORE INSERT ON persistent WHEN NEW.id={} BEGIN SELECT RAISE(ABORT,'injected restart failure'); END;", key("AfterhoursStoryEdits"))).unwrap();
    assert!(editor(&f).apply(&p, "failed", Timestamp(3)).is_err());
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
    assert_eq!(f.backups(), 1);
    sql.execute_batch("DROP TRIGGER reject_restart;").unwrap();
    drop(sql);
    assert!(editor(&f).apply(&p, "working", Timestamp(4)).is_ok());
    assert_eq!(f.backups(), 2);
}

#[test]
fn restart_rejects_wrong_intent_target_stale_save_and_missing_defaults_before_backup() {
    let mut f = fixture();
    let p = proof(&f);
    assert!(
        f.editor(true)
            .apply(&p, "wrong-intent", Timestamp(3))
            .is_err()
    );
    assert!(
        !editor(&f)
            .preview(Checkpoint::AfterIntro)
            .unwrap()
            .can_apply
    );
    let selected = Editor {
        starter: Some("04".repeat(16)),
        ..editor(&f)
    };
    assert!(selected.preview(p.target).is_err());
    let stale = Proof {
        expected: p.expected - 1,
        ..p
    };
    assert!(editor(&f).apply(&stale, "stale", Timestamp(3)).is_err());
    f.rules.persistent = None;
    assert!(!editor(&f).preview(stale.target).unwrap().can_apply);
    assert_eq!(f.backups(), 0);
}

#[test]
fn storage_reset_rejects_stale_documents_foreign_leases_and_invalid_graphs_atomically() {
    let f = fixture();
    let before = f.repo.inspect(account(1)).unwrap();
    let editor = editor(&f);
    let operation = f.rules.policy().next_operation(&before.snapshot).unwrap();
    let batch = editor
        .restart_batch(&before.snapshot, operation, Checkpoint::Objective(OPENING))
        .unwrap();
    let docs =
        nfs_services::fresh_account::restart_documents(account(1), &before.documents).unwrap();
    let lease = f.repo.writer_lease(account(1)).unwrap();
    let mut stale_docs = docs.clone();
    stale_docs[0].1.revision += 1;
    assert!(
        f.repo
            .restart_progress(&lease, &batch, &stale_docs, Timestamp(3))
            .is_err()
    );
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
    let mut invalid = batch.clone();
    invalid.ops.retain(|op| !matches!(op, Op::SetGarage(_)));
    invalid.ops.push(Op::SetGarage(GarageSlots([
        ItemId::new(999),
        None,
        None,
        None,
        None,
    ])));
    assert!(
        f.repo
            .restart_progress(&lease, &invalid, &docs, Timestamp(3))
            .is_err()
    );
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
    let foreign = Fixture::new();
    let foreign_lease = foreign.repo.writer_lease(account(1)).unwrap();
    assert!(
        f.repo
            .restart_progress(&foreign_lease, &batch, &docs, Timestamp(3))
            .is_err()
    );
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
    f.repo
        .restart_progress(&lease, &batch, &docs, Timestamp(3))
        .unwrap();
    let after = f.repo.inspect(account(1)).unwrap();
    // The old reset must not clear later earned money, even when retried directly.
    f.repo
        .apply(
            account(1),
            &Batch {
                id: 778,
                expected_generation: after.snapshot.generation,
                ops: vec![nfs_services::bank::Balance(51).store()],
            },
            Timestamp(4),
        )
        .unwrap();
    let later = f.repo.inspect(account(1)).unwrap();
    assert!(
        f.repo
            .restart_progress(&lease, &batch, &docs, Timestamp(5))
            .unwrap()
            .replayed
    );
    assert_eq!(f.repo.inspect(account(1)).unwrap(), later);
}
