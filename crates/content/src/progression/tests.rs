// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    assets::AssetIndex,
    synthetic::{self, Field},
};
use std::collections::BTreeSet;
struct Installation(std::path::PathBuf);
impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn index(extra: Vec<synthetic::Asset>) -> (Installation, AssetIndex) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = Installation(std::env::temp_dir().join(format!(
        "nfs-progression-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    let mut assets = synthetic::assets();
    assets.retain(|a| !extra.iter().any(|b| b.name == a.name));
    assets.extend(extra);
    let (options, _) = synthetic::write(&dir.0, &assets).unwrap();
    let index = AssetIndex::open(&dir.0, &options).unwrap();
    (dir, index)
}
#[test]
fn defaults_roles_registry_order_and_explicit_channel_targets_come_from_installation() {
    let (_dir, mut index) = index(vec![]);
    let bytes = build(&mut index, &"a".repeat(64)).unwrap();
    let v: Json = serde_json::from_slice(&bytes).unwrap();
    let parsed = Content::from_json(&v).unwrap();
    let main = parsed.construction.blueprint(Branch::Main).unwrap();
    let intro = parsed.construction.blueprint(Branch::Intro).unwrap();
    assert_eq!(main.asset().bundle, 4);
    assert_eq!(main.asset().type_id, 30);
    assert_eq!(main.asset().local_index, 1);
    assert_eq!(intro.asset().local_index, 0);
    assert_eq!(main.bus_count(), 1);
    let id = (-13i64) as u64;
    assert_eq!(parsed.catalog.objective_ids(Branch::Main), [id].into());
    assert_eq!(
        parsed.catalog.branches()[&Branch::Main].objectives(),
        &[(id, Expr::Constant(false))]
    );
    assert_eq!(
        parsed.catalog.branches()[&Branch::Intro].objectives(),
        &[(77, Expr::Constant(true))]
    );
    let key = crate::assets::scene_key(synthetic::progression::SCENE);
    // Serializer1 has the same channel asset but no selected spawner link.
    assert_eq!(
        main.roles(),
        [
            entities::Role::Root,
            entities::Role::Objective { id },
            entities::Role::Channel {
                scene_content_key: key,
                scene_serializer: 2
            }
        ]
    );
    assert_eq!(
        intro.roles()[2],
        entities::Role::Channel {
            scene_content_key: key,
            scene_serializer: 3
        }
    );
    assert_eq!(v["restoration"]["source"]["sha256"], "a".repeat(64));
    assert!(!parsed.settings.consumer_beta);
    assert!(!parsed.speed_list_bypass);
}
#[test]
fn invalid_graphs_fail_without_guessing_missing_ids_inputs_or_channels() {
    for case in 0..7 {
        let extras = synthetic::progression::assets_with(|name, fields, objects| {
            if name != synthetic::progression::MAIN {
                return;
            }
            match case {
                0 => objects[4].1[0].1 = Field::Long(0),
                1 => objects[4].1[0].1 = Field::Long(77),
                2 => {
                    if let Field::Structs(_, rows) = &mut fields
                        .iter_mut()
                        .find(|(n, _)| *n == "PropertyConnections")
                        .unwrap()
                        .1
                    {
                        rows.push(rows[0].clone());
                    }
                }
                3 => {
                    if let Field::Structs(_, rows) = &mut fields
                        .iter_mut()
                        .find(|(n, _)| *n == "LinkConnections")
                        .unwrap()
                        .1
                    {
                        rows.pop();
                    }
                }
                4 => objects[3].0 = "UnrecognizedObjective",
                5 => {
                    if let Field::RawPointers(rows) =
                        &mut fields.iter_mut().find(|(n, _)| *n == "Objects").unwrap().1
                    {
                        rows.push(1);
                    }
                }
                6 => {
                    if let Field::Structs(_, rows) = &mut fields
                        .iter_mut()
                        .find(|(n, _)| *n == "LinkConnections")
                        .unwrap()
                        .1
                    {
                        rows.push(rows[1].clone());
                    }
                }
                _ => unreachable!(),
            }
        });
        let (_dir, mut index) = index(extras);
        assert!(build(&mut index, &"a".repeat(64)).is_err(), "case{case}");
    }
}
#[test]
fn progression_rule_changes_invalidate_content_fingerprint() {
    let (dir, index) = index(vec![]);
    let mut options = crate::Options {
        identity: index.identity,
        limits: crate::Limits::default(),
    };
    let first = crate::fingerprint(&dir.0, &options).unwrap();
    options
        .identity
        .progression
        .ports
        .insert("spawner_channels".into(), 999);
    assert_ne!(
        first.digest,
        crate::fingerprint(&dir.0, &options).unwrap().digest
    );
}
#[test]
fn profile_rejects_unknown_roles_ports_and_duplicate_roots() {
    let original = synthetic::progression::rules().to_json();
    for case in 0..5 {
        let mut value = original.clone();
        match case {
            0 => value["classes"]["RaceObjectiveEntityData"] = "guessed".into(),
            1 => value["ports"]["active"] = 0.into(),
            2 => value["ports"]["unexpected"] = 15.into(),
            3 => value["intro"] = value["main"].clone(),
            4 => value["scene"] = "../outside".into(),
            _ => unreachable!(),
        }
        assert!(Rules::from_json(&value).is_err(), "case{case}");
    }
    assert_eq!(
        Rules::from_json(&original).unwrap(),
        synthetic::progression::rules()
    );
}

#[test]
fn activation_evaluates_against_completed_objectives_and_post_intro_rows_follow() {
    let catalog = Catalog::from_json(&serde_json::json!({"version":1,"build_sha256":BUILD,
        "source":{"path":"constructed-progression","sha256":"00".repeat(32)},"branches":[
        {"name":"main","objectives":[
            {"id":30,"active":{"all":[{"completed":20},{"negate":{"setting":"Gameplay.NFS16ConsumerBeta"}}]}},
            {"id":40,"active":{"completed":30}},
            {"id":50,"active":{"constant":false}}],"ints":[7]},
        {"name":"intro","objectives":[{"id":20,"active":{"completed":10}},{"id":10,"active":{"constant":true}}],"ints":[]}
    ]}))
    .unwrap();
    let off = Settings {
        consumer_beta: false,
    };
    let on = Settings {
        consumer_beta: true,
    };
    let main = catalog.branches()[&Branch::Main].objectives();
    let intro = catalog.objective_ids(Branch::Intro);
    assert_eq!(intro, BTreeSet::from([10, 20]));
    let (first, second, never) = (&main[0].1, &main[1].1, &main[2].1);
    assert!(!first.initial(off));
    assert!(first.evaluate(off, &intro));
    assert!(!first.evaluate(on, &intro));
    assert!(!first.evaluate(off, &BTreeSet::from([10])));
    assert!(!second.evaluate(off, &intro));
    assert!(second.evaluate(off, &BTreeSet::from([30])));
    assert!(!never.evaluate(off, &BTreeSet::from([10, 20, 30, 40])));
    assert_eq!(
        catalog.post_intro(off),
        [(10, false, true), (20, false, true), (30, true, false)].map(|(id, active, completed)| {
            ObjectiveRow {
                id,
                active,
                completed,
            }
        })
    );
    assert_eq!(catalog.post_intro(on).len(), 2);
}
