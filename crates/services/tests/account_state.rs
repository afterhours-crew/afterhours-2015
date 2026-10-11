// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_services::{
    account_state::{self, Error, Imports},
    user_settings::{Change, Settings},
};
use nfs_storage::{
    AccountId,
    account::{Kind, Repository},
};
use serde_json::{Value, json};
use std::path::PathBuf;
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = Self(
            std::env::temp_dir().join(format!("account-services-{}-{stamp}", std::process::id())),
        );
        std::fs::create_dir_all(&root.0).unwrap();
        root
    }
    fn file(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn documents(a: AccountId) -> [Value; 5] {
    let build = nfs_services::SUPPORTED_BUILD_SHA256;
    [
        json!({"version":1,"storage_account":a.hex(),"persona":123,"account":456,"name":"Synthetic Driver"}),
        json!({"format":"nfs-entitlement-state","version":1,"build_sha256":build,"account":a.hex(),"scopes":[["synthetic"]],"grants":[
            {"device_uri":"","grant_date":"synthetic","group_name":"synthetic","id":u64::MAX,"is_consumable":false,"persona_id":0,"project_id":"project","product_catalog":1,"product_id":"product","status":1,"status_reason_code":0,"entitlement_tag":"license","termination_date":"","entitlement_type":5,"use_count":1,"version":2}
        ]}),
        json!({"format":"nfs-kickback-state","version":1,"build_sha256":build,"account":a.hex(),"screenshot_count":3,"screenshot_count_max":9,"gallery":"empty","winner":{
            "datetime":123456,"title":"Synthetic tile","screenshot_id":9001,"kickback_count":5,"persona_id":789,"player_provided_kickback":true,"record_name":"test-record","screenshot_type":1
        }}),
        json!({"format":"nfs-speedwall-state","version":1,"build_sha256":build,"account":a.hex(),"rows":[{"speed_wall_id":5,"stat_type":0,"rank":2,"rating_bits":0x43200000,"total_count":4,"integers":[["wins",7]],"floats":[],"strings":null}]}),
        Settings::default().to_json(),
    ]
}
fn imports(root: &Root, docs: &[Value; 5]) -> Imports {
    Imports {
        identity: Some(root.file("identity.json", &docs[0])),
        entitlements: Some(root.file("grants.json", &docs[1])),
        kickback: Some(root.file("kickback.json", &docs[2])),
        speedwall: Some(root.file("speedwall.json", &docs[3])),
        settings: Some(root.file("settings.json", &docs[4])),
    }
}

#[test]
fn imported_state_reopens_without_files_and_settings_share_the_account_database() {
    let root = Root::new();
    let a = account(1);
    let docs = documents(a);
    let seeds = imports(&root, &docs);
    let store = account_state::open(&root.0, a, &seeds).unwrap();
    assert_eq!(store.identity.persona_id(), 123);
    assert_eq!(store.identity.account_id(), 456);
    assert_eq!(store.kickback.counters(), (3, 9));
    assert_eq!(store.kickback.winner().unwrap().screenshot_id, 9001);
    assert_eq!(store.speedwall.rows()[0].integers, vec![("wins".into(), 7)]);
    store
        .settings
        .apply(&Change::Preferences(vec![(b"mode".to_vec(), 42)], vec![]))
        .unwrap();
    drop(store);
    for path in [
        seeds.identity,
        seeds.entitlements,
        seeds.kickback,
        seeds.speedwall,
        seeds.settings,
    ]
    .into_iter()
    .flatten()
    {
        std::fs::remove_file(path).unwrap();
    }
    let reopened = account_state::open(&root.0, a, &Imports::default()).unwrap();
    assert_eq!(
        reopened.settings.current().unwrap().integers,
        vec![(b"mode".to_vec(), 42)]
    );
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    for (kind, original) in Kind::ALL.into_iter().zip(docs) {
        let saved = repo.read(a, kind).unwrap().unwrap();
        if kind != Kind::Settings {
            assert_eq!(
                serde_json::from_slice::<Value>(&saved.bytes).unwrap(),
                original
            );
        }
    }
    assert!(!root.0.join(format!("settings-{}.sqlite", a.hex())).exists());
}

#[test]
fn one_invalid_or_missing_domain_prevents_all_state_imports() {
    let root = Root::new();
    let a = account(1);
    let mut docs = documents(a);
    docs[3]["rows"][0]["stat_type"] = json!(9);
    let mut seeds = imports(&root, &docs);
    assert!(matches!(
        account_state::open(&root.0, a, &seeds),
        Err(Error::Invalid(Kind::Speedwall))
    ));
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    for kind in Kind::ALL {
        assert!(repo.read(a, kind).unwrap().is_none());
    }
    seeds = imports(&root, &documents(a));
    seeds.settings = None;
    assert!(matches!(
        account_state::open(&root.0, a, &seeds),
        Err(Error::Missing(Kind::Settings))
    ));
    for kind in Kind::ALL {
        assert!(repo.read(a, kind).unwrap().is_none());
    }
}

#[test]
fn foreign_identity_or_grants_are_rejected_without_adoption() {
    let root = Root::new();
    let a = account(1);
    let mut docs = documents(a);
    docs[0]["storage_account"] = json!(account(2).hex());
    let seeds = imports(&root, &docs);
    assert!(matches!(
        account_state::open(&root.0, a, &seeds),
        Err(Error::Invalid(Kind::Identity))
    ));
    docs = documents(a);
    docs[1]["account"] = json!(account(2).hex());
    let seeds = imports(&root, &docs);
    assert!(matches!(
        account_state::open(&root.0, a, &seeds),
        Err(Error::Invalid(Kind::Entitlements))
    ));
}

#[test]
fn database_wins_over_changed_imports_and_legacy_json_and_invalid_state_is_not_reseeded() {
    let root = Root::new();
    let a = account(1);
    let mut docs = documents(a);
    let seeds = imports(&root, &docs);
    account_state::open(&root.0, a, &seeds).unwrap();
    docs[0]["persona"] = json!(999);
    docs[2]["screenshot_count"] = json!(0);
    let seeds = imports(&root, &docs);
    root.file(
        &format!("user-settings-{}.json", a.hex()),
        &json!({"invalid":"ignored"}),
    );
    let loaded = account_state::open(&root.0, a, &seeds).unwrap();
    assert_eq!(loaded.identity.persona_id(), 123);
    assert_eq!(loaded.kickback.counters(), (3, 9));
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    repo.compare_exchange(a, Kind::Speedwall, Some(1), b"{}")
        .unwrap();
    assert!(matches!(
        account_state::open(&root.0, a, &seeds),
        Err(Error::Invalid(Kind::Speedwall))
    ));
}
