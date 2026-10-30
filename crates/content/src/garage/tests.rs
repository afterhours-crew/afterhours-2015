// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field};
use std::sync::atomic::{AtomicU32, Ordering};

fn build_assets(assets: Vec<Asset>) -> Result<serde_json::Value, Error> {
    read_assets(assets, |index| {
        build(index).map(|bytes| serde_json::from_slice(&bytes).unwrap())
    })
}
fn read_assets<T>(
    assets: Vec<Asset>,
    read: impl FnOnce(&mut AssetIndex) -> Result<T, Error>,
) -> Result<T, Error> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "nfs-garage-content-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let (options, _) = synthetic::write(&path, &assets).unwrap();
    let mut index = AssetIndex::open(&path, &options).unwrap();
    let result = read(&mut index);
    drop(index);
    std::fs::remove_dir_all(path).unwrap();
    result
}

fn starter_assets(case: usize) -> Vec<Asset> {
    let mut assets = synthetic::garage_layout_with(|g, objects, _, _| {
        rows(g, "LinkConnections").truncate(3);
        for i in 0..3 {
            let (partition, guid) = synthetic::reference(60 + i);
            objects.push((
                "ItemEntityData",
                vec![("Item", Field::Import(partition, guid))],
            ));
            if let Field::RawPointers(v) = field(g, "Objects") {
                v.push(13 + u32::from(i));
            }
            rows(g, "LinkConnections").push(vec![
                ("Source", Field::RawPointer(3 + u32::from(i))),
                ("SourceFieldId", Field::UInt(starter::SLOT_ITEM)),
                ("Target", Field::RawPointer(13 + u32::from(i))),
                ("TargetFieldId", Field::UInt(0)),
            ]);
        }
        match case {
            1 => {
                rows(g, "LinkConnections").swap(1, 2);
            }
            2 => {
                rows(g, "LinkConnections").remove(3);
            }
            3 => {
                *field(&mut rows(g, "LinkConnections")[4], "Target") = Field::RawPointer(13);
            }
            4 => {
                *field(&mut rows(g, "LinkConnections")[4], "Target") = Field::RawPointer(8);
            }
            5 => {
                *field(&mut rows(g, "LinkConnections")[1], "Target") = Field::RawPointer(3);
            }
            6 => {
                rows(g, "PropertyConnections").remove(1);
            }
            7 => {
                *field(&mut rows(g, "PropertyConnections")[1], "Flags") = Field::UInt(2);
            }
            8 => {
                if let Field::RawPointers(v) = field(g, "Objects") {
                    v.push(13);
                }
            }
            _ => {}
        }
    });
    assets.extend(
        (60..63).map(|i| synthetic::item(i, "RaceVehicleItemData", vec![], vec![], vec![])),
    );
    assets
}

#[test]
fn starter_slots_follow_their_own_items_and_locators() {
    let garage = read_assets(starter_assets(0), starter::extract).unwrap();
    assert_eq!(
        garage
            .slots
            .map(|s| (s.component, s.item_component, s.definition, s.locator())),
        std::array::from_fn(|i| (
            6 - i,
            7 + i,
            synthetic::reference(60 + i as u8).1,
            [i as f32 * 7., 2., -3.]
        ))
    );
    let reordered = read_assets(starter_assets(1), starter::extract).unwrap();
    assert_eq!(reordered.slots[1].definition, synthetic::reference(62).1);
    for case in 2..9 {
        assert!(
            read_assets(starter_assets(case), starter::extract).is_err(),
            "case {case}"
        );
    }
    assert!(read_assets(synthetic::garage_layout(), starter::extract).is_err());
}
fn field<'a>(fields: &'a mut [(&str, Field)], name: &str) -> &'a mut Field {
    &mut fields.iter_mut().find(|(n, _)| *n == name).unwrap().1
}
fn rows<'a>(
    fields: &'a mut [(&str, Field)],
    name: &str,
) -> &'a mut Vec<Vec<(&'static str, Field)>> {
    match field(fields, name) {
        Field::Structs(_, rows) => rows,
        _ => panic!("fixture rows"),
    }
}

#[test]
fn link_order_controls_slots_while_graph_order_controls_serializer_positions() {
    let value = build_assets(synthetic::garage_layout()).unwrap();
    for (ordinal, row) in value["main"].as_array().unwrap().iter().enumerate() {
        assert_eq!(row["ordinal"], ordinal);
        assert_eq!(row["component_index"], 6 - ordinal);
        assert_eq!(row["item_component_index"], 5 - ordinal);
        assert_eq!(row["transform_bits"][12], (ordinal as f32 * 7.0).to_bits());
        assert_eq!(row["transform_bits"][13], 2.0f32.to_bits());
        assert_eq!(row["transform_bits"][14], (-3.0f32).to_bits());
    }
    let reordered = build_assets(synthetic::garage_layout_with(|g, _, _, _| {
        rows(g, "LinkConnections").swap(1, 4);
    }))
    .unwrap();
    // Slot identities and transforms follow the list; inventory roles follow ordinal.
    assert_eq!(
        reordered["main"][1]["component_index"],
        value["main"][4]["component_index"]
    );
    assert_eq!(
        reordered["main"][1]["transform_bits"],
        value["main"][4]["transform_bits"]
    );
    assert_eq!(
        reordered["main"][1]["item_component_index"],
        value["main"][1]["item_component_index"]
    );
}

#[test]
fn invalid_connections_never_publish_a_partial_or_guessed_layout() {
    for case in 0..13 {
        let assets = synthetic::garage_layout_with(|g, objects, p, _| match case {
            0 => {
                let r = rows(g, "LinkConnections");
                *field(&mut r[4], "Target") = Field::RawPointer(3);
            }
            1 => {
                rows(g, "PropertyConnections").remove(0);
            }
            2 => {
                let r = rows(g, "PropertyConnections");
                r.push(r[0].clone());
            }
            3 => {
                *field(&mut rows(g, "PropertyConnections")[0], "Flags") = Field::UInt(0);
            }
            4 => {
                *field(&mut rows(g, "LinkConnections")[0], "Target") = Field::RawPointer(0);
            }
            5 => {
                *field(&mut rows(g, "LinkConnections")[0], "Target") = Field::RawPointer(99);
            }
            6 => {
                let r = rows(p, "LinkConnections");
                *field(&mut r[1], "Target") = Field::RawPointer(3);
            }
            7 => {
                if let Field::RawPointers(v) = field(g, "Objects") {
                    v.push(3);
                }
            }
            8 => {
                if let Field::RawPointers(v) = field(g, "Objects") {
                    v.retain(|p| *p != 8);
                }
            }
            9 => {
                objects[6].0 = "SyntheticBool";
            }
            10 => {
                objects.push(("GarageEntityData", vec![]));
                if let Field::RawPointers(v) = field(g, "Objects") {
                    v.push(13);
                }
                let r = rows(g, "LinkConnections");
                let mut other = r.clone();
                for row in &mut other {
                    *field(row, "Source") = Field::RawPointer(13);
                }
                r.extend(other);
            }
            11 => {
                rows(g, "LinkConnections").remove(0);
            }
            12 => {
                if let Field::RawPointers(v) = field(p, "Objects") {
                    v.push(2);
                }
            }
            _ => unreachable!(),
        });
        assert!(build_assets(assets).is_err(), "case {case}");
    }
}

#[test]
fn transforms_require_finite_float_components_and_a_valid_basis() {
    for value in [
        Field::Float(f32::NAN),
        Field::Float(f32::INFINITY),
        Field::Int(0),
    ] {
        let assets = synthetic::garage_layout_with(|_, objects, _, _| {
            let Field::Struct(_, transform) = field(&mut objects[6].1, "Transform") else {
                panic!()
            };
            let Field::Struct(_, translation) = field(transform, "trans") else {
                panic!()
            };
            *field(translation, "x") = value;
        });
        assert!(build_assets(assets).is_err());
    }
    let zero_basis = synthetic::garage_layout_with(|_, objects, _, _| {
        let Field::Struct(_, transform) = field(&mut objects[6].1, "Transform") else {
            panic!()
        };
        *field(transform, "right") = synthetic::vector(0.0, 0.0, 0.0);
    });
    assert!(build_assets(zero_basis).is_err());
}

#[test]
fn link_profile_is_strict_and_signed_content_ids_preserve_their_bits() {
    let base = synthetic::garage_links().to_json();
    assert_eq!(Links::from_json(&base).unwrap().to_json(), base);
    for (key, value) in [
        ("items", json!([1, 2, 3, 4, 4])),
        ("items", json!([1, 2, 3, 4])),
        ("primary_slot", json!(0)),
        ("primary_slot", json!(101)),
        ("slot_location", json!(-1)),
        ("locator_transform", json!(u64::from(u32::MAX) + 1)),
        ("exit_location", json!(0)),
        ("extra", json!(7)),
    ] {
        let mut bad = base.clone();
        bad[key] = value;
        assert!(Links::from_json(&bad).is_err());
    }
    assert_eq!(
        id(&Fields(vec![("x".into(), Value::Int32(-1))]), "x").unwrap(),
        u32::MAX
    );
    assert!(id(&Fields(vec![("x".into(), Value::Float32(1.0))]), "x").is_err());
}

fn build_exit(
    edit: impl FnOnce(
        &mut Vec<(&'static str, Field)>,
        &mut Vec<(&'static str, Vec<(&'static str, Field)>)>,
    ),
) -> Result<serde_json::Value, Error> {
    let mut assets = synthetic::garage_layout();
    *assets
        .iter_mut()
        .find(|a| a.name.ends_with("/spawnpoints"))
        .unwrap() = synthetic::exit_layout_with(edit);
    build_assets(assets)
}

#[test]
fn exit_destination_follows_the_exported_property_not_locator_order() {
    let original = build_assets(synthetic::garage_layout()).unwrap();
    assert_eq!(
        original["exit_teleport"]["transform_bits"][12],
        20.0f32.to_bits()
    );
    assert_eq!(
        original["exit_teleport"]["transform_bits"][13],
        5.0f32.to_bits()
    );
    let redirected = build_exit(|root, _| {
        *field(&mut rows(root, "PropertyConnections")[0], "Source") = Field::RawPointer(2);
    })
    .unwrap();
    assert_eq!(
        redirected["exit_teleport"]["transform_bits"][12],
        10.0f32.to_bits()
    );
    assert_eq!(original["main"], redirected["main"]);
    assert!(original.get("world_spawn").is_none());
}

#[test]
fn missing_or_ambiguous_exit_exports_and_unconstructed_locators_are_rejected() {
    for case in 0..12 {
        assert!(
            build_exit(|root, objects| match case {
                0 => *field(root, "Interface") = Field::RawPointer(0),
                1 => *field(root, "Interface") = Field::RawPointer(50),
                2 => *field(root, "Interface") = Field::RawPointer(2),
                3 => *field(&mut rows(&mut objects[2].1, "Fields")[0], "Id") = Field::Int(999),
                4 => {
                    let fields = rows(&mut objects[2].1, "Fields");
                    fields.push(fields[0].clone());
                }
                5 =>
                    *field(&mut rows(&mut objects[2].1, "Fields")[0], "AccessType") =
                        Field::Enum("FieldAccessType", "FieldAccessType_Target", 1),
                6 =>
                    *field(&mut rows(root, "PropertyConnections")[0], "TargetFieldId") =
                        Field::Int(999),
                7 => {
                    let properties = rows(root, "PropertyConnections");
                    properties.push(properties[0].clone());
                }
                8 => *field(&mut rows(root, "PropertyConnections")[0], "Flags") = Field::UInt(1),
                9 =>
                    *field(&mut rows(root, "PropertyConnections")[0], "Source") =
                        Field::RawPointer(0),
                10 => *field(root, "Objects") = Field::RawPointers(vec![2]),
                11 => *field(root, "Objects") = Field::RawPointers(vec![2, 3, 3]),
                _ => unreachable!(),
            })
            .is_err(),
            "case {case}"
        );
    }
}
