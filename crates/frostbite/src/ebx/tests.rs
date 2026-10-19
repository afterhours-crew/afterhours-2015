// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{Field, PartitionWriter};

const PART: [u8; 16] = [0x11; 16];
const INST: [u8; 16] = [0x22; 16];
const OTHER: ([u8; 16], [u8; 16]) = ([0x33; 16], [0x44; 16]);

fn sample() -> Vec<u8> {
    PartitionWriter::default().build(
        PART,
        "SampleItemData",
        INST,
        vec![
            ("Value", Field::Int(-42)),
            ("Count", Field::UInt(0xFFFF_FFFF)),
            ("Ratio", Field::Float(0.25)),
            ("Enabled", Field::Bool(true)),
            ("Name", Field::CString("Items/Sample".into())),
            ("Level", Field::Enum("LevelType", "LevelType_Owned", 1)),
            ("Parent", Field::Import(OTHER.0, OTHER.1)),
            (
                "Children",
                Field::Imports(vec![OTHER, ([0x55; 16], [0x66; 16])]),
            ),
            (
                "Paint",
                Field::Struct(
                    "Vec3",
                    vec![
                        ("x", Field::Float(1.0)),
                        ("y", Field::Float(0.5)),
                        ("z", Field::Float(0.0)),
                    ],
                ),
            ),
            (
                "Sizes",
                Field::Structs(
                    "Size",
                    vec![
                        vec![("Width", Field::Float(2.0)), ("Tag", Field::Int(7))],
                        vec![("Width", Field::Float(3.0)), ("Tag", Field::Int(8))],
                    ],
                ),
            ),
        ],
    )
}

#[test]
fn nested_struct_arrays_preserve_empty_and_different_length_rows() {
    let rows = [vec![], vec![2., 3.], vec![9.]]
        .into_iter()
        .map(|values| {
            vec![
                ("Before", Field::Int(7)),
                (
                    "Path",
                    Field::Struct(
                        "Path",
                        vec![(
                            "Points",
                            Field::Structs(
                                "Point",
                                values
                                    .into_iter()
                                    .map(|value| vec![("x", Field::Float(value))])
                                    .collect(),
                            ),
                        )],
                    ),
                ),
                ("After", Field::Int(11)),
            ]
        })
        .collect();
    let data = PartitionWriter::default().build(
        PART,
        "Nested",
        INST,
        vec![("Rows", Field::Structs("Row", rows))],
    );
    let limits = Limits::default();
    let part = Partition::parse(&data, &limits).unwrap();
    let objects = part.objects(&limits).unwrap();
    let rows = objects[0].fields.get("Rows").unwrap().as_array().unwrap();
    for (row, expected) in rows.iter().zip([vec![], vec![2., 3.], vec![9.]]) {
        let fields = row.as_fields().unwrap();
        assert_eq!(fields.get("Before"), Some(&Value::Int32(7)));
        assert_eq!(fields.get("After"), Some(&Value::Int32(11)));
        let points = fields
            .get("Path")
            .unwrap()
            .as_fields()
            .unwrap()
            .get("Points")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(points.len(), expected.len());
        for (point, value) in points.iter().zip(expected) {
            assert_eq!(
                point.as_fields().unwrap().get("x"),
                Some(&Value::Float32(value))
            );
        }
    }
}

#[test]
fn constructed_partition_decodes_every_field_kind() {
    let data = sample();
    let limits = Limits::default();
    let part = Partition::parse(&data, &limits).unwrap();
    assert_eq!(part.guid, PART);
    assert_eq!(part.primary_class(), Some("SampleItemData"));
    let objects = part.objects(&limits).unwrap();
    assert_eq!(objects.len(), 1);
    let object = &objects[0];
    assert_eq!(object.guid, Some(INST));
    let f = &object.fields;
    assert_eq!(f.get("Value"), Some(&Value::Int32(-42)));
    assert_eq!(f.get("Value").and_then(Value::bits32), Some(0xFFFF_FFD6));
    assert_eq!(f.get("Count").and_then(Value::bits32), Some(u32::MAX));
    assert_eq!(
        f.get("Ratio").and_then(Value::bits32),
        Some(0.25f32.to_bits())
    );
    assert_eq!(f.get("Enabled").and_then(Value::as_bool), Some(true));
    assert_eq!(f.get("Name").and_then(Value::as_str), Some("Items/Sample"));
    assert_eq!(
        f.get("Level").and_then(Value::as_enum_name),
        Some("LevelType_Owned")
    );
    assert_eq!(
        f.get("Parent").and_then(Value::as_pointer),
        Some(Pointer::Import {
            partition: OTHER.0,
            instance: OTHER.1
        })
    );
    let children = f.get("Children").and_then(Value::as_array).unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(
        children[1].as_pointer(),
        Some(Pointer::Import {
            partition: [0x55; 16],
            instance: [0x66; 16]
        })
    );
    let paint = f.get("Paint").and_then(Value::as_fields).unwrap();
    assert_eq!(paint.get("y"), Some(&Value::Float32(0.5)));
    let sizes = f.get("Sizes").and_then(Value::as_array).unwrap();
    assert_eq!(
        sizes[1].as_fields().and_then(|s| s.get("Tag")),
        Some(&Value::Int32(8))
    );
    assert_eq!(part.imports.len(), 2);
}

#[test]
fn bad_magic_and_truncation_fail() {
    assert!(matches!(
        Partition::parse(&[0u8; 64], &Limits::default()),
        Err(Error::Malformed(_))
    ));
    let data = sample();
    for cut in [10, 63, 64, 100, data.len() - 1] {
        let truncated = &data[..cut];
        let parsed = Partition::parse(truncated, &Limits::default());
        assert!(
            parsed.is_err() || parsed.unwrap().objects(&Limits::default()).is_err(),
            "cut at {cut} decoded"
        );
    }
}

#[test]
fn field_offset_overrun_fails() {
    // Scalar-only partition: the first field descriptor is the primary's first field.
    let mut data = PartitionWriter::default().build(
        PART,
        "Plain",
        INST,
        vec![
            ("Value", Field::Int(1)),
            ("Name", Field::CString("x".into())),
        ],
    );
    let names_len = usize::from(u16::from_le_bytes([data[26], data[27]]));
    let first_field = 64 + names_len;
    data[first_field + 8..first_field + 12].copy_from_slice(&(1u32 << 20).to_le_bytes());
    let part = Partition::parse(&data, &Limits::default()).unwrap();
    assert!(matches!(
        part.objects(&Limits::default()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn bounds_reject_arrays_values_and_size() {
    let data = sample();
    let narrow = Limits {
        max_elements: 1,
        ..Limits::default()
    };
    assert!(matches!(
        Partition::parse(&data, &narrow).and_then(|p| p.objects(&narrow)),
        Err(Error::Bound(_))
    ));
    let few = Limits {
        max_values: 5,
        ..Limits::default()
    };
    assert!(matches!(
        Partition::parse(&data, &few).unwrap().objects(&few),
        Err(Error::Bound(_))
    ));
    let small = Limits {
        max_asset_bytes: data.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(
        Partition::parse(&data, &small),
        Err(Error::Bound(_))
    ));
}

#[test]
fn instance_layout_mismatch_fails() {
    let mut data = sample();
    // Declare 64 more data bytes than the instance occupies.
    let data_len = u32::from_le_bytes(data[36..40].try_into().unwrap());
    data[36..40].copy_from_slice(&(data_len + 64).to_le_bytes());
    data.extend([0u8; 64]);
    let part = Partition::parse(&data, &Limits::default()).unwrap();
    assert!(matches!(
        part.objects(&Limits::default()),
        Err(Error::Malformed("EBX instance layout"))
    ));
}

#[test]
fn cyclic_array_members_hit_the_depth_bound() {
    let mut data = PartitionWriter::default().build(
        PART,
        "Root",
        INST,
        vec![("Children", Field::Imports(vec![OTHER]))],
    );
    let imports = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    let names = u16::from_le_bytes(data[26..28].try_into().unwrap()) as usize;
    let member = 64 + imports * 32 + names;
    // Array 0's member is itself another reference to array 0.
    data[member + 4..member + 6].copy_from_slice(&0x40u16.to_le_bytes());
    let arrays_at = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize
        + u32::from_le_bytes(data[28..32].try_into().unwrap()) as usize
        + u32::from_le_bytes(data[36..40].try_into().unwrap()) as usize;
    data[arrays_at..arrays_at + 4].copy_from_slice(&0u32.to_le_bytes());
    for max_depth in [0, 1, 8, Limits::default().max_depth] {
        let limits = Limits {
            max_depth,
            ..Limits::default()
        };
        assert!(matches!(
            Partition::parse(&data, &limits).unwrap().objects(&limits),
            Err(Error::Bound("EBX nesting depth"))
        ));
    }
}
