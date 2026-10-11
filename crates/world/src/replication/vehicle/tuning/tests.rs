// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::items::{Item, Layout};

fn item(id: u64, owner: u64, derived: Derived) -> Item {
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
fn definitions(c: &Collection) -> Definitions {
    let classes = Catalog::new(c.items.values().map(|item| {
        let class = DefinitionClass::ALL
            .into_iter()
            .filter(|class| class.layout() == item.derived.layout())
            .min_by_key(|&class| !applies(class))
            .unwrap();
        (item.definition, class)
    }))
    .unwrap();
    let entries = classes
        .definitions()
        .filter_map(|(guid, class)| {
            applies(class).then_some((
                *guid,
                PartInput {
                    asset_present: true,
                    transmission_values: (class == DefinitionClass::PersistantTuningItemData)
                        .then_some([10, 0]),
                },
            ))
        })
        .collect::<Vec<_>>();
    Definitions::new(classes, entries).unwrap()
}
fn generated(c: &Collection, vehicle: u64) -> Result<Settings, Error> {
    Settings::from_vehicle(c, &definitions(c), vehicle)
}
fn collection(parts: Vec<Derived>) -> Collection {
    let mut car = item(
        1,
        0,
        Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap(),
    );
    car.children = (2..2 + parts.len() as u64).collect();
    let mut items = std::collections::BTreeMap::from([(1, car)]);
    for (index, part) in parts.into_iter().enumerate() {
        let id = index as u64 + 2;
        items.insert(id, item(id, 1, part));
    }
    Collection {
        roots: vec![1],
        items,
    }
}
#[test]
fn tuning_uses_current_children_and_keeps_constructor_values_for_absent_fields() {
    let mut c = collection(vec![
        Derived::ControlArmTuning {
            caster_setting: 1,
            camber_front_setting: 2,
            camber_rear_setting: 3,
            ride_height_height_setting: 4,
            ride_height_rake_setting: 6,
            toe_front_setting: 7,
            toe_rear_setting: 8,
            track_width_front_setting: 9,
            track_width_rear_setting: 10,
        },
        Derived::PersistantTuning {
            abs_setting: 0,
            stability_control_setting: 10,
            traction_control_setting: 0,
            air_pressure_front_setting: 11,
            air_pressure_rear_setting: 12,
        },
    ]);
    c.items.get_mut(&1).unwrap().defaults = vec![99];
    c.items.insert(
        99,
        item(
            99,
            1,
            Derived::ControlArmTuning {
                caster_setting: 15,
                camber_front_setting: 15,
                camber_rear_setting: 15,
                ride_height_height_setting: 15,
                ride_height_rake_setting: 15,
                toe_front_setting: 15,
                toe_rear_setting: 15,
                track_width_front_setting: 15,
                track_width_rear_setting: 15,
            },
        ),
    );
    let mut settings = generated(&c, 1).unwrap();
    assert_eq!(
        settings.values(),
        [
            5, 5, 5, 5, 5, 0, 10, 0, 5, 5, 4, 6, 9, 10, 1, 2, 3, 7, 8, 5, 5, 5, 5, 5, 5, 5, 11, 12,
            5, 5, 5, 0
        ]
    );
    settings.set(Setting::ManualTransmission, 0).unwrap();
    assert_eq!(settings.values()[31], 0);
    assert_eq!(generated(&c, 1).unwrap().values()[31], 0);
}
#[test]
fn tuning_later_eligible_parts_replace_earlier_values() {
    let brake = Derived::BrakeDiscs {
        brake_strength_setting: 8,
        brake_bias_setting: 3,
    };
    let mut c = collection(vec![brake.clone(), brake]);
    let first = generated(&c, 1).unwrap();
    c.items.get_mut(&1).unwrap().children.reverse();
    assert_eq!(generated(&c, 1), Ok(first));
    c.items.get_mut(&2).unwrap().derived = Derived::BrakeDiscs {
        brake_strength_setting: 7,
        brake_bias_setting: 3,
    };
    assert_eq!(generated(&c, 1).unwrap().values()[1], 7);
    c.items.get_mut(&1).unwrap().children.reverse();
    assert_eq!(generated(&c, 1).unwrap().values()[1], 8);
}
#[test]
fn tuning_rejects_missing_duplicate_foreign_and_unowned_parts() {
    let original = collection(vec![Derived::NosTuning { nos_setting: 4 }]);
    let mut c = original.clone();
    c.items.remove(&2);
    assert_eq!(generated(&c, 1), Err(Error::UnknownObject));
    let mut c = original.clone();
    c.items.get_mut(&1).unwrap().children.push(2);
    assert_eq!(generated(&c, 1), Err(Error::DuplicateObject));
    let mut c = original.clone();
    c.items.get_mut(&2).unwrap().owner = 9;
    assert_eq!(generated(&c, 1), Err(Error::Shape));
    let mut c = original;
    c.items.get_mut(&2).unwrap().state = crate::items::PURCHASABLE;
    assert_eq!(generated(&c, 1), Err(Error::Shape));
}
#[test]
fn tuning_rejects_wrong_roots_and_bounds_without_mutating_input() {
    let c = collection(vec![Derived::NosTuning { nos_setting: 16 }]);
    assert_eq!(generated(&c, 1), Err(Error::Bound));
    assert_eq!(c.items[&2].derived, Derived::NosTuning { nos_setting: 16 });
    assert_eq!(generated(&c, 2), Err(Error::Shape));
    let mut wrong = c.clone();
    wrong.items.get_mut(&1).unwrap().derived = Derived::Empty;
    assert_eq!(generated(&wrong, 1), Err(Error::TypeMismatch));
    let mut too_many = c;
    too_many.items.get_mut(&1).unwrap().children = vec![2; MAX_COLLECTION + 1];
    assert_eq!(generated(&too_many, 1), Err(Error::Bound));
    let mut settings = Settings::default();
    assert_eq!(settings.set(Setting::Nos, u32::MAX), Err(Error::Bound));
    assert_eq!(settings, Settings::default());
}
#[test]
fn tuning_keeps_player_collections_isolated_and_reflects_changes() {
    let a = collection(vec![Derived::NosTuning { nos_setting: 1 }]);
    let mut b = collection(vec![Derived::NosTuning { nos_setting: 9 }]);
    assert_eq!(generated(&a, 1).unwrap().values()[28], 1);
    assert_eq!(generated(&b, 1).unwrap().values()[28], 9);
    b.items.get_mut(&2).unwrap().derived = Derived::NosTuning { nos_setting: 2 };
    assert_eq!(generated(&b, 1).unwrap().values()[28], 2);
    assert_eq!(generated(&a, 1).unwrap().values()[28], 1);
}

#[test]
fn tuning_maps_item_wire_order_to_reflected_vehicle_order() {
    let parts: &[(Layout, &[u32])] = &[
        (Layout::Spoiler, &[0]),
        (Layout::BrakeDiscs, &[1, 2]),
        (Layout::HandbrakeTuning, &[3]),
        (Layout::DifferentialTuning, &[4]),
        (Layout::PersistantTuning, &[5, 6, 7, 10, 11]),
        (Layout::SteeringTuning, &[8, 9]),
        (Layout::ControlArmTuning, &[14, 15, 0, 10, 11, 1, 2, 12, 13]),
        (Layout::SuspensionTuning, &[3, 4, 5, 6]),
        (Layout::SwaybarTuning, &[7, 8]),
        (Layout::TireComposition, &[9]),
        (Layout::NosTuning, &[12]),
        (Layout::GearboxTuning, &[13]),
    ];
    let values = parts
        .iter()
        .map(|(layout, words)| {
            let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
            Derived::decode(*layout, &bytes).unwrap()
        })
        .collect();
    let actual = generated(&collection(values), 1).unwrap().values();
    assert_eq!(
        &actual[..30],
        &(0..30).map(|i| (i % 16) as u8).collect::<Vec<_>>()
    );
    assert_eq!(&actual[30..], &[5, 0]);
}

#[test]
fn cosmetic_shared_layouts_and_missing_assets_do_not_override_tuning() {
    let c = collection(vec![
        Derived::TireComposition { tire_setting: 0 },
        Derived::TireComposition { tire_setting: 15 },
        Derived::BrakeDiscs {
            brake_strength_setting: 3,
            brake_bias_setting: 4,
        },
        Derived::BrakeDiscs {
            brake_strength_setting: 5,
            brake_bias_setting: 5,
        },
        Derived::NosTuning { nos_setting: 12 },
    ]);
    let mut defs = definitions(&c);
    defs.classes = Catalog::new(defs.classes.definitions().map(|(guid, class)| {
        (
            *guid,
            if *guid == c.items[&3].definition {
                DefinitionClass::TiresItemData
            } else if *guid == c.items[&5].definition {
                DefinitionClass::BrakeDiscsItemData
            } else {
                class
            },
        )
    }))
    .unwrap();
    defs.parts.remove(&c.items[&3].definition);
    defs.parts.remove(&c.items[&5].definition);
    defs.parts
        .get_mut(&c.items[&6].definition)
        .unwrap()
        .asset_present = false;
    let values = Settings::from_vehicle(&c, &defs, 1).unwrap().values();
    assert_eq!(values[25], 0);
    assert_eq!(&values[1..3], &[3, 4]);
    assert_eq!(values[28], 5);
    defs.parts.remove(&c.items[&2].definition);
    assert_eq!(
        Settings::from_vehicle(&c, &defs, 1),
        Err(Error::UnknownObject)
    );
}

#[test]
fn tuning_walks_nested_current_parts_in_preorder_and_rejects_cycles_or_depth() {
    let mut c = collection(vec![
        Derived::Empty,
        Derived::NosTuning { nos_setting: 2 },
        Derived::NosTuning { nos_setting: 9 },
    ]);
    c.items.get_mut(&1).unwrap().children = vec![2, 4];
    c.items.get_mut(&2).unwrap().children = vec![3];
    c.items.get_mut(&3).unwrap().owner = 2;
    assert_eq!(generated(&c, 1).unwrap().values()[28], 9);
    c.items.get_mut(&1).unwrap().children = vec![4, 2];
    assert_eq!(generated(&c, 1).unwrap().values()[28], 2);
    c.items.get_mut(&3).unwrap().children = vec![1];
    assert_eq!(generated(&c, 1), Err(Error::DuplicateObject));
    c.items.get_mut(&3).unwrap().children.clear();
    for id in 5..5 + MAX_DEPTH as u64 {
        let parent = if id == 5 { 3 } else { id - 1 };
        c.items.get_mut(&parent).unwrap().children = vec![id];
        c.items.insert(id, item(id, parent, Derived::Empty));
    }
    assert_eq!(generated(&c, 1), Err(Error::Bound));
}

#[test]
fn transmission_uses_vehicle_flag_and_first_recursive_persistent_asset() {
    let persistent = Derived::decode(Layout::PersistantTuning, &[0; 20]).unwrap();
    let mut c = collection(vec![persistent.clone(), persistent]);
    let mut defs = definitions(&c);
    defs.parts
        .get_mut(&c.items[&3].definition)
        .unwrap()
        .transmission_values = Some([13, 3]);
    assert_eq!(
        Settings::from_vehicle(&c, &defs, 1).unwrap().values()[31],
        0
    );
    if let Derived::RaceVehicle {
        vehicle_item_flags, ..
    } = &mut c.items.get_mut(&1).unwrap().derived
    {
        *vehicle_item_flags = 4;
    }
    assert_eq!(
        Settings::from_vehicle(&c, &defs, 1).unwrap().values()[31],
        10
    );
    c.items.get_mut(&1).unwrap().children.reverse();
    assert_eq!(
        Settings::from_vehicle(&c, &defs, 1).unwrap().values()[31],
        13
    );
    let input = defs.parts.get_mut(&c.items[&3].definition).unwrap();
    input.asset_present = false;
    input.transmission_values = None;
    assert_eq!(
        Settings::from_vehicle(&c, &defs, 1),
        Err(Error::Unsupported)
    );
}

#[test]
fn static_tuning_definitions_reject_wrong_classes_shapes_duplicates_and_bounds() {
    let guid = [1; 16];
    let catalog = Catalog::new([(guid, DefinitionClass::PersistantTuningItemData)]).unwrap();
    let input = PartInput {
        asset_present: true,
        transmission_values: Some([10, 0]),
    };
    assert!(Definitions::new(catalog.clone(), [(guid, input)]).is_ok());
    assert_eq!(
        Definitions::new(catalog.clone(), [(guid, input), (guid, input)]),
        Err(Error::DuplicateObject)
    );
    assert_eq!(
        Definitions::new(
            catalog.clone(),
            [(
                guid,
                PartInput {
                    transmission_values: Some([16, 0]),
                    ..input
                }
            )]
        ),
        Err(Error::Bound)
    );
    assert_eq!(
        Definitions::new(
            catalog,
            [(
                guid,
                PartInput {
                    transmission_values: None,
                    ..input
                }
            )]
        ),
        Err(Error::Shape)
    );
    let wrong = Catalog::new([(guid, DefinitionClass::TiresItemData)]).unwrap();
    assert_eq!(
        Definitions::new(wrong, [(guid, input)]),
        Err(Error::TypeMismatch)
    );
}
