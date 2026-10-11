// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field, PartitionWriter, guid};
use std::path::PathBuf;

struct Fixture {
    dir: PathBuf,
    index: AssetIndex,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture(assets: Vec<Asset>) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "nfs-scene-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (options, _) = synthetic::write(&dir, &assets).unwrap();
    let index = AssetIndex::open(&dir, &options).unwrap();
    Fixture { dir, index }
}
fn asset(seed: u8, name: &str, class: &str, fields: Vec<(&'static str, Field)>) -> Asset {
    Asset {
        name: name.to_owned(),
        partition: PartitionWriter::default().build(guid(seed), class, guid(seed ^ 0x80), fields),
    }
}

#[test]
fn construction_order_preserves_shared_instances_and_ignores_unlinked_objects() {
    let root = Asset {
        name: "level".into(),
        partition: PartitionWriter::default().build_many(
            guid(1),
            "LevelData",
            guid(0x81),
            // Deliberately store fields backwards from construction order.
            vec![
                ("Blueprint", Field::Import(guid(2), guid(0x82))),
                ("Components", Field::RawPointers(vec![2])),
                ("Object", Field::RawPointer(0)),
                ("Objects", Field::RawPointers(vec![3, 2])),
                ("Links", Field::RawPointers(vec![4])),
            ],
            vec![
                ("SyntheticBool", vec![]),
                ("SyntheticRpc", vec![]),
                ("UnknownUnlinkedObject", vec![]),
            ],
        ),
    };
    let prefab = asset(2, "prefab", "SyntheticRpc", vec![]);
    let mut f = fixture(vec![root, prefab]);
    let scene = f.index.scene("LEVEL").unwrap();
    assert!(scene.profile.is_root());
    assert_eq!(
        scene.profile.kinds(),
        [
            Kind::Noop,
            Kind::Rpc,
            Kind::BoolProperty,
            Kind::BoolProperty,
            Kind::Rpc
        ]
    );
    assert_eq!(scene.launchers, [1, 4]);
    assert_eq!(scene, f.index.scene("level").unwrap());
    assert!(f.index.scene("prefab").is_err());
}

#[test]
fn cycles_bad_edges_and_unknown_classes_are_errors_not_partial_profiles() {
    for fields in [
        vec![("Objects", Field::RawPointers(vec![1]))],
        vec![("Objects", Field::RawPointers(vec![99]))],
        vec![("Objects", Field::Ints(vec![1]))],
        vec![("Blueprint", Field::Import(guid(90), guid(91)))],
        vec![("Blueprint", Field::Import(guid(2), guid(99)))],
        vec![("Objects", Field::Imports(vec![(guid(2), guid(0x82))]))],
    ] {
        let mut f = fixture(vec![
            asset(1, "root", "SubWorldData", fields),
            asset(2, "child", "UnknownClass", vec![]),
        ]);
        assert!(f.index.scene("root").is_err());
    }
    let mut f = fixture(vec![
        asset(
            1,
            "root",
            "SubWorldData",
            vec![("Blueprint", Field::Import(guid(2), guid(0x82)))],
        ),
        asset(
            2,
            "child",
            "WorldPartData",
            vec![("Object", Field::Import(guid(1), guid(0x81)))],
        ),
    ]);
    assert!(
        f.index
            .scene("root")
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn traversal_depth_object_and_serializer_bounds_are_independent() {
    let chain = (1..=MAX_GRAPH_DEPTH + 2)
        .map(|i| {
            asset(
                i as u8,
                &format!("node{i}"),
                if i == 1 {
                    "SubWorldData"
                } else {
                    "WorldPartData"
                },
                if i <= MAX_GRAPH_DEPTH + 1 {
                    vec![(
                        "Blueprint",
                        Field::Import(guid((i + 1) as u8), guid((i + 1) as u8 ^ 0x80)),
                    )]
                } else {
                    vec![]
                },
            )
        })
        .collect();
    let mut f = fixture(chain);
    assert!(
        f.index
            .scene("node1")
            .unwrap_err()
            .to_string()
            .contains("graph bound")
    );
    for (class, count, error) in [
        ("SyntheticRpc", MAX_SERIALIZERS, "serializer bound"),
        ("WorldPartData", MAX_GRAPH_OBJECTS, "graph bound"),
    ] {
        let root = asset(
            1,
            "root",
            "SubWorldData",
            vec![(
                "Objects",
                Field::Imports(vec![(guid(2), guid(0x82)); count]),
            )],
        );
        let mut f = fixture(vec![root, asset(2, "leaf", class, vec![])]);
        assert!(
            f.index
                .scene("root")
                .unwrap_err()
                .to_string()
                .contains(error)
        );
    }
}

#[test]
fn catalog_validates_shapes_roots_launcher_indices_and_bounds() {
    let base = json!({"format":FORMAT,"version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "level":"constructed/level","scenes":[
            {"content_key":1,"root":true,"serializers":["noop","rpc"],"launchers":[1]},
            {"content_key":2,"root":false,"serializers":["noop"],"launchers":[]}]});
    assert_eq!(Catalog::from_json(&base).unwrap().to_json(), base);
    let mut cases = Vec::new();
    for (field, value) in [
        ("version", json!(2)),
        ("build_sha256", json!("unknown")),
        ("extra", json!(true)),
    ] {
        let mut bad = base.clone();
        bad[field] = value;
        cases.push(bad);
    }
    for (field, value) in [
        ("root", json!(false)),
        ("serializers", json!(["future"])),
        ("serializers", json!(vec!["noop"; MAX_SERIALIZERS + 1])),
        ("launchers", json!([0])),
        ("launchers", json!([1, 1])),
        ("launchers", json!([2])),
        ("state", json!([])),
    ] {
        let mut bad = base.clone();
        bad["scenes"][0][field] = value;
        cases.push(bad);
    }
    let mut duplicate = base.clone();
    duplicate["scenes"][1] = base["scenes"][0].clone();
    cases.push(duplicate);
    let mut no_root = base.clone();
    no_root["scenes"] = json!([base["scenes"][1]]);
    cases.push(no_root);
    let mut many = base.clone();
    many["scenes"] = json!(vec![base["scenes"][0].clone(); MAX_PROFILES + 1]);
    cases.push(many);
    for bad in cases {
        assert!(Catalog::from_json(&bad).is_err(), "{bad}");
    }
}
