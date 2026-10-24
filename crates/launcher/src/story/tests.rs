// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{records, session::Lock, testing};
use nfs_services::persistent::key;
use nfs_storage::{
    AccountId, Batch, InventoryRepository, Op, SqliteRepository, Timestamp, tables::Table,
};
use serde_json::json;

fn setup() -> (
    std::path::PathBuf,
    std::path::PathBuf,
    SqliteRepository,
    AccountId,
) {
    let data = testing::scratch("story-read");
    let directory = data.join("artifacts").join("account");
    let repo = SqliteRepository::open_owned_directory(&directory).unwrap();
    let account = AccountId::from_owned_config([21; 16]).unwrap();
    repo.open(account).unwrap();
    repo.apply(
        account,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: vec![
                Op::SetTable(key("ProgressionObjective"), Table::default()),
                Op::SetTable(key("ProgressionStats"), Table::default()),
            ],
        },
        Timestamp(1),
    )
    .unwrap();
    records::write_new_json(
        &data.join("account.json"),
        &records::AccountRecord::post_intro(account.hex()),
    )
    .unwrap();
    let content = data.join("content");
    std::fs::create_dir(&content).unwrap();
    std::fs::write(content.join("progression-content.json"),serde_json::to_vec(&json!({
        "format":"nfs-progression-entities","version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "restoration":{"version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"source":{"path":"constructed","sha256":"00".repeat(32)},
        "branches":[{"name":"main","objectives":[{"id":1,"active":{"completed":9007199254740993u64}}],"ints":[]},
        {"name":"intro","objectives":[{"id":9007199254740993u64,"label":"Authored first step","active":{"constant":true}}],"ints":[]}]},
        "settings":{"consumer_beta":false}
    })).unwrap()).unwrap();
    (data, content, repo, account)
}

#[test]
fn report_reads_saved_rules_and_exact_ids_without_writing_and_while_session_runs() {
    let (data, content, repo, account) = setup();
    let before = repo.snapshot(account).unwrap();
    let view = report(&data, None, Some(&content)).unwrap();
    assert_eq!(view.current_checkpoint, "9007199254740993");
    assert_eq!(view.generation, "1");
    assert_eq!(view.checkpoints[0].label, "Authored first step");
    assert!(!view.session_running && !view.can_edit && !view.checkpoints[0].selectable);
    let lock = Lock::acquire(&data).unwrap();
    assert!(report(&data, None, Some(&content)).unwrap().session_running);
    assert_eq!(repo.snapshot(account).unwrap(), before);
    crate::contract::testing::validate("story", &view);
    drop(lock);
    drop(repo);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn missing_bad_or_oversized_catalog_never_changes_the_account() {
    let (data, content, repo, account) = setup();
    let before = repo.snapshot(account).unwrap();
    let file = content.join("progression-content.json");
    for bytes in [b"{}".to_vec(), vec![b' '; 512 * 1024 + 1]] {
        std::fs::write(&file, bytes).unwrap();
        assert!(report(&data, None, Some(&content)).is_err());
        assert_eq!(repo.snapshot(account).unwrap(), before);
    }
    std::fs::remove_file(&file).unwrap();
    assert!(report(&data, None, Some(&content)).is_err());
    drop(repo);
    std::fs::remove_dir_all(data).unwrap();
}
