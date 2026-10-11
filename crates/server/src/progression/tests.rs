// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::persistent;
use nfs_storage::{
    AccountId, Batch, GarageSlots, InventoryRepository, MemoryRepository, Op, Snapshot, Timestamp,
    tables::{Row, Table},
};
use serde_json::json;
pub(crate) fn content() -> Json {
    json!({"version":1,"build_sha256":BUILD,"source":{"path":"constructed-progression","sha256":"00".repeat(32)},"branches":[
        {"name":"main","objectives":[{"id":30,"active":{"all":[{"completed":20},{"negate":{"setting":"Gameplay.NFS16ConsumerBeta"}}]}}],"ints":[u64::MAX,42]},
        {"name":"intro","objectives":[{"id":20,"active":{"completed":10}},{"id":10,"active":{"constant":true}}],"ints":[]}
    ]})
}
pub(crate) fn tables() -> persistent::Catalog {
    let mut v = persistent::tests::content();
    v["tables"][0]["columns"] = json!([
        {"name":"Active","key":ACTIVE,"value_type":"int","default":"0"},
        {"name":"Completed","key":COMPLETED,"value_type":"int","default":"0"}
    ]);
    v["tables"][1]["columns"] = json!([
        {"name":"floats","key":persistent::key("floats"),"value_type":"float","default":""},
        {"name":"ints","key":INTS,"value_type":"int","default":"0"},
        {"name":"dynamicKeyFloats","key":persistent::key("dynamicKeyFloats"),"value_type":"float","default":""}
    ]);
    persistent::Catalog::from_json(&v).unwrap()
}
fn empty(repo: &dyn InventoryRepository, who: AccountId) -> Snapshot {
    repo.open(who).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops: vec![Op::SetGarage(GarageSlots::default())],
        },
        Timestamp(1),
    )
    .unwrap();
    tables()
        .ensure_loaded(repo, who, repo.snapshot(who).unwrap(), Timestamp(2))
        .unwrap()
        .0
}
pub(crate) fn local() -> (MemoryRepository, AccountId, Snapshot) {
    let repo = MemoryRepository::default();
    let who = AccountId::from_owned_config([41; 16]).unwrap();
    let snapshot = empty(&repo, who);
    (repo, who, snapshot)
}
fn restore(snapshot: &Snapshot) -> Restored {
    Catalog::from_json(&content())
        .unwrap()
        .restore(
            &tables().view(snapshot).unwrap(),
            Settings {
                consumer_beta: false,
            },
        )
        .unwrap()
}
#[test]
fn empty_save_preserves_wired_initial_flags_and_zero_values() {
    let (_, _, snapshot) = local();
    let restored = restore(&snapshot);
    assert_eq!(restored.generation(), 2);
    assert_eq!(
        restored.branch(Branch::Intro).objectives()[&10],
        Objective {
            active: true,
            completed: false
        }
    );
    for (branch, id) in [(Branch::Main, 30), (Branch::Intro, 20)] {
        assert_eq!(
            restored.branch(branch).objectives()[&id],
            Objective {
                active: false,
                completed: false
            }
        );
    }
    assert_eq!(
        restored.branch(Branch::Main).ints(),
        &BTreeMap::from([(42, 0), (u64::MAX, 0)])
    );
    assert_eq!(
        restored.branch(Branch::Main).pending(),
        &[
            Output::ValuesRestored,
            Output::StartObjective { id: 30 },
            Output::ObjectivesRestored
        ]
    );
    assert_eq!(snapshot.tables[&OBJECTIVES], Table::default());
}
#[test]
fn both_saved_flags_required_and_negative_is_false_not_nonzero() {
    let (_, _, mut snapshot) = local();
    snapshot.tables.get_mut(&OBJECTIVES).unwrap().rows = BTreeMap::from([
        (10, Row::from([(ACTIVE, Value::Int(0))])),
        (
            20,
            Row::from([(ACTIVE, Value::Int(-1)), (COMPLETED, Value::Int(i32::MAX))]),
        ),
        (
            30,
            Row::from([(ACTIVE, Value::Int(7)), (COMPLETED, Value::Int(i32::MIN))]),
        ),
        (u64::MAX, Row::from([(ACTIVE, Value::Int(1))])),
    ]);
    let before = snapshot.clone();
    let restored = restore(&snapshot);
    assert_eq!(
        restored.branch(Branch::Intro).objectives()[&10],
        Objective {
            active: true,
            completed: false
        }
    );
    assert_eq!(
        restored.branch(Branch::Intro).objectives()[&20],
        Objective {
            active: false,
            completed: true
        }
    );
    assert_eq!(
        restored.branch(Branch::Main).objectives()[&30],
        Objective {
            active: true,
            completed: false
        }
    );
    assert_eq!(snapshot, before);
    let view = tables().view(&snapshot).unwrap();
    assert_eq!(
        view.tables()[&OBJECTIVES].rows[&10][&COMPLETED],
        Value::Int(0)
    );
    assert!(!view.has_cell(OBJECTIVES, 10, COMPLETED));
    assert!(view.has_cell(OBJECTIVES, 10, ACTIVE));
}
#[test]
fn saved_values_have_full_width_ids_and_only_present_ints_publish() {
    let (_, _, mut snapshot) = local();
    snapshot.tables.get_mut(&VALUES).unwrap().rows = BTreeMap::from([
        (u64::MAX, Row::from([(INTS, Value::Int(-789))])),
        (
            42,
            Row::from([(persistent::key("floats"), Value::Float(2.0f32.to_bits()))]),
        ),
        (99, Row::from([(INTS, Value::Int(100))])),
    ]);
    let restored = restore(&snapshot);
    assert_eq!(
        restored.branch(Branch::Main).ints(),
        &BTreeMap::from([(42, 0), (u64::MAX, -789)])
    );
    assert_eq!(
        restored.branch(Branch::Main).pending()[0],
        Output::Int {
            id: u64::MAX,
            value: -789
        }
    );
    assert_eq!(restored.branch(Branch::Main).pending().len(), 4);
}
#[test]
fn output_transfer_is_once_and_preserves_native_link_order() {
    let (_, _, snapshot) = local();
    let mut a = restore(&snapshot);
    let b = a.clone();
    let commands = a.branch_mut(Branch::Intro).take_outputs();
    assert_eq!(
        commands,
        &[
            Output::ValuesRestored,
            Output::StartObjective { id: 20 },
            Output::StartObjective { id: 10 },
            Output::ObjectivesRestored
        ]
    );
    assert!(a.branch_mut(Branch::Intro).take_outputs().is_empty());
    assert_eq!(b, restore(&snapshot));
    assert_eq!(
        a.branch(Branch::Intro).objectives(),
        b.branch(Branch::Intro).objectives()
    );
    assert!(!a.branch(Branch::Main).pending().is_empty());
}
#[test]
fn content_rejects_ambiguous_or_unbounded_static_definitions() {
    let valid = content();
    for case in 0..11 {
        let mut v = valid.clone();
        match case {
            0 => v["branches"][1]["name"] = json!("main"),
            1 => v["branches"][1]["objectives"][0]["id"] = json!(30),
            2 => v["branches"][0]["ints"] = json!([42, 42]),
            3 => v["branches"][1]["objectives"][1]["active"] = json!({"completed":999}),
            4 => v["branches"][0]["objectives"][0]["active"] = json!({"setting":"unknown"}),
            5 => v["branches"][0]["objectives"][0]["active"] = json!({"all":[]}),
            6 => v["branches"][0]["ints"] = json!(vec![1; MAX_ENTRIES + 1]),
            7 => v["branches"][0]["objectives"][0]["id"] = json!(0),
            8 => v["build_sha256"] = json!("wrong"),
            9 => v["branches"][0]["captured_values"] = json!([]),
            _ => {
                let mut expr = json!({"constant":true});
                for _ in 0..18 {
                    expr = json!({"negate":expr});
                }
                v["branches"][0]["objectives"][0]["active"] = expr;
            }
        }
        assert!(Catalog::from_json(&v).is_err(), "case{case}");
    }
}
#[test]
fn explicit_settings_affect_initial_properties_without_inventing_progress() {
    let mut v = content();
    v["branches"][0]["objectives"][0]["active"] =
        json!({"negate":{"setting":"Gameplay.NFS16ConsumerBeta"}});
    let catalog = Catalog::from_json(&v).unwrap();
    let (_, _, snapshot) = local();
    let loaded = tables().view(&snapshot).unwrap();
    for consumer_beta in [false, true] {
        let state = catalog
            .restore(&loaded, Settings { consumer_beta })
            .unwrap();
        assert_eq!(
            state.branch(Branch::Main).objectives()[&30].active,
            !consumer_beta
        );
        assert!(!state.branch(Branch::Main).objectives()[&30].completed);
    }
}
#[test]
fn dispatch_consumes_link_order_once_and_readiness_needs_both_branches() {
    let (_, _, snapshot) = local();
    let mut state = restore(&snapshot);
    let dispatched = dispatch(&mut state).unwrap();
    assert_eq!(
        dispatched[&Branch::Intro],
        Dispatched {
            values_restored: true,
            objectives_restored: true,
            ints_published: 0,
            activated: vec![10],
        }
    );
    assert!(dispatched[&Branch::Main].restored());
    assert!(dispatched[&Branch::Main].activated.is_empty());
    assert!(ready(&dispatched, false));
    assert!(ready(&dispatched, true));
    let again = dispatch(&mut state).unwrap();
    assert!(!ready(&again, false));
    assert!(!ready(&again, true));
    let mut partial = BTreeMap::from([(Branch::Main, Dispatched::default())]);
    assert!(!ready(&partial, true));
    partial.insert(
        Branch::Intro,
        Dispatched {
            values_restored: true,
            objectives_restored: true,
            ..Default::default()
        },
    );
    assert!(ready(&partial, true));
    assert!(!ready(&partial, false));
}
#[test]
fn combined_content_checks_objective_roles_against_the_restoration_catalog() {
    let valid = json!({"format":FORMAT,"version":VERSION,"build_sha256":BUILD,
        "sources":[{"path":"redacted","sha256":"00".repeat(32)}],
        "restoration":content(),"entities":entities::tests::construction_json(),
        "settings":{"consumer_beta":false,"enable_speed_lists":false}});
    let loaded = Content::from_json(&valid).unwrap();
    assert!(!loaded.speed_list_bypass);
    assert!(!loaded.settings.consumer_beta);
    for case in 0..5 {
        let mut v = valid.clone();
        match case {
            0 => v["entities"]["branches"][0]["roles"][2] = json!({"kind":"objective","id":31}),
            1 => v["settings"]["enable_speed_lists"] = json!(1),
            2 => v["sources"] = json!([]),
            3 => v["version"] = json!(2),
            _ => v["restoration"]["branches"][1]["objectives"][1]["id"] = json!(11),
        }
        assert!(Content::from_json(&v).is_err(), "case{case}");
    }
}
#[test]
fn independent_account_restore_and_reconnect_use_current_committed_generation() {
    let (repo, who, mut snapshot) = local();
    let other = AccountId::from_owned_config([42; 16]).unwrap();
    let other_snapshot = empty(&repo, other);
    let table = Table {
        rows: BTreeMap::from([(
            10,
            Row::from([(ACTIVE, Value::Int(1)), (COMPLETED, Value::Int(1))]),
        )]),
    };
    repo.apply(
        who,
        &Batch {
            id: 2,
            expected_generation: snapshot.generation,
            ops: vec![Op::SetTable(OBJECTIVES, table)],
        },
        Timestamp(3),
    )
    .unwrap();
    snapshot = repo.snapshot(who).unwrap();
    assert!(restore(&snapshot).branch(Branch::Intro).objectives()[&10].completed);
    assert!(!restore(&other_snapshot).branch(Branch::Intro).objectives()[&10].completed);
    assert_eq!(restore(&snapshot), restore(&repo.snapshot(who).unwrap()));
    assert_eq!(restore(&snapshot).generation(), 3);
}
