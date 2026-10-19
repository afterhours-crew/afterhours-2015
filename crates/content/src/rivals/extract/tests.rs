// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, local};
use nfs_frostbite::ebx::{Fields, Pointer};
fn v(x: f32) -> Value {
    Value::Struct(Fields(
        ["x", "y", "z"]
            .map(|k| (k.into(), Value::Float32(x)))
            .to_vec(),
    ))
}

#[test]
fn provider_tags_use_hash_identity_and_name_id_precedes_literal_name() {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "PersonaEntityData",
        &[
            ("PersonaName", Value::String("Literal".into())),
            ("PersonaNameId", Value::String("ID_TEST_PERSONA".into())),
            ("VehicleTags", Value::Array(vec![local(1), local(2)])),
            ("VehicleItemData", local(3)),
            ("VehicleStyle", Value::Pointer(Pointer::Null)),
            ("AdditionalItems", Value::Array(vec![])),
            ("OverrideCarColor", Value::Bool(false)),
            ("OverridePerformance", Value::Bool(false)),
            ("OverrideGripValue", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        1,
        "VehicleTag",
        &[
            ("Name", Value::String("First".into())),
            ("TagHash", Value::UInt32(123)),
        ],
    );
    m.put(
        "a",
        2,
        "VehicleTag",
        &[
            ("Name", Value::String("Alias".into())),
            ("TagHash", Value::UInt32(123)),
        ],
    );
    m.put("a", 3, "RaceVehicleItemData", &[]);
    m.0.get_mut(&("a".into(), 3)).unwrap().guid = Some([7; 16]);
    let mut g = Graph::new(&mut m);
    let key = ("a".into(), 0);
    let object = g.source.object(&key).unwrap();
    assert_eq!(tags(&mut g, &key, &object).unwrap(), BTreeSet::from([123]));
    let p = persona(&mut g, &key, 19).unwrap();
    assert_eq!(p.name, "ID_TEST_PERSONA");
    assert_eq!(p.provider_index, 19);
    assert_eq!(p.preset.vehicle, [7; 16]);
    assert_eq!(p.driving, Default::default());
    g.source
        .0
        .get_mut(&key)
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "PersonaNameId")
        .unwrap()
        .1 = Value::String(String::new());
    assert_eq!(persona(&mut g, &key, 19).unwrap().name, "Literal");
    g.source.0.get_mut(&("a".into(), 1)).unwrap().class = "Unrelated".into();
    assert!(tags(&mut g, &key, &object).is_err());
}

#[test]
fn style_sentinels_preserve_absence_and_plate_is_bounded() {
    let mut fields = vec![
        ("SubItems", Value::Array(vec![])),
        ("WindowTint", Value::Float32(-1.)),
        ("LicensePlate", Value::String("NIGHTSHIFT".into())),
    ];
    for key in [
        "VehicleColor",
        "VehicleMaterial",
        "RimPrimaryColorFront",
        "RimPrimaryMaterialFront",
        "RimSecondaryColorFront",
        "RimSecondaryMaterialFront",
        "RimPrimaryColorRear",
        "RimPrimaryMaterialRear",
        "RimSecondaryColorRear",
        "RimSecondaryMaterialRear",
    ] {
        fields.push((key, v(-1.)));
    }
    for key in [
        "Caster",
        "CamberFront",
        "CamberRear",
        "RideHeight",
        "RideHeightRake",
        "ToeFront",
        "ToeRear",
        "TrackWidthFront",
        "TrackWidthRear",
        "RimSizeFront",
        "RimSizeRear",
    ] {
        fields.push((key, Value::UInt32(u32::MAX)));
    }
    let mut m = Memory::default();
    m.put("a", 0, "StyleItemData", &fields);
    let mut g = Graph::new(&mut m);
    let key = ("a".into(), 0);
    let expected = Style {
        license_plate: Some(*b"NIGHTSHI"),
        ..Default::default()
    };
    assert_eq!(style(&mut g, &key).unwrap(), expected);
    let o = g.source.0.get_mut(&key).unwrap();
    o.fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "WindowTint")
        .unwrap()
        .1 = Value::Float32(-0.99999);
    o.fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "VehicleColor")
        .unwrap()
        .1 = v(-0.99999);
    assert_eq!(style(&mut g, &key).unwrap(), expected);
    g.source
        .0
        .get_mut(&key)
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "RimSizeFront")
        .unwrap()
        .1 = Value::UInt32(4);
    assert_eq!(style(&mut g, &key).unwrap().rims[0].selection, Some(4));
    g.source
        .0
        .get_mut(&key)
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == "VehicleColor")
        .unwrap()
        .1 = v(f32::NAN);
    assert!(style(&mut g, &key).is_err());
}
