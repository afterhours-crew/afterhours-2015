// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::{Definition, DefinitionFlags, Layout, OwnershipLevel};
use std::collections::BTreeMap;

fn guid(n: u8) -> Guid {
    [n; 16]
}
fn fixture() -> (InventoryCatalog, Offers, wheel_customization::Definitions) {
    use DefinitionClass::*;
    let entries = [
        RaceVehicleItemData,
        RimsItemData,
        RimsItemData,
        ControlArmTuningItemData,
        HoodItemData,
        BumperItemData,
        BumperItemData,
        RimsItemData,
        BodyKitItemData,
        HoodItemData,
        ControlArmTuningItemData,
        RimsItemData,
        NosTuningItemData,
        BumperItemData,
        HoodItemData,
        BodyKitItemData,
        BumperItemData,
    ];
    let definitions = entries.into_iter().enumerate().map(|(i, class)| {
        let id = i as u8 + 1;
        let bytes = match class.layout() {
            Layout::RaceVehicle => 68,
            Layout::Rims => 52,
            Layout::ControlArmTuning => 36,
            Layout::NosTuning => 4,
            Layout::Empty => 0,
            _ => panic!("fixture"),
        };
        let children = match id {
            1 => vec![2, 3, 4, 5, 6, 7],
            9 => vec![14, 15],
            16 => vec![17],
            _ => vec![],
        };
        (
            guid(id),
            Definition {
                class,
                buy_price: 0,
                sell_price: 0,
                quantity: 1,
                ownership_level: OwnershipLevel::Owned,
                flags: DefinitionFlags {
                    requires_owner: id != 1,
                    ..Default::default()
                },
                sub_items: children.into_iter().map(guid).collect(),
                additional_items: vec![],
                default_derived: Some(Derived::decode(class.layout(), &vec![0; bytes]).unwrap()),
            },
        )
    });
    let catalog = InventoryCatalog::new(definitions, vec![]).unwrap();
    // The presets deliberately have no purchasable offers. Only installed slot
    // metadata is needed; a persona does not purchase or unlock its parts.
    let slots = Offers::new([])
        .unwrap()
        .with_vehicle_scopes(&catalog, [])
        .unwrap()
        .with_part_policy(
            &catalog,
            [
                (2, 0),
                (3, 1),
                (6, 0),
                (7, 1),
                (8, 0),
                (12, 0),
                (14, 0),
                (17, 1),
            ]
            .map(|(id, at)| (guid(id), at)),
            [],
        )
        .unwrap();
    let wheels = wheel_customization::Definitions::new(
        catalog.bindings().clone(),
        [2, 3, 8, 12].map(|id| {
            (
                guid(id),
                wheel_customization::Input {
                    rear: id == 3,
                    geometry: wheel_customization::Geometry::Rim(wheel_customization::Rim {
                        diameter: 0.5,
                        width: 0.2,
                        scale: 1.,
                        sizes: vec![0.5, 0.6, 0.7],
                        flag: false,
                    }),
                },
            )
        }),
    )
    .unwrap();
    (catalog, slots, wheels)
}
fn selected(items: &Collection) -> Vec<u8> {
    items.items[&1]
        .children
        .iter()
        .map(|id| items.items[id].definition[0])
        .collect()
}

#[test]
fn fixed_size_rim_preserves_native_minus_one_selection() {
    let (catalog, slots, _) = fixture();
    let wheels = wheel_customization::Definitions::new(
        catalog.bindings().clone(),
        [2, 3].map(|id| {
            (
                guid(id),
                wheel_customization::Input {
                    rear: id == 3,
                    geometry: wheel_customization::Geometry::Rim(wheel_customization::Rim {
                        diameter: 0.5,
                        width: 0.2,
                        scale: 1.,
                        sizes: vec![],
                        flag: false,
                    }),
                },
            )
        }),
    )
    .unwrap();
    let mut style = Style::default();
    style.rims[0].selection = Some(4);
    style.rims[1].selection = Some(0);
    let preset = Preset {
        vehicle: guid(1),
        color: None,
        style: Some(style),
        additional: vec![],
    };
    let items = preset.instantiate(&catalog, &slots, &wheels).unwrap();
    for id in [2, 3] {
        let Derived::Rims { rim_selection, .. } = items.items[&id].derived else {
            panic!("rim")
        };
        assert_eq!(rim_selection, u32::MAX);
    }
}

#[test]
fn ordered_styles_restore_uncovered_kit_slots_and_additions_follow_style_values() {
    let (catalog, slots, wheels) = fixture();
    let baseline = catalog.clone();
    let mut style = Style {
        parts: [8, 9, 10, 16].map(guid).to_vec(),
        color: Some([0.1, 0.2, 0.3]),
        material: Some([1., 0.5, 1.]),
        window_tint: Some(0.8),
        license_plate: Some(*b"NIGHT\0\0\0"),
        alignment: [Some(7); 9],
        ..Default::default()
    };
    style.rims = std::array::from_fn(|i| RimStyle {
        selection: Some(99),
        primary_color: Some([i as f32; 3]),
        primary_material: Some([0.2; 3]),
        secondary_color: Some([0.3; 3]),
        secondary_material: Some([0.9; 3]),
    });
    let preset = Preset {
        vehicle: guid(1),
        color: Some([0.9; 3]),
        style: Some(style),
        additional: vec![guid(12), guid(13)],
    };
    let actual = preset.instantiate(&catalog, &slots, &wheels).unwrap();
    assert_eq!(selected(&actual), [3, 4, 10, 16, 6, 12, 13]);
    let Derived::RaceVehicle {
        default_paint,
        material,
        window_tint,
        default_license_plate_text,
        ..
    } = actual.items[&1].derived
    else {
        panic!("car")
    };
    assert_eq!(default_paint, [0.1_f32, 0.2, 0.3].map(f32::to_bits));
    assert_eq!(material, [1_f32, 0.5, 1.].map(f32::to_bits));
    assert_eq!(window_tint, 0.8_f32.to_bits());
    assert_eq!(default_license_plate_text, *b"NIGHT\0\0\0");
    let current: BTreeMap<_, _> = actual.items[&1]
        .children
        .iter()
        .map(|id| (actual.items[id].definition[0], &actual.items[id]))
        .collect();
    let Derived::Rims {
        rim_selection,
        primary_paint,
        primary_material,
        secondary_material,
        ..
    } = current[&3].derived
    else {
        panic!("rear rim")
    };
    assert_eq!(rim_selection, 2);
    assert_eq!(primary_paint, [1_f32; 3].map(f32::to_bits));
    assert_eq!(primary_material, [0.2_f32; 3].map(f32::to_bits));
    assert_eq!(secondary_material, primary_material);
    let Derived::Rims {
        rim_selection,
        primary_material,
        ..
    } = current[&12].derived
    else {
        panic!("additional rim")
    };
    assert_eq!(rim_selection, 0);
    assert_eq!(primary_material, [0; 3]);
    let Derived::ControlArmTuning {
        caster_setting,
        track_width_rear_setting,
        ..
    } = current[&4].derived
    else {
        panic!("alignment")
    };
    assert_eq!((caster_setting, track_width_rear_setting), (7, 7));
    assert_eq!(catalog, baseline);
    assert_eq!(
        preset.instantiate(&catalog, &slots, &wheels).unwrap(),
        actual
    );
    assert_eq!(
        catalog
            .instantiate_vehicle(guid(1), 1)
            .unwrap()
            .collection
            .items[&1]
            .children
            .len(),
        6
    );
}

#[test]
fn absent_style_preserves_stock_and_invalid_or_late_failed_setup_does_not_escape() {
    let (catalog, slots, wheels) = fixture();
    let preset = Preset {
        vehicle: guid(1),
        color: None,
        style: None,
        additional: vec![],
    };
    let expected = catalog.instantiate_vehicle(guid(1), 1).unwrap().collection;
    assert_eq!(
        preset.instantiate(&catalog, &slots, &wheels).unwrap(),
        expected
    );
    let mut invalid = preset.clone();
    invalid.additional = vec![guid(8), guid(255)];
    assert!(invalid.instantiate(&catalog, &slots, &wheels).is_err());
    invalid.additional = vec![guid(8); MAX_PARTS + 1];
    assert!(invalid.instantiate(&catalog, &slots, &wheels).is_err());
    invalid.additional.clear();
    invalid.color = Some([f32::NAN; 3]);
    assert!(invalid.instantiate(&catalog, &slots, &wheels).is_err());
    invalid.color = None;
    invalid.style = Some(Style {
        alignment: [Some(11); 9],
        ..Default::default()
    });
    assert!(invalid.instantiate(&catalog, &slots, &wheels).is_err());
    assert_eq!(
        preset.instantiate(&catalog, &slots, &wheels).unwrap(),
        expected
    );
}
