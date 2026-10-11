// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::items::{Catalog, Item, Layout, OWNED};
use DefinitionClass::*;

fn performance(value: f32) -> Input {
    Input::Performance {
        value,
        group_a: None,
        clutch_group: None,
        induction: None,
    }
}
fn appearance(group_b: Option<u8>, flag: bool) -> Input {
    Input::Appearance {
        spoiler_group: None,
        group_b,
        rim: None,
        flag,
    }
}
fn rim(rear: bool, values: Vec<f32>) -> Input {
    Input::Appearance {
        rim: Some(Rim { rear, values }),
        spoiler_group: None,
        group_b: None,
        flag: false,
    }
}
fn item(id: u64, owner: u64, class: DefinitionClass) -> Item {
    let derived = match class.layout() {
        Layout::RaceVehicle => Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap(),
        Layout::Rims => Derived::decode(Layout::Rims, &[0; 52]).unwrap(),
        Layout::Spoiler => Derived::Spoiler {
            downforce_setting: 5,
        },
        Layout::BrakeDiscs => Derived::BrakeDiscs {
            brake_strength_setting: 5,
            brake_bias_setting: 5,
        },
        Layout::Empty => Derived::Empty,
        _ => panic!("test fixture class"),
    };
    Item {
        id,
        owner,
        definition: u128::from(id).to_le_bytes(),
        defaults: vec![],
        children: vec![],
        state: OWNED,
        buy_price: 0,
        sell_price: 0,
        derived,
    }
}
fn fixture(inputs: Vec<(DefinitionClass, Input)>) -> (Collection, Definitions) {
    let mut root = item(1, 0, RaceVehicleItemData);
    root.children = (2..inputs.len() as u64 + 2).collect();
    let mut collection = Collection {
        roots: vec![1],
        items: BTreeMap::from([(1, root)]),
    };
    let mut classes = vec![(1_u128.to_le_bytes(), RaceVehicleItemData)];
    let mut entries = vec![(1_u128.to_le_bytes(), Input::Other)];
    let mut tuning_inputs = vec![];
    for (i, (class, input)) in inputs.into_iter().enumerate() {
        let id = i as u64 + 2;
        let part = item(id, 1, class);
        if tuning::applies(class) {
            let asset_present = match &input {
                Input::Performance { group_a, .. } => group_a.is_some(),
                Input::Appearance { spoiler_group, .. } => spoiler_group.is_some(),
                _ => false,
            };
            tuning_inputs.push((
                part.definition,
                tuning::PartInput {
                    asset_present,
                    transmission_values: None,
                },
            ));
        }
        classes.push((part.definition, class));
        entries.push((part.definition, input));
        collection.items.insert(id, part);
    }
    let tuning = tuning::Definitions::new(Catalog::new(classes).unwrap(), tuning_inputs).unwrap();
    (collection, Definitions::new(tuning, entries).unwrap())
}
fn build(c: &Collection, d: &Definitions) -> Customization {
    Customization::from_vehicle(c, d, 1).unwrap()
}

#[test]
fn empty_current_parts_use_native_constructor_defaults() {
    let (mut c, d) = fixture(vec![(AirFilterItemData, performance(0.8))]);
    c.items.get_mut(&1).unwrap().children.clear();
    c.items.get_mut(&1).unwrap().defaults = vec![2];
    let result = build(&c, &d);
    assert_eq!(
        result,
        Customization {
            value: 0,
            list_a: vec![],
            list_b: vec![],
            index: None,
            values: [0, 0],
            flag: false,
            tuning: [5; 32]
        }
    );
}

#[test]
fn performance_uses_float32_sum_truncation_threshold_and_saturation() {
    let (c, d) = fixture(vec![
        (AirFilterItemData, performance(0.125)),
        (CamShaftItemData, performance(0.5)),
    ]);
    assert_eq!(build(&c, &d).value, 159);
    let (c, d) = fixture(vec![
        (AirFilterItemData, performance(0.8)),
        (CamShaftItemData, performance(0.5)),
    ]);
    assert_eq!(build(&c, &d).value, 255);
    let (c, d) = fixture(vec![(AirFilterItemData, performance(0.00000099)); 4000]);
    assert_eq!(build(&c, &d).value, 0);
    let (c, d) = fixture(vec![(AirFilterItemData, performance(f32::MAX))]);
    assert_eq!(Customization::from_vehicle(&c, &d, 1), Err(Error::Bound));
}

#[test]
fn groups_are_unsigned_sorted_unique_and_use_static_asset_gates() {
    let (mut c, d) = fixture(vec![
        (
            BrakesItemData,
            Input::Performance {
                group_a: Some(200),
                value: 0.0,
                clutch_group: None,
                induction: None,
            },
        ),
        (
            SpoilerItemData,
            Input::Appearance {
                spoiler_group: Some(2),
                group_b: Some(11),
                flag: true,
                rim: None,
            },
        ),
        (
            ClutchItemData,
            Input::Performance {
                clutch_group: Some(255),
                value: 0.0,
                group_a: None,
                induction: None,
            },
        ),
        (BumperItemData, appearance(Some(11), false)),
        (BrakesItemData, performance(0.0)),
    ]);
    let result = build(&c, &d);
    assert_eq!(result.list_a, [2, 200]);
    assert_eq!(result.list_b, [11, 255]);
    assert!(result.flag);
    c.items.get_mut(&1).unwrap().children.reverse();
    assert_eq!(build(&c, &d), result);
}

#[test]
fn rim_selection_clamps_rounds_and_later_recursive_parts_win() {
    let (mut c, d) = fixture(vec![
        (RimsItemData, rim(false, vec![0.461, 0.384])),
        (RimsItemData, rim(true, vec![0.8])),
        (RimsItemData, rim(false, vec![-1.0, 1.0])),
    ]);
    c.items.get_mut(&1).unwrap().children = vec![2, 3];
    c.items.get_mut(&2).unwrap().children = vec![4];
    c.items.get_mut(&4).unwrap().owner = 2;
    assert_eq!(build(&c, &d).values, [0, 65535]);
    c.items.get_mut(&2).unwrap().children.clear();
    assert_eq!(build(&c, &d).values, [37765, 65535]);
    if let Derived::Rims { rim_selection, .. } = &mut c.items.get_mut(&2).unwrap().derived {
        *rim_selection = 1;
    }
    assert_eq!(build(&c, &d).values, [31457, 65535]);
    if let Derived::Rims { rim_selection, .. } = &mut c.items.get_mut(&2).unwrap().derived {
        *rim_selection = 2;
    }
    assert_eq!(Customization::from_vehicle(&c, &d, 1), Err(Error::Bound));
    assert_eq!(quantize_rim(1.0), 65535);
    assert_eq!(quantize_rim(0.4), 32768);
}

#[test]
fn absent_rim_array_skips_selection_and_induction_zero_is_present() {
    let (mut c, d) = fixture(vec![
        (RimsItemData, rim(false, vec![])),
        (
            ForcedInductionItemData,
            Input::Performance {
                induction: Some(63),
                value: 0.0,
                group_a: None,
                clutch_group: None,
            },
        ),
        (
            ForcedInductionItemData,
            Input::Performance {
                induction: Some(0),
                value: 0.0,
                group_a: None,
                clutch_group: None,
            },
        ),
    ]);
    if let Derived::Rims { rim_selection, .. } = &mut c.items.get_mut(&2).unwrap().derived {
        *rim_selection = u32::MAX;
    }
    assert_eq!(build(&c, &d).values, [0, 0]);
    assert_eq!(build(&c, &d).index, Some(0));
    c.items.get_mut(&1).unwrap().children.reverse();
    assert_eq!(build(&c, &d).index, Some(63));
}

#[test]
fn rebuilding_after_part_removal_resets_state_without_affecting_another_car() {
    let (mut c, d) = fixture(vec![(RollCageItemData, appearance(Some(4), true))]);
    let mut other = c.items[&1].clone();
    other.id = 10;
    other.children.clear();
    c.items.insert(10, other);
    assert!(build(&c, &d).flag);
    let isolated = Customization::from_vehicle(&c, &d, 10).unwrap();
    assert!(!isolated.flag);
    assert!(isolated.list_b.is_empty());
    assert_eq!(build(&c, &d), build(&c, &d));
    c.items.get_mut(&1).unwrap().children.clear();
    assert_eq!(build(&c, &d), isolated);
}

#[test]
fn invalid_ownership_duplicate_children_missing_content_and_wire_count_fail() {
    let (mut c, mut d) = fixture(vec![(BumperItemData, appearance(Some(3), false))]);
    c.items.get_mut(&2).unwrap().owner = 99;
    assert_eq!(Customization::from_vehicle(&c, &d, 1), Err(Error::Shape));
    c.items.get_mut(&2).unwrap().owner = 1;
    c.items.get_mut(&1).unwrap().children.push(2);
    assert_eq!(
        Customization::from_vehicle(&c, &d, 1),
        Err(Error::DuplicateObject)
    );
    c.items.get_mut(&1).unwrap().children.pop();
    d.inputs.remove(&c.items[&2].definition);
    assert_eq!(
        Customization::from_vehicle(&c, &d, 1),
        Err(Error::UnknownObject)
    );
    let (c, d) = fixture(
        (0..=255)
            .map(|g| (BumperItemData, appearance(Some(g), false)))
            .collect(),
    );
    assert_eq!(Customization::from_vehicle(&c, &d, 1), Err(Error::Bound));
}

#[test]
fn content_rejects_wrong_families_shape_nonfinite_values_and_duplicates() {
    let (c, d) = fixture(vec![(AirFilterItemData, performance(0.0))]);
    let guid = c.items[&2].definition;
    for value in [f32::NAN, f32::INFINITY, -0.1] {
        assert_eq!(
            Definitions::new(d.tuning.clone(), [(guid, performance(value))]),
            Err(Error::Bound)
        );
    }
    assert_eq!(
        Definitions::new(d.tuning.clone(), [(guid, Input::Other)]),
        Err(Error::TypeMismatch)
    );
    assert_eq!(
        Definitions::new(d.tuning.clone(), vec![(guid, performance(0.0)); 2]),
        Err(Error::DuplicateObject)
    );
    assert_eq!(
        Definitions::new(
            d.tuning.clone(),
            [(
                guid,
                Input::Performance {
                    induction: Some(0),
                    value: 0.0,
                    group_a: None,
                    clutch_group: None,
                }
            )]
        ),
        Err(Error::Shape)
    );
    let (c, d) = fixture(vec![(RimsItemData, rim(false, vec![]))]);
    let guid = c.items[&2].definition;
    for values in [vec![f32::NAN], vec![0.0; 257]] {
        assert_eq!(
            Definitions::new(d.tuning.clone(), [(guid, rim(false, values))]),
            Err(Error::Bound)
        );
    }
    assert_eq!(
        Definitions::new(d.tuning, [(guid, appearance(None, false))]),
        Err(Error::Shape)
    );
}
