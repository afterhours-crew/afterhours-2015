// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{InventoryRepository, SqliteRepository};
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, Layout, OwnershipLevel,
};
use std::path::PathBuf;

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("fresh-account-{}-{stamp}", std::process::id())))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
    Prepared::new(
        "Fresh Driver",
        [label; SEED_BYTES],
        Timestamp(17),
        &Policy::from_json(&policy()).unwrap(),
        &catalog(),
        &persistent::tests::catalog(),
    )
    .unwrap()
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
                &persistent::tests::catalog()
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
            &persistent::tests::catalog()
        ),
        Err(Error::Identity)
    ));
}
