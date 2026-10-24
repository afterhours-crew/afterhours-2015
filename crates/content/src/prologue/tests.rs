// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::{Fields, Object};

fn transform(position: [f32; 3]) -> Value {
    let vectors = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], position];
    Value::Struct(Fields(
        ["right", "up", "forward", "trans"]
            .into_iter()
            .zip(vectors)
            .map(|(k, v)| {
                (
                    k.into(),
                    Value::Struct(Fields(
                        ["x", "y", "z"]
                            .into_iter()
                            .zip(v)
                            .map(|(c, n)| (c.into(), Value::Float32(n)))
                            .collect(),
                    )),
                )
            })
            .collect(),
    ))
}
fn fixture() -> (Memory, Links, crate::progression::Rules) {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(2), 101, local(3), 0),
                    edge(local(1), 202, local(3), 301),
                ]),
            ),
        ],
    );
    m.put(
        "a",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    m.put("a", 2, "ParticipantLogicEntityData", &[]);
    m.put(
        "a",
        3,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("f", 0))],
    );
    m.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Interface", local(6)),
            (
                "Objects",
                Value::Array((1..=5).chain([7]).map(local).collect()),
            ),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 201, local(2), 0),
                    edge(local(3), 202, local(6), 202),
                    edge(local(5), 103, local(7), 0),
                ]),
            ),
        ],
    );
    m.put("b", 1, "ItemSelectorEntityData", &[]);
    m.put("b", 2, "ItemEntityData", &[("Item", ptr("d", 0))]);
    m.put("b", 3, "SetTeleportLocationEntityData", &[]);
    m.put(
        "b",
        4,
        "StateSequenceRefererEntityData",
        &[("Sequence", ptr("e", 0))],
    );
    m.put("b", 5, "RaceObjectiveEntityData", &[]);
    m.put("b", 6, "InterfaceDescriptorData", &[]);
    m.put(
        "b",
        7,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(9007199254740993))],
    );
    m.put(
        "c",
        0,
        "SubWorldData",
        &[
            (
                "Objects",
                Value::Array((1..=5).chain([7, 8]).map(local).collect()),
            ),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 102, local(2), 0),
                    edge(local(2), 301, local(3), 0),
                    edge(local(3), 203, local(4), 0),
                    edge(local(5), 301, local(7), 0),
                    edge(local(7), 203, local(8), 0),
                ]),
            ),
        ],
    );
    m.put(
        "c",
        1,
        "ParticipantLogicSpawnerEntityData",
        &[("ParticipantLogicBlueprint", ptr("a", 0))],
    );
    for i in [2, 5] {
        m.put(
            "c",
            i,
            "LinkedSchematicChannelEntityData",
            &[("ChannelAsset", ptr("f", 0))],
        );
    }
    for i in [3, 7] {
        m.put("c", i, "TeleportLocationEntityData", &[]);
    }
    m.put(
        "c",
        4,
        "LocatorEntityData",
        &[("Transform", transform([1., 2., 3.]))],
    );
    m.put(
        "c",
        8,
        "LocatorEntityData",
        &[("Transform", transform([9., 8., 7.]))],
    );
    m.put("d", 0, "RaceVehicleItemData", &[]);
    m.put("e", 0, "ParticipantStateSequenceBlueprint", &[]);
    m.put("f", 0, "LinkedSchematicChannelAsset", &[]);
    m.0.get_mut(&("d".into(), 0)).unwrap().guid = Some([7; 16]);
    m.0.get_mut(&("e".into(), 0)).unwrap().guid = Some([8; 16]);
    let links = Links {
        first_drive: "b".into(),
        item_definition: 201,
        teleport_location: 202,
        teleport_locator: 203,
    };
    let mut rules = crate::BuildIdentity::supported().progression;
    rules.intro = "a".into();
    rules.scene = "c".into();
    rules.ports.insert("root_channels".into(), 101);
    rules.ports.insert("spawner_channels".into(), 102);
    rules.ports.insert("stable_id".into(), 103);
    (m, links, rules)
}
fn run(m: &mut Memory, links: &Links, rules: &crate::progression::Rules) -> Result<Start, Error> {
    read(&mut Graph::new(m), links, rules, Site::root("c"))
}

#[test]
fn destination_preserves_the_declaring_occurrence_without_reading_an_inventory_item() {
    let (mut m, links, rules) = fixture();
    m.0.remove(&("d".into(), 0));
    let (destination, _) =
        read_destination(&mut Graph::new(&mut m), &links, &rules, Site::root("c")).unwrap();
    assert_eq!(destination.objective, 9_007_199_254_740_993);
    assert_eq!(destination.teleport.position, [1., 2., 3.]);
    assert_eq!(destination.reference.asset, "b");
    assert_eq!(destination.reference.index, 4);
    assert_eq!(destination.reference.instances, vec![("a".into(), 1)]);
    assert!(run(&mut m, &links, &rules).is_err());
}
fn object<'a>(m: &'a mut Memory, asset: &str, index: usize) -> &'a mut Object {
    m.0.get_mut(&(asset.into(), index)).unwrap()
}
fn field<'a>(m: &'a mut Memory, asset: &str, index: usize, name: &str) -> &'a mut Value {
    &mut object(m, asset, index)
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == name)
        .unwrap()
        .1
}

#[test]
fn first_drive_uses_its_spawner_binding_in_the_presence_of_compatible_decoys() {
    let (mut m, links, rules) = fixture();
    let start = run(&mut m, &links, &rules).unwrap();
    assert_eq!(start.objective, 9007199254740993);
    assert_eq!(start.vehicle_item, [7; 16]);
    assert_eq!(start.opening_guid, [8; 16]);
    assert_eq!(start.opening_blueprint, "e");
    assert_eq!(
        start.teleport,
        Pose {
            position: [1., 2., 3.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
        }
    );
    let Value::Array(edges) = field(&mut m, "c", 0, "LinkConnections") else {
        panic!()
    };
    edges[0] = edge(local(1), 102, local(5), 0);
    assert_eq!(
        run(&mut m, &links, &rules).unwrap().teleport.position,
        [9., 8., 7.]
    );
}

#[test]
fn duplicate_occurrences_and_multiple_location_endpoints_are_refused() {
    let (mut m, links, rules) = fixture();
    m.put(
        "a",
        4,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    let Value::Array(objects) = field(&mut m, "a", 0, "Objects") else {
        panic!()
    };
    objects.push(local(4));
    assert!(run(&mut m, &links, &rules).is_err());
    let (mut m, links, rules) = fixture();
    let Value::Array(edges) = field(&mut m, "c", 0, "LinkConnections") else {
        panic!()
    };
    edges.push(edge(local(2), 301, local(7), 0));
    assert!(run(&mut m, &links, &rules).is_err());
}

#[test]
fn missing_or_wrong_definition_exports_never_produce_a_partial_start() {
    for mutation in 0..4 {
        let (mut m, links, rules) = fixture();
        match mutation {
            0 => object(&mut m, "d", 0).class = "OtherItemData".into(),
            1 => object(&mut m, "e", 0).guid = None,
            2 => {
                *field(&mut m, "b", 7, "StableUniqueId64") = Value::UInt64(0);
            }
            _ => {
                *field(&mut m, "b", 2, "Item") = Value::Pointer(nfs_frostbite::ebx::Pointer::Null);
            }
        }
        assert!(run(&mut m, &links, &rules).is_err());
    }
}

#[test]
fn malformed_locator_and_channel_bindings_are_rejected() {
    for mutation in 0..4 {
        let (mut m, links, rules) = fixture();
        match mutation {
            0 => {
                *field(&mut m, "c", 4, "Transform") = transform([f32::NAN, 0., 0.]);
            }
            1 => object(&mut m, "c", 4).class = "UnrelatedSpatialEntityData".into(),
            2 => {
                *field(&mut m, "c", 2, "ChannelAsset") = ptr("d", 0);
            }
            _ => {
                *field(&mut m, "a", 0, "LinkConnections") = Value::Array(vec![]);
            }
        }
        assert!(run(&mut m, &links, &rules).is_err());
    }
}

#[test]
fn links_require_exact_bounded_paths_and_nonzero_distinct_ports() {
    let (_, links, _) = fixture();
    let original = links.to_json();
    assert_eq!(Links::from_json(&original).unwrap(), links);
    for (field, value) in [
        ("first_drive", json!("../intro")),
        ("first_drive", json!("x".repeat(513))),
        ("item_definition", json!(0)),
        ("teleport_location", json!(201)),
        ("teleport_locator", json!(u64::MAX)),
        ("unknown", json!(1)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert!(Links::from_json(&changed).is_err());
    }
}
