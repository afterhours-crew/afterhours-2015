// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_services::user_settings::{
    Change, Error, MAX_ENTRIES, Session, Settings, Store, body_limits, frame_limits,
};
use nfs_storage::AccountId;
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let name = format!(
            "settings-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        Self(std::env::temp_dir().join(name))
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
fn store(root: &Root, n: u8) -> Store {
    Store::open(Settings::default(), Some((root.0.clone(), account(n)))).unwrap()
}
fn change(key: &[u8], n: u32) -> Change {
    Change::Preferences(vec![(key.to_vec(), n)], vec![])
}
fn save(value: &[u8]) -> Vec<u8> {
    let body = nfs_protocol::util::UserSettingsSaveRequest {
        key: Some(b"option"),
        data: Some(value),
        user_id: Some(0),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                routing_a: 9,
                routing_b: 11,
                ..Default::default()
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .unwrap()
}

#[test]
fn separate_connections_merge_keys_and_restart_without_lost_updates() {
    let root = Root::new();
    let first = store(&root, 1);
    let second = store(&root, 1);
    let stale_first = first.current().unwrap();
    let stale_second = second.current().unwrap();
    assert_eq!(stale_first, stale_second);
    let barrier = Arc::new(Barrier::new(2));
    let gate = barrier.clone();
    let worker = std::thread::spawn(move || {
        gate.wait();
        first.apply(&change(b"one", 1)).unwrap();
    });
    barrier.wait();
    second.apply(&change(b"two", 2)).unwrap();
    worker.join().unwrap();
    let restarted = store(&root, 1);
    let mut values = restarted.current().unwrap().integers;
    values.sort();
    assert_eq!(values, vec![(b"one".to_vec(), 1), (b"two".to_vec(), 2)]);
    restarted.apply(&change(b"one", 3)).unwrap();
    assert_eq!(
        second
            .current()
            .unwrap()
            .integers
            .iter()
            .find(|(k, _)| k == b"one")
            .unwrap()
            .1,
        3
    );
    assert!(store(&root, 2).current().unwrap().integers.is_empty());
}

#[test]
fn failed_or_partial_delta_does_not_modify_state() {
    let root = Root::new();
    let store = store(&root, 1);
    store.apply(&change(b"original", 9)).unwrap();
    let before = store.current().unwrap();
    let invalid = Change::Preferences(vec![(b"original".to_vec(), 2), (vec![], 3)], vec![]);
    assert_eq!(store.apply(&invalid), Err(Error::Config));
    assert_eq!(store.current().unwrap(), before);
    let mut full = Settings {
        integers: (0..MAX_ENTRIES)
            .map(|n| (format!("k{n}").into_bytes(), 0))
            .collect(),
        ..Default::default()
    };
    let snapshot = full.clone();
    assert_eq!(
        Change::Preferences(vec![(b"k0".to_vec(), 1), (b"overflow".to_vec(), 1)], vec![])
            .apply(&mut full),
        Err(Error::Bounds)
    );
    assert_eq!(full, snapshot);
    let path = nfs_storage::settings::Repository::open_owned_directory(&root.0)
        .unwrap()
        .path(account(1));
    let moved = path.with_extension("backup");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(store.apply(&change(b"original", 1)), Err(Error::Storage));
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(moved, path).unwrap();
    assert_eq!(store.current().unwrap(), before);
}

#[test]
fn durable_save_survives_lost_reply_and_identical_retry_does_not_rewrite() {
    let root = Root::new();
    let store = store(&root, 1);
    let mut session = Session::new(123, store.current().unwrap()).unwrap();
    session.reply(&save(b"new")).unwrap().unwrap();
    let durable = store.apply(&session.pending().unwrap()).unwrap();
    session.refresh(durable.clone()).unwrap();
    // Network output fails after commit. The success may be lost, the save is not.
    session.abort();
    drop(session);
    let repo = nfs_storage::settings::Repository::open_owned_directory(&root.0).unwrap();
    let before = repo.read(account(1)).unwrap().unwrap();
    let mut retry = Session::new(123, store.current().unwrap()).unwrap();
    retry.reply(&save(b"new")).unwrap().unwrap();
    assert_eq!(store.apply(&retry.pending().unwrap()).unwrap(), durable);
    assert_eq!(repo.read(account(1)).unwrap().unwrap(), before);
}

#[test]
fn malformed_frames_and_pending_request_cannot_publish_changes() {
    let wire = save(b"valid");
    for end in 0..wire.len() {
        let mut session = Session::new(123, Settings::default()).unwrap();
        assert!(session.reply(&wire[..end]).is_err());
        assert_eq!(session.pending(), None);
    }
    let mut session = Session::new(123, Settings::default()).unwrap();
    assert!(
        session
            .reply(&[wire.clone(), wire.clone()].concat())
            .is_err()
    );
    session.reply(&wire).unwrap().unwrap();
    assert_eq!(session.reply(&wire), Err(Error::Phase));
    assert_eq!(session.pending(), None);
    assert!(Session::new(0, Settings::default()).is_err());
    let mut value = Settings::default().to_json();
    value["strings"] = serde_json::json!([[{"hex":"éé"}, "x"]]);
    assert_eq!(Settings::from_json(&value), Err(Error::Config));
}
