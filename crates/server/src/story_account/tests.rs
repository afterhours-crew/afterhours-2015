// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_storage::{Batch, DefinitionGuid, GarageSlots, ItemId, ItemRecord, Op};
mod restart;

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog_json() -> serde_json::Value {
    json!({"version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "source":{"path":"constructed","sha256":"00".repeat(32)},"branches":[
        {"name":"intro","objectives":[
            {"id":7,"active":{"completed":9007199254740993u64}},
            {"id":9007199254740993u64,"active":{"constant":true}},
            {"id":21,"active":{"completed":7}}],"ints":[]},
        {"name":"main","objectives":[{"id":30,"active":{"completed":21}}],"ints":[]}]})
}
struct Fixture {
    root: PathBuf,
    rules: Rules,
    repo: SqliteRepository,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "story-editor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("artifacts")).unwrap();
        let repo = SqliteRepository::open_owned_directory(&root.join("artifacts/account")).unwrap();
        for who in [account(1), account(2)] {
            repo.open(who).unwrap();
            let item = ItemRecord {
                id: ItemId::new(3).unwrap(),
                definition: DefinitionGuid([9; 16]),
                owner: None,
                state: 2,
                buy_price: 500,
                sell_price: 300,
                default_items: vec![],
                sub_items: vec![],
                derived: vec![4, 5, 6],
            };
            repo.apply(
                who,
                &Batch {
                    id: 1,
                    expected_generation: 0,
                    ops: vec![
                        Op::SetTable(
                            key("ProgressionObjective"),
                            Table {
                                rows: BTreeMap::from([(
                                    30,
                                    Row::from([
                                        (key("Active"), Value::Int(1)),
                                        (key("Completed"), Value::Int(1)),
                                    ]),
                                )]),
                            },
                        ),
                        Op::SetTable(key("ProgressionStats"), Table::default()),
                        nfs_services::bank::Balance(12345).store(),
                        Op::SetTable(
                            key("Reputation"),
                            Table {
                                rows: BTreeMap::from([(0, Row::from([(1, Value::Int(456))]))]),
                            },
                        ),
                        Op::Insert(item.clone()),
                        Op::SetGarage(GarageSlots([Some(item.id), None, None, None, None])),
                    ],
                },
                Timestamp(1),
            )
            .unwrap();
        }
        Self {
            root,
            rules: Rules {
                catalog: Catalog::from_json(&catalog_json()).unwrap(),
                settings: Settings {
                    consumer_beta: false,
                },
                digest: digest(b"constructed"),
                options: None,
                items: None,
                persistent: None,
            },
            repo,
        }
    }
    fn editor(&self, supported: bool) -> Editor<'_> {
        Editor {
            root: &self.root,
            account: account(1),
            rules: &self.rules,
            supported,
            starter: None,
            restart: false,
        }
    }
    fn proof(&self, target: Checkpoint) -> Proof {
        let preview = self.editor(true).preview(target).unwrap();
        Proof {
            target,
            expected: decimal(&preview.generation).unwrap(),
            plan_id: preview.plan_id,
        }
    }
    fn backups(&self) -> usize {
        std::fs::read_dir(self.root.join("artifacts/story-backups"))
            .map(|e| e.count())
            .unwrap_or(0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn preview_is_read_only_ordered_and_keeps_identifiers_exact() {
    let f = Fixture::new();
    let before = std::fs::read(f.repo.path(account(1))).unwrap();
    let p = f.editor(true).preview(Checkpoint::Objective(21)).unwrap();
    assert_eq!(
        p.completes
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["9007199254740993", "7"]
    );
    assert_eq!(p.source_checkpoint, "9007199254740993");
    assert!(p.can_apply);
    assert!(p.reopens.is_empty());
    assert_eq!(std::fs::read(f.repo.path(account(1))).unwrap(), before);
    assert_eq!(f.backups(), 0);
    assert!(!f.root.join("launcher.lock").exists());
    assert!(
        !f.repo
            .path(account(1))
            .with_extension("writer.lock")
            .exists()
    );
    let gated = f.editor(false).preview(Checkpoint::AfterIntro).unwrap();
    assert!(!gated.can_apply);
    assert_eq!(gated.reasons.len(), 1);
    assert_ne!(
        gated.plan_id,
        f.editor(true)
            .preview(Checkpoint::AfterIntro)
            .unwrap()
            .plan_id
    );
}

#[test]
fn edit_has_a_standalone_backup_preserves_other_domains_and_exact_retry_is_inert() {
    let f = Fixture::new();
    let before = f.repo.inspect(account(1)).unwrap();
    let other = f.repo.inspect(account(2)).unwrap();
    let proof = f.proof(Checkpoint::AfterIntro);
    let editor = f.editor(true);
    let saved = editor.apply(&proof, "first", Timestamp(2)).unwrap();
    assert!(!saved.replayed);
    assert_eq!(f.backups(), 1);
    editor.verify_backup(&saved.backup, &proof.plan_id).unwrap();
    let after = f.repo.inspect(account(1)).unwrap();
    assert_eq!(after.snapshot.generation, before.snapshot.generation + 1);
    assert_eq!(after.snapshot.items, before.snapshot.items);
    assert_eq!(after.snapshot.garage, before.snapshot.garage);
    for name in ["Bank", "Reputation"] {
        assert_eq!(
            after.snapshot.tables.get(&key(name)),
            before.snapshot.tables.get(&key(name))
        );
    }
    assert_eq!(
        after.snapshot.tables[&key("ProgressionObjective")].rows[&30][&key("Completed")],
        Value::Int(1)
    );
    let state = f.rules.policy().inspect(&after.snapshot).unwrap();
    assert_eq!(state.position, 3);
    assert!(
        state
            .milestones
            .iter()
            .all(|m| m.edited && !m.played && !m.reward_claimed)
    );
    let restored = SqliteRepository::open_owned_directory(&f.root.join("restore")).unwrap();
    let backup_dir = f
        .root
        .join("artifacts/story-backups")
        .join(&saved.backup.id);
    std::fs::copy(
        backup_dir.join(format!("{}.sqlite", account(1).hex())),
        restored.path(account(1)),
    )
    .unwrap();
    assert_eq!(restored.inspect(account(1)).unwrap(), before);
    let repeat = editor.apply(&proof, "retry", Timestamp(3)).unwrap();
    assert!(repeat.replayed);
    assert_eq!(repeat.backup, saved.backup);
    assert_eq!(f.backups(), 1);
    assert_eq!(f.repo.inspect(account(1)).unwrap(), after);
    assert_eq!(f.repo.inspect(account(2)).unwrap(), other);
    let rewind = f.proof(Checkpoint::Objective(7));
    let preview = editor.preview(rewind.target).unwrap();
    assert_eq!(
        preview
            .reopens
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["7", "21"]
    );
    editor.apply(&rewind, "rewind", Timestamp(4)).unwrap();
    assert_eq!(
        f.rules
            .policy()
            .inspect(&f.repo.snapshot(account(1)).unwrap())
            .unwrap()
            .position,
        1
    );
    assert!(editor.apply(&proof, "old-retry", Timestamp(5)).is_err());
}

#[test]
fn unsupported_busy_stale_and_mismatched_proofs_never_start_backups() {
    let f = Fixture::new();
    let mut proof = f.proof(Checkpoint::AfterIntro);
    let before = f.repo.inspect(account(1)).unwrap();
    assert!(
        f.editor(false)
            .apply(&proof, "unsupported", Timestamp(2))
            .is_err()
    );
    let held = f.repo.writer_lease(account(1)).unwrap();
    assert!(!f.editor(true).preview(proof.target).unwrap().can_apply);
    assert!(f.editor(true).apply(&proof, "busy", Timestamp(2)).is_err());
    drop(held);
    let held = session_lock(&f.root, true).unwrap();
    assert!(!f.editor(true).preview(proof.target).unwrap().can_apply);
    assert!(
        f.editor(true)
            .apply(&proof, "session", Timestamp(2))
            .is_err()
    );
    drop(held);
    proof.expected += 1;
    assert!(f.editor(true).apply(&proof, "stale", Timestamp(2)).is_err());
    proof.expected -= 1;
    proof.target = Checkpoint::Objective(7);
    assert!(
        f.editor(true)
            .apply(&proof, "target", Timestamp(2))
            .is_err()
    );
    let changed = Rules {
        catalog: f.rules.catalog.clone(),
        settings: f.rules.settings,
        digest: digest(b"changed rules"),
        options: f.rules.options.clone(),
        items: f.rules.items.clone(),
        persistent: f.rules.persistent.clone(),
    };
    let changed_editor = Editor {
        root: &f.root,
        account: account(1),
        rules: &changed,
        supported: true,
        starter: None,
        restart: false,
    };
    assert!(
        changed_editor
            .apply(&f.proof(Checkpoint::AfterIntro), "rules", Timestamp(2))
            .is_err()
    );
    assert_eq!(f.backups(), 0);
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
}

#[test]
fn failure_after_backup_rolls_back_every_table_then_retry_uses_a_fresh_backup() {
    let f = Fixture::new();
    let proof = f.proof(Checkpoint::AfterIntro);
    let before = f.repo.inspect(account(1)).unwrap();
    let sql = rusqlite::Connection::open(f.repo.path(account(1))).unwrap();
    sql.execute_batch(&format!("CREATE TRIGGER reject_story_receipt BEFORE INSERT ON persistent WHEN NEW.id={} BEGIN SELECT RAISE(ABORT,'injected commit failure'); END;",key("AfterhoursStoryEdits"))).unwrap();
    assert!(
        f.editor(true)
            .apply(&proof, "failed", Timestamp(2))
            .is_err()
    );
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
    assert_eq!(f.backups(), 1);
    sql.execute_batch("DROP TRIGGER reject_story_receipt;")
        .unwrap();
    drop(sql);
    let applied = f
        .editor(true)
        .apply(&proof, "fresh-attempt", Timestamp(3))
        .unwrap();
    assert!(!applied.replayed);
    assert_eq!(f.backups(), 2);
    assert_eq!(
        applied.generation,
        (before.snapshot.generation + 1).to_string()
    );
    assert!(
        f.editor(true)
            .apply(&proof, "retry", Timestamp(4))
            .unwrap()
            .replayed
    );
    assert_eq!(f.backups(), 2);
}

#[test]
fn retry_refuses_a_missing_or_changed_backup_and_corrupt_receipts() {
    let f = Fixture::new();
    let proof = f.proof(Checkpoint::AfterIntro);
    let saved = f
        .editor(true)
        .apply(&proof, "original", Timestamp(2))
        .unwrap();
    let before = f.repo.inspect(account(1)).unwrap();
    let path = f
        .root
        .join("artifacts/story-backups")
        .join(&saved.backup.id)
        .join("manifest.json");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        f.editor(true)
            .apply(&proof, "missing", Timestamp(3))
            .is_err()
    );
    std::fs::write(&path, b"{}").unwrap();
    assert!(
        f.editor(true)
            .apply(&proof, "changed", Timestamp(3))
            .is_err()
    );
    std::fs::write(path, bytes).unwrap();
    assert!(
        f.editor(true)
            .apply(&proof, "good", Timestamp(3))
            .unwrap()
            .replayed
    );
    let mut corrupt = before.snapshot.clone();
    corrupt
        .tables
        .get_mut(&key("AfterhoursStoryEdits"))
        .unwrap()
        .rows
        .get_mut(&0)
        .unwrap()
        .insert(5, Value::String("01".into()));
    assert!(f.editor(true).receipt(&corrupt, &proof).is_err());
    assert_eq!(f.repo.inspect(account(1)).unwrap(), before);
}

#[test]
fn invalid_lock_files_are_errors_rather_than_busy_previews() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("launcher.lock")).unwrap();
    assert!(f.editor(true).preview(Checkpoint::AfterIntro).is_err());
}

#[test]
fn a_preview_cannot_be_applied_to_a_different_copy_of_the_same_account() {
    let first = Fixture::new();
    let other = Fixture::new();
    let proof = first.proof(Checkpoint::AfterIntro);
    let before = other.repo.inspect(account(1)).unwrap();
    assert_eq!(first.repo.inspect(account(1)).unwrap(), before);
    assert_ne!(proof.plan_id, other.proof(Checkpoint::AfterIntro).plan_id);
    assert!(
        other
            .editor(true)
            .apply(&proof, "different-save", Timestamp(2))
            .is_err()
    );
    assert_eq!(other.repo.inspect(account(1)).unwrap(), before);
    assert_eq!(other.backups(), 0);
}

#[test]
fn command_options_require_one_action_and_complete_canonical_proof() {
    let base = [
        "--root",
        "data",
        "--local-account",
        "01010101010101010101010101010101",
        "--progression-content",
        "rules.json",
    ];
    let parse_extra = |extra: Vec<&str>| parse(base.into_iter().chain(extra).map(Into::into));
    assert!(parse_extra(vec!["--preview", "9007199254740993"]).is_ok());
    for extra in [
        vec!["--apply", "after-intro"],
        vec!["--preview", "7", "--apply", "21"],
        vec!["--preview", "0"],
        vec!["--preview", "07"],
        vec!["--preview", "7", "--preview", "21"],
        vec!["--preview", "7", "--expected-generation", "1"],
        vec!["--preview", "7", "--unknown", "x"],
    ] {
        assert!(parse_extra(extra).is_err());
    }
    assert!(
        parse_extra(vec![
            "--apply",
            "21",
            "--expected-generation",
            "1",
            "--plan-id",
            &"a".repeat(64)
        ])
        .is_ok()
    );
    assert!(
        parse_extra(vec![
            "--apply",
            "21",
            "--expected-generation",
            "01",
            "--plan-id",
            &"a".repeat(64)
        ])
        .is_err()
    );
}

#[test]
fn production_command_reads_prepared_rules_and_keeps_unvalidated_points_disabled() {
    let f = Fixture::new();
    let rules = f.root.join("rules.json");
    std::fs::write(&rules,serde_json::to_vec(&json!({"format":"nfs-progression-entities","version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"restoration":catalog_json(),"settings":{"consumer_beta":false}})).unwrap()).unwrap();
    let before = std::fs::read(f.repo.path(account(1))).unwrap();
    let args = || Args {
        root: f.root.clone(),
        account: account(1),
        rules: rules.clone(),
        target: Checkpoint::AfterIntro,
        proof: None,
        starter: None,
        restart: false,
    };
    let p = run(args()).unwrap();
    assert_eq!(p["can_apply"], false);
    let mut apply = args();
    apply.proof = Some(Proof {
        target: Checkpoint::AfterIntro,
        expected: 1,
        plan_id: p["plan_id"].as_str().unwrap().into(),
    });
    assert!(run(apply).is_err());
    assert_eq!(f.backups(), 0);
    assert_eq!(std::fs::read(f.repo.path(account(1))).unwrap(), before);
}
