// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_storage::{
    AccountId, Error,
    settings::{MAX_BYTES, Repository},
};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("settings-storage-{}-{stamp}", std::process::id())))
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

#[test]
fn revisions_restart_stale_writes_and_account_identity() {
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    assert_eq!(repo.read(account(1)).unwrap(), None);
    assert!(repo.compare_exchange(account(1), None, b"one").unwrap());
    assert!(!repo.compare_exchange(account(1), None, b"lost").unwrap());
    assert!(repo.compare_exchange(account(1), Some(1), b"two").unwrap());
    assert!(
        !repo
            .compare_exchange(account(1), Some(1), b"stale")
            .unwrap()
    );
    let reopened = Repository::open_owned_directory(&root.0).unwrap();
    let state = reopened.read(account(1)).unwrap().unwrap();
    assert_eq!(
        (state.revision, state.bytes.as_slice()),
        (2, b"two".as_slice())
    );
    assert!(
        reopened
            .compare_exchange(account(2), None, b"separate")
            .unwrap()
    );
    std::fs::copy(repo.path(account(1)), repo.path(account(3))).unwrap();
    assert_eq!(repo.read(account(3)), Err(Error::Identity));
    assert_eq!(
        repo.compare_exchange(account(3), Some(2), b"wrong"),
        Err(Error::Identity)
    );
    assert_eq!(repo.read(account(2)).unwrap().unwrap().bytes, b"separate");
}

#[test]
fn concurrent_compare_exchange_has_one_winner() {
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    repo.compare_exchange(account(1), None, b"start").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|i| {
            let repo = repo.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                repo.compare_exchange(account(1), Some(1), &[i]).unwrap()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| usize::from(w.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(repo.read(account(1)).unwrap().unwrap().revision, 2);
}

#[test]
fn bounded_data_and_foreign_schema_fail_without_modification() {
    let root = Root::new();
    let repo = Repository::open_owned_directory(&root.0).unwrap();
    assert_eq!(
        repo.compare_exchange(account(1), None, &[]),
        Err(Error::Bounds)
    );
    assert_eq!(
        repo.compare_exchange(account(1), None, &vec![1; MAX_BYTES + 1]),
        Err(Error::Bounds)
    );
    repo.compare_exchange(account(1), None, b"preserved")
        .unwrap();
    let path = repo.path(account(1));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version=99").unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(repo.read(account(1)), Err(Error::Version));
    assert_eq!(
        repo.compare_exchange(account(1), Some(1), b"bad"),
        Err(Error::Version)
    );
    assert_eq!(std::fs::read(path).unwrap(), before);
}
