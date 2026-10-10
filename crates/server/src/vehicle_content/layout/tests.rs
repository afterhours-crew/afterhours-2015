// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::items::{DefinitionClass, Item};
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) fn fixture() -> Value {
    let main = (0..5).map(|i| {
        let mut bits = [0u32; 16];
        for j in [0, 5, 10] { bits[j] = 1f32.to_bits() }
        bits[12] = (i as f32 * 7.).to_bits();
        json!({"ordinal":i,"component_index":(i+1)*3,"item_component_index":20+i,"transform_bits":bits})
    }).collect::<Vec<_>>();
    json!({"format":"nfs-garage-population","version":1,"build_sha256":crate::content::BUILD,
        "blueprint":"Garage/Test","item_blueprint":"Gameplay/Test","main":main})
}
fn inventory() -> Collection {
    Collection {
        roots: vec![91, 7],
        items: [91, 7]
            .into_iter()
            .map(|id| {
                (
                    id,
                    Item {
                        id,
                        owner: 0,
                        definition: [1; 16],
                        defaults: vec![],
                        children: vec![],
                        state: OWNED,
                        buy_price: 0,
                        sell_price: 0,
                        derived: Derived::decode(
                            DefinitionClass::RaceVehicleItemData.layout(),
                            &[0; 68],
                        )
                        .unwrap(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn durable_order_and_empty_slots_select_the_current_static_placement() {
    let layout = Layout::from_json(&fixture()).unwrap();
    let items = inventory();
    let slots = Slots::new([Some(91), None, None, None, Some(7)]).unwrap();
    let selected = layout.occupied(slots, &items).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|v| (
                v.ordinal,
                v.item,
                v.slot.component_index,
                v.slot.item_component_index
            ))
            .collect::<Vec<_>>(),
        [(0, 91, 3, 20), (4, 7, 15, 24)]
    );
    assert_eq!(selected[1].slot.locator(), [28., 0., 0.]);
    assert_eq!(
        selected[1].slot.basis(),
        [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
    );
    assert!(
        layout
            .occupied(Slots::default(), &items)
            .unwrap()
            .is_empty()
    );
    let swapped = layout
        .occupied(
            Slots::new([Some(7), None, None, None, Some(91)]).unwrap(),
            &items,
        )
        .unwrap();
    assert_eq!(swapped[0].item, 7);
    assert_eq!(swapped[1].slot.locator(), [28., 0., 0.]);
}

#[test]
fn missing_foreign_unowned_or_nonvehicle_items_reject_the_whole_selection() {
    let layout = Layout::from_json(&fixture()).unwrap();
    let slots = Slots::new([Some(91), Some(7), None, None, None]).unwrap();
    for mode in 0..6 {
        let mut items = inventory();
        match mode {
            0 => {
                items.items.remove(&7);
            }
            1 => items.items.get_mut(&7).unwrap().owner = 91,
            2 => items.items.get_mut(&7).unwrap().state = 0,
            3 => items.items.get_mut(&7).unwrap().derived = Derived::NosTuning { nos_setting: 0 },
            4 => items.items.get_mut(&7).unwrap().id = 8,
            5 => items.roots.retain(|id| *id != 7),
            _ => unreachable!(),
        }
        assert!(layout.occupied(slots, &items).is_err());
    }
    assert!(Slots::new([Some(91), Some(91), None, None, None]).is_err());
}

#[test]
fn content_cannot_override_inventory_or_hide_invalid_pose_and_endpoint_data() {
    for mode in 0..9 {
        let mut value = fixture();
        match mode {
            0 => {
                value["main"].as_array_mut().unwrap().pop();
            }
            1 => value["main"][4]["ordinal"] = json!(0),
            2 => value["main"][1]["component_index"] = value["main"][0]["component_index"].clone(),
            3 => value["main"][4]["item_component_index"] = json!(512),
            4 => value["main"][0]["transform_bits"][12] = json!(f32::NAN.to_bits()),
            5 => value["main"][0]["transform_bits"][0] = json!(0),
            6 => value["main"][0]["transform_bits"][15] = json!(1),
            7 => value["main"][0]["item"] = json!(91),
            8 => value["build_sha256"] = json!("different"),
            _ => unreachable!(),
        }
        assert!(Layout::from_json(&value).is_err(), "mode {mode}");
    }
}

#[test]
fn world_spawn_is_optional_and_validated() {
    let plain = Layout::from_json(&fixture()).unwrap();
    assert!(plain.world_spawn().is_none());
    let mut spawn = [0u32; 16];
    for j in [0, 5, 10] {
        spawn[j] = 1f32.to_bits();
    }
    spawn[12] = 959.5f32.to_bits();
    let mut v = fixture();
    v["world_spawn"] = json!({"transform_bits": spawn});
    let layout = Layout::from_json(&v).unwrap();
    assert_eq!(layout.world_spawn().unwrap().locator(), [959.5, 0., 0.]);
    for broken in 0..4 {
        let mut v = fixture();
        let mut bits = spawn;
        match broken {
            0 => bits[13] = f32::NAN.to_bits(),
            1 => bits[15] = 1,
            2 => bits[0] = 0,
            _ => {}
        }
        v["world_spawn"] = if broken == 3 {
            json!({"transform_bits": spawn, "extra": 1})
        } else {
            json!({"transform_bits": bits})
        };
        assert!(Layout::from_json(&v).is_err(), "case {broken}");
    }
}

#[test]
fn exit_teleport_is_optional_validated_and_independent_of_the_spawn() {
    assert!(
        Layout::from_json(&fixture())
            .unwrap()
            .exit_teleport()
            .is_none()
    );
    // A quarter turn about the vertical axis at a synthetic destination.
    let mut bits = [0u32; 16];
    bits[2] = 1f32.to_bits();
    bits[5] = 1f32.to_bits();
    bits[8] = (-1f32).to_bits();
    bits[12] = 12.5f32.to_bits();
    bits[13] = 3.25f32.to_bits();
    bits[14] = (-40f32).to_bits();
    let mut v = fixture();
    v["exit_teleport"] = json!({"transform_bits": bits});
    let layout = Layout::from_json(&v).unwrap();
    let destination = layout.exit_teleport().unwrap();
    assert_eq!(destination.locator(), [12.5, 3.25, -40.]);
    assert_eq!(
        destination.basis(),
        [[0., 0., 1.], [0., 1., 0.], [-1., 0., 0.]]
    );
    assert!(layout.world_spawn().is_none());
    for broken in 0..3 {
        let mut v = fixture();
        let mut b = bits;
        match broken {
            0 => b[14] = f32::INFINITY.to_bits(),
            1 => b[3] = 1,
            _ => b = [0; 16],
        }
        v["exit_teleport"] = json!({"transform_bits": b});
        assert!(Layout::from_json(&v).is_err(), "case {broken}");
    }
}
