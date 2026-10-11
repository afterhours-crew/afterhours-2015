// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use nfs_storage::{
    AccountId, Error, InventoryRepository, SqliteRepository,
    account::{Document, Kind, Repository},
    settings,
};
use std::path::PathBuf;
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("account-storage-{}-{stamp}", std::process::id())))
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
fn doc(revision: u64, bytes: &[u8]) -> Document {
    Document {
        revision,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn fresh_account_domains_and_inventory_publish_atomically_and_cannot_reset_state() {
    use nfs_storage::{Batch, GarageSlots, ItemId, Op, Timestamp};
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    let a = account(1);
    let documents: Vec<_> = Kind::ALL
        .into_iter()
        .map(|k| (k, doc(1, b"owned")))
        .collect();
    let mut batch = Batch {
        id: 17,
        expected_generation: 0,
        ops: vec![Op::SetGarage(GarageSlots([
            Some(ItemId::new(7).unwrap()),
            None,
            None,
            None,
            None,
        ]))],
    };
    assert_eq!(
        repo.initialize_fresh(a, &documents, &batch, Timestamp(1)),
        Err(Error::DanglingReference)
    );
    for kind in Kind::ALL {
        assert!(repo.read(a, kind).unwrap().is_none());
    }
    let inventory = SqliteRepository::open_owned_directory(&root.0).unwrap();
    assert_eq!(inventory.snapshot(a).unwrap().generation, 0);
    batch.ops = vec![Op::SetGarage(GarageSlots::default())];
    repo.initialize_fresh(a, &documents, &batch, Timestamp(2))
        .unwrap();
    let saved = inventory.snapshot(a).unwrap();
    assert_eq!(saved.generation, 1);
    for kind in Kind::ALL {
        assert_eq!(repo.read(a, kind).unwrap(), Some(doc(1, b"owned")));
    }
    assert_eq!(
        repo.initialize_fresh(a, &documents, &batch, Timestamp(3)),
        Err(Error::Conflict)
    );
    assert_eq!(inventory.snapshot(a).unwrap(), saved);
    assert_eq!(
        repo.initialize_fresh(account(2), &documents[..4], &batch, Timestamp(3)),
        Err(Error::Invalid)
    );
    // A partially imported account is also ineligible, even at generation zero.
    let b = account(3);
    repo.initialize(b, &[(Kind::Settings, doc(1, b"previous"))])
        .unwrap();
    assert_eq!(
        repo.initialize_fresh(b, &documents, &batch, Timestamp(3)),
        Err(Error::Conflict)
    );
    assert!(repo.read(b, Kind::Identity).unwrap().is_none());
    assert_eq!(
        repo.read(b, Kind::Settings).unwrap(),
        Some(doc(1, b"previous"))
    );
}

#[test]
fn domains_share_account_file_preserve_inventory_and_ignore_reimport() {
    let root = Root::new();
    let inventory = SqliteRepository::open_owned_directory(&root.0).unwrap();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    let a = account(1);
    let saved = inventory.open(a).unwrap();
    let documents: Vec<_> = Kind::ALL
        .into_iter()
        .map(|k| (k, doc(7, b"owned")))
        .collect();
    repo.initialize(a, &documents).unwrap();
    assert_eq!(repo.path(a), inventory.path(a));
    assert_eq!(inventory.snapshot(a).unwrap(), saved);
    drop(repo);
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    for kind in Kind::ALL {
        assert_eq!(repo.read(a, kind).unwrap(), Some(doc(7, b"owned")));
        assert!(!repo.compare_exchange(a, kind, Some(6), b"stale").unwrap());
        assert!(repo.compare_exchange(a, kind, Some(7), b"next").unwrap());
    }
    repo.initialize(a, &documents).unwrap();
    repo.initialize(account(2), &documents).unwrap();
    for kind in Kind::ALL {
        assert_eq!(repo.read(a, kind).unwrap(), Some(doc(8, b"next")));
        assert_eq!(repo.read(account(2), kind).unwrap(), Some(doc(7, b"owned")));
    }
    std::fs::copy(repo.path(a), repo.path(account(3))).unwrap();
    assert_eq!(repo.prepare(account(3)), Err(Error::Identity));
    assert_eq!(repo.read(account(3), Kind::Identity), Err(Error::Identity));
}

#[test]
fn import_rolls_back_all_domains_on_a_later_database_error() {
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    let a = account(1);
    repo.initialize(a, &[(Kind::Settings, doc(3, b"settings"))])
        .unwrap();
    let conn = rusqlite::Connection::open(repo.path(a)).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE user_settings SET value=X'';")
        .unwrap();
    drop(conn);
    assert_eq!(
        repo.initialize(
            a,
            &[
                (Kind::Identity, doc(1, b"identity")),
                (Kind::Settings, doc(1, b"seed"))
            ]
        ),
        Err(Error::Config)
    );
    assert!(repo.read(a, Kind::Identity).unwrap().is_none());
}

#[test]
fn every_domain_enforces_its_size_and_revision_bound_before_writing() {
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    let a = account(1);
    repo.prepare(a).unwrap();
    for kind in Kind::ALL {
        for invalid in [
            doc(0, b"x"),
            doc(u64::MAX, b"x"),
            doc(1, b""),
            doc(1, &vec![0; kind.max_bytes() + 1]),
        ] {
            assert_eq!(repo.initialize(a, &[(kind, invalid)]), Err(Error::Bounds));
        }
        let data = vec![0xab; kind.max_bytes()];
        repo.initialize(a, &[(kind, doc(i64::MAX as u64, &data))])
            .unwrap();
        assert_eq!(repo.read(a, kind).unwrap().unwrap().bytes, data);
        assert_eq!(
            repo.compare_exchange(a, kind, Some(i64::MAX as u64), b"overflow"),
            Err(Error::Bounds)
        );
    }
    assert_eq!(
        repo.initialize(
            a,
            &[
                (Kind::Identity, doc(1, b"x")),
                (Kind::Identity, doc(1, b"y"))
            ]
        ),
        Err(Error::Invalid)
    );
}

#[test]
fn legacy_settings_import_preserves_revision_and_source_and_rejects_wrong_owner() {
    let root = Root::new();
    let repo = settings::Repository::open_owned_directory(&root.0).unwrap();
    let a = account(1);
    let old = root.0.join(format!("settings-{}.sqlite", a.hex()));
    let conn = rusqlite::Connection::open(&old).unwrap();
    conn.execute_batch("CREATE TABLE settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1),account BLOB NOT NULL,revision INTEGER NOT NULL,value BLOB NOT NULL) STRICT; PRAGMA application_id=1313231696; PRAGMA user_version=1;").unwrap();
    conn.execute(
        "INSERT INTO settings VALUES(1,?1,19,?2)",
        rusqlite::params![a.bytes().as_slice(), b"legacy".as_slice()],
    )
    .unwrap();
    drop(conn);
    let original = std::fs::read(&old).unwrap();
    let document = repo.read_legacy(a).unwrap().unwrap();
    assert_eq!(document, doc(19, b"legacy"));
    repo.initialize(a, document).unwrap();
    assert!(repo.compare_exchange(a, Some(19), b"changed").unwrap());
    repo.initialize(a, repo.read_legacy(a).unwrap().unwrap())
        .unwrap();
    assert_eq!(repo.read(a).unwrap(), Some(doc(20, b"changed")));
    assert_eq!(std::fs::read(&old).unwrap(), original);
    std::fs::copy(
        old,
        root.0.join(format!("settings-{}.sqlite", account(2).hex())),
    )
    .unwrap();
    assert_eq!(repo.read_legacy(account(2)), Err(Error::Identity));
}
