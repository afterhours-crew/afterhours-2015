// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{InventoryRepository, SqliteRepository};
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, Layout, OwnershipLevel,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Root(PathBuf, PathBuf);
impl Root {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self::with_stamp(stamp)
    }
    fn with_stamp(stamp: u128) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fresh-account-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.join("account"), path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

#[test]
fn coincident_fixture_clocks_preserve_separate_data_and_cleanup() {
    let first = Root::with_stamp(0);
    let second = Root::with_stamp(0);
    let prepared = prepare(5);
    assert!(!first.0.exists());
    assert!(!second.0.exists());
    prepared.publish(&first.0).unwrap();
    prepared.publish(&second.0).unwrap();
    std::fs::write(first.0.join("owned"), b"first").unwrap();
    std::fs::write(second.0.join("owned"), b"second").unwrap();
    assert_eq!(std::fs::read(first.0.join("owned")).unwrap(), b"first");
    drop(first);
    assert_eq!(std::fs::read(second.0.join("owned")).unwrap(), b"second");
}

fn policy() -> Value {
    json!({"format":"nfs-fresh-account-policy","version":1,"build_sha256":BUILD,"screenshot_count_max":20,
    "entitlements":{"scopes":[["synthetic"]],"grants":[
        {"device_uri":"","grant_date":"","group_name":"synthetic","is_consumable":false,"project_id":"synthetic-project","product_catalog":1,"product_id":"synthetic-product","status":1,"status_reason_code":0,"entitlement_tag":"synthetic-tag","termination_date":"","entitlement_type":5,"use_count":0,"version":1}
    ]}})
}
fn catalog() -> InventoryCatalog {
    InventoryCatalog::new(
        [(
            [4; 16],
            Definition {
                class: DefinitionClass::RaceVehicleItemData,
                buy_price: 10,
                sell_price: 5,
                quantity: 1,
                ownership_level: OwnershipLevel::Owned,
                flags: DefinitionFlags::default(),
                sub_items: vec![],
                additional_items: vec![],
                default_derived: Some(Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap()),
            },
        )],
        vec![[4; 16]],
    )
    .unwrap()
}
fn prepare(label: u8) -> Prepared {
    prepare_with(label, &persistent::tests::catalog(), &[]).unwrap()
}

#[test]
fn starting_resources_publish_atomically_and_never_reset_an_existing_save() {
    use crate::{
        bank::Balance,
        reputation::{Field, Scores, Thresholds},
    };
    let mut content = persistent::tests::content();
    let rep = content["tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|table| table["name"] == "RepValuesTable")
        .unwrap();
    rep["columns"] = json!(
        [
            "RepScore",
            "RepLevel",
            "BuildScore",
            "CrewScore",
            "OutlawScore",
            "SpeedScore",
            "StyleScore"
        ]
        .map(
            |name| json!({"name":name,"key":persistent::key(name),"value_type":"int","default":"7"})
        )
    );
    let tables = persistent::Catalog::from_json(&content).unwrap();
    let thresholds = Thresholds::new(vec![0, 100, 300]).unwrap();
    let root = Root::new();
    let prepared = prepare_with(31, &tables, &[])
        .unwrap()
        .with_starting_resources(&tables, &thresholds, 2, Balance(10_000_000))
        .unwrap();
    let account = prepared.identity().storage_account();
    assert!(
        !root.0.exists(),
        "resource preparation must remain in memory"
    );
    prepared.publish(&root.0).unwrap();
    {
        let repository = SqliteRepository::open_owned_directory(&root.0).unwrap();
        let saved = repository.snapshot(account).unwrap();
        assert_eq!(saved.generation, 1, "one atomic creation batch");
        assert_eq!(Balance::from_snapshot(&saved), Ok(Balance(10_000_000)));
        let scores = Scores::load(&tables.view(&saved).unwrap()).unwrap();
        assert_eq!(scores.total, 100);
        assert_eq!(
            [
                scores.build,
                scores.crew,
                scores.outlaw,
                scores.speed,
                scores.style
            ],
            [7; 5]
        );
        assert_eq!(thresholds.evaluate(scores).get(Field::Level), 2);
        assert_eq!(
            saved.tables[&persistent::key("RepValuesTable")].rows[&0][&persistent::key("RepLevel")],
            Cell::Int(2)
        );
        assert!(!saved.items.is_empty());
        assert!(saved.garage.is_some());
        crate::bank::set_for_operator(
            &repository,
            account,
            saved.generation,
            Balance(123),
            Timestamp(18),
        )
        .unwrap();
    }
    assert_eq!(prepared.publish(&root.0), Err(Error::DestinationExists));
    let reopened = SqliteRepository::open_owned_directory(&root.0).unwrap();
    let saved = reopened.snapshot(account).unwrap();
    assert_eq!(Balance::from_snapshot(&saved), Ok(Balance(123)));
    assert_eq!(
        thresholds
            .evaluate(Scores::load(&tables.view(&saved).unwrap()).unwrap())
            .get(Field::Level),
        2
    );
    for invalid in [0, 4] {
        assert!(
            prepare_with(32, &tables, &[])
                .unwrap()
                .with_starting_resources(&tables, &thresholds, invalid, Balance(10_000_000))
                .is_err()
        );
    }
}
/// The synthetic table catalog with the real objective columns.
fn objective_tables() -> persistent::Catalog {
    let mut v = persistent::tests::content();
    v["tables"][0]["columns"] = json!([
        {"name":ACTIVE_COLUMN,"key":persistent::key(ACTIVE_COLUMN),"value_type":"int","default":""},
        {"name":COMPLETED_COLUMN,"key":persistent::key(COMPLETED_COLUMN),"value_type":"int","default":""}
    ]);
    persistent::Catalog::from_json(&v).unwrap()
}
fn prepare_with(
    label: u8,
    tables: &persistent::Catalog,
    rows: &[ObjectiveRow],
) -> Result<Prepared, Error> {
    Prepared::new(
        "Fresh Driver",
        [label; SEED_BYTES],
        Timestamp(17),
        &Policy::from_json(&policy()).unwrap(),
        &catalog(),
        tables,
        rows,
    )
}

#[test]
fn saved_objective_rows_are_published_with_both_cells_and_validated() {
    let rows = [
        ObjectiveRow {
            id: 10,
            active: false,
            completed: true,
        },
        ObjectiveRow {
            id: 30,
            active: true,
            completed: false,
        },
    ];
    let root = Root::new();
    let prepared = prepare_with(5, &objective_tables(), &rows).unwrap();
    let account = prepared.identity().storage_account();
    prepared.publish(&root.0).unwrap();
    let repository = SqliteRepository::open_owned_directory(&root.0).unwrap();
    let saved = repository.snapshot(account).unwrap();
    assert_eq!(saved.generation, 1);
    let objectives = persistent::key(OBJECTIVES_TABLE);
    let table = &saved.tables[&objectives];
    assert_eq!(table.rows.len(), 2);
    for row in rows {
        let cells = &table.rows[&row.id];
        assert_eq!(cells.len(), 2);
        assert_eq!(
            cells[&persistent::key(ACTIVE_COLUMN)],
            Cell::Int(row.active.into())
        );
        assert_eq!(
            cells[&persistent::key(COMPLETED_COLUMN)],
            Cell::Int(row.completed.into())
        );
    }
    // The same catalog reopens the saved rows as present cells; absent
    // objectives have none.
    let loaded = objective_tables().view(&saved).unwrap();
    assert!(loaded.has_cell(objectives, 30, persistent::key(ACTIVE_COLUMN)));
    assert!(loaded.has_cell(objectives, 10, persistent::key(COMPLETED_COLUMN)));
    assert!(!loaded.has_cell(objectives, 20, persistent::key(ACTIVE_COLUMN)));
    // Without rows the table stays asset-default and empty, as before.
    let fresh = prepare(6);
    let fresh_table = fresh
        .batch
        .ops
        .iter()
        .find_map(|op| match op {
            Op::SetTable(key, table) if *key == objectives => Some(table),
            _ => None,
        })
        .unwrap();
    assert!(fresh_table.rows.is_empty());
    // Duplicate or zero IDs, columns the schema lacks and too many rows fail
    // before anything is published.
    let duplicate = [rows[0], rows[0]];
    assert_eq!(
        prepare_with(7, &objective_tables(), &duplicate).err(),
        Some(Error::Progression)
    );
    let zero = [ObjectiveRow {
        id: 0,
        active: true,
        completed: false,
    }];
    assert_eq!(
        prepare_with(7, &objective_tables(), &zero).err(),
        Some(Error::Progression)
    );
    assert_eq!(
        prepare_with(7, &persistent::tests::catalog(), &rows).err(),
        Some(Error::Progression)
    );
    let many: Vec<_> = (1..=MAX_ROWS as u64 + 1)
        .map(|id| ObjectiveRow {
            id,
            active: false,
            completed: true,
        })
        .collect();
    assert_eq!(
        prepare_with(7, &objective_tables(), &many).err(),
        Some(Error::Bounds)
    );
}

#[test]
fn generated_account_reopens_with_owned_items_default_progression_and_no_profile_inputs() {
    let root = Root::new();
    let prepared = prepare(3);
    let account = prepared.identity().storage_account();
    prepared.publish(&root.0).unwrap();
    let state = account_state::open(&root.0, account, &Default::default()).unwrap();
    assert_eq!(state.identity.persona_id(), prepared.identity.persona_id());
    assert_ne!(state.identity.persona_id(), state.identity.account_id());
    assert_eq!(state.kickback.counters(), (0, 20));
    assert!(state.kickback.winner().is_none());
    assert!(state.speedwall.rows().is_empty());
    assert_eq!(state.settings.current().unwrap(), Settings::default());
    let repository = SqliteRepository::open_owned_directory(&root.0).unwrap();
    let saved = repository.snapshot(account).unwrap();
    assert_eq!(saved.generation, 1);
    assert_eq!(saved.updated_at, Timestamp(17));
    assert_eq!(saved.items.len(), 1);
    assert_eq!(saved.garage.unwrap().0[0].unwrap().get(), 1);
    assert_eq!(saved.tables.len(), 25);
    // The first menu reads must not collide with the creator's transaction or
    // require another seed transaction. This regresses the initial batch-ID
    // collision with awards and proves both service domains were published.
    assert_eq!(
        crate::awards::ensure_empty(&repository, account, saved.clone(), Timestamp(27)).unwrap(),
        saved
    );
    assert_eq!(
        crate::challenges::ensure_empty(&repository, account, saved.clone(), Timestamp(28))
            .unwrap(),
        saved
    );
    assert_eq!(
        crate::awards::State::load(&saved).unwrap(),
        crate::awards::State::default()
    );
    crate::challenges::Current::from_snapshot(&saved).unwrap();
    let loaded = inventory::load(&repository, account, &catalog(), 99, Timestamp(29)).unwrap();
    assert_eq!(loaded, saved);
    assert_eq!(
        persistent::tests::catalog()
            .ensure_loaded(&repository, account, loaded, Timestamp(30))
            .unwrap()
            .0,
        saved
    );
    assert_eq!(prepared.publish(&root.0), Err(Error::DestinationExists));
    assert_eq!(repository.snapshot(account).unwrap(), saved);
}

#[test]
fn independent_entropy_changes_identities_and_grant_ids_without_shared_progress() {
    let one = prepare(4);
    let two = prepare(5);
    assert_ne!(
        one.identity.storage_account(),
        two.identity.storage_account()
    );
    assert_ne!(one.identity.persona_id(), two.identity.persona_id());
    assert_ne!(one.identity.account_id(), two.identity.account_id());
    let grant_id = |p: &Prepared| -> u64 {
        let document = p
            .documents
            .iter()
            .find(|(kind, _)| *kind == Kind::Entitlements)
            .unwrap();
        serde_json::from_slice::<Value>(&document.1.bytes).unwrap()["grants"][0]["id"]
            .as_u64()
            .unwrap()
    };
    assert_ne!(grant_id(&one), grant_id(&two));
    let repeat = prepare(4);
    assert_eq!(repeat.documents, one.documents);
}

#[test]
fn account_bearing_policy_and_invalid_identity_inputs_are_rejected_before_publishing() {
    let mut v = policy();
    v["account"] = json!("an-existing-account");
    assert!(matches!(Policy::from_json(&v), Err(Error::Policy)));
    v = policy();
    v["entitlements"]["grants"][0]["id"] = json!(13);
    assert!(matches!(Policy::from_json(&v), Err(Error::Policy)));
    v = policy();
    v["entitlements"]["grants"][0]["persona_id"] = json!(123);
    assert!(matches!(Policy::from_json(&v), Err(Error::Policy)));
    let policy = Policy::from_json(&policy()).unwrap();
    for name in ["", "bad\nname", &"é".repeat(17)] {
        assert!(matches!(
            Prepared::new(
                name,
                [1; SEED_BYTES],
                Timestamp(1),
                &policy,
                &catalog(),
                &persistent::tests::catalog(),
                &[]
            ),
            Err(Error::Identity)
        ));
    }
    assert!(matches!(
        Prepared::new(
            "Driver",
            [0; SEED_BYTES],
            Timestamp(1),
            &policy,
            &catalog(),
            &persistent::tests::catalog(),
            &[]
        ),
        Err(Error::Identity)
    ));
}
