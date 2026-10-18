// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, local, ptr};
use nfs_frostbite::ebx::Fields;

fn row(id: i32, access: Access, text: &str) -> Value {
    let (value, name) = match access {
        Access::Source => (0, "FieldAccessType_Source"),
        Access::Target => (1, "FieldAccessType_Target"),
    };
    Value::Struct(Fields(vec![
        ("Id".into(), Value::Int32(id)),
        (
            "AccessType".into(),
            Value::Enum {
                value,
                name: Some(name.into()),
            },
        ),
        ("Value".into(), Value::String(text.into())),
        ("ValueRef".into(), Value::Pointer(Pointer::Null)),
    ]))
}
fn object(rows: Vec<Value>) -> Object {
    let mut memory = Memory::default();
    memory.put(
        "a",
        0,
        "InterfaceDescriptorData",
        &[("Fields", Value::Array(rows))],
    );
    memory.object(&("a".into(), 0)).unwrap()
}

#[test]
fn declarations_preserve_order_signed_ids_empty_and_typed_defaults() {
    let fields = compile(&object(vec![
        row(-3, Access::Source, ""),
        row(4, Access::Target, "Boolean False"),
        row(1, Access::Target, "Float32 -0"),
        row(2, Access::Target, "Float32 1.375"),
    ]))
    .unwrap();
    assert_eq!(
        fields[0],
        Field {
            id: (-3i32) as u32,
            access: Access::Source,
            default: None
        }
    );
    assert_eq!(fields[1].default, Some(Scalar::Boolean(false)));
    let Some(Scalar::Float(zero)) = fields[2].default else {
        panic!()
    };
    assert_eq!(zero.to_bits(), (-0.0f32).to_bits());
    assert_eq!(fields[3].default, Some(Scalar::Float(1.375)));
    assert_eq!(
        default("Boolean True").unwrap(),
        Some(Scalar::Boolean(true))
    );
    assert_eq!(
        default("Float32 +2.5e-1").unwrap(),
        Some(Scalar::Float(0.25))
    );
    assert!(compile(&object(vec![])).unwrap().is_empty());
}

#[test]
fn unknown_malformed_nonfinite_and_reference_defaults_fail_closed() {
    for text in [
        " ",
        "Float32",
        "Float32 ",
        "Float32 1 2",
        "Float32 1.0x",
        "Float32 NaN",
        "Float32 inf",
        "Float32 1e99",
        "Float32  1",
        "Float32 1\n",
        "Float32 --1",
        "Float64 1",
        "Boolean false",
        "Boolean 0",
        "Int32 4",
    ] {
        assert!(
            compile(&object(vec![row(1, Access::Target, text)])).is_err(),
            "{text:?}"
        );
    }
    assert!(default(&"x".repeat(MAX_DEFAULT_BYTES + 1)).is_err());
    for (name, value) in [
        ("Value", Value::Float32(0.0)),
        ("ValueRef", local(1)),
        (
            "AccessType",
            Value::Enum {
                value: 0,
                name: Some("FieldAccessType_Target".into()),
            },
        ),
        (
            "AccessType",
            Value::Enum {
                value: 0,
                name: None,
            },
        ),
        ("AccessType", Value::UInt32(0)),
        ("Id", Value::Float32(1.0)),
    ] {
        let Value::Struct(mut fields) = row(1, Access::Target, "") else {
            panic!()
        };
        fields.0.iter_mut().find(|(key, _)| key == name).unwrap().1 = value;
        assert!(compile(&object(vec![Value::Struct(fields)])).is_err());
    }
    for name in ["Id", "Value", "ValueRef", "AccessType"] {
        let Value::Struct(mut fields) = row(1, Access::Target, "") else {
            panic!()
        };
        fields.0.retain(|(key, _)| key != name);
        assert!(compile(&object(vec![Value::Struct(fields)])).is_err());
    }
    assert!(
        compile(&object(vec![
            row(1, Access::Source, ""),
            row(1, Access::Target, "")
        ]))
        .is_err()
    );
    assert!(
        compile(&object(vec![
            row(1, Access::Source, "");
            MAX_PROPERTIES + 1
        ]))
        .is_err()
    );
    let mut wrong = object(vec![]);
    wrong.class = "Other".into();
    assert!(compile(&wrong).is_err());
}

fn prefabs() -> Memory {
    let mut memory = Memory::default();
    memory.put(
        "a",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2)])),
            ("Interface", Value::Pointer(Pointer::Null)),
        ],
    );
    for i in [1, 2] {
        memory.put(
            "a",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
    }
    memory.put("b", 0, "Blueprint", &[("Interface", local(1))]);
    memory.put(
        "b",
        1,
        "InterfaceDescriptorData",
        &[(
            "Fields",
            Value::Array(vec![
                row(7, Access::Target, "Float32 3.5"),
                row(8, Access::Source, ""),
            ]),
        )],
    );
    memory
}
#[test]
fn interfaces_outside_object_arrays_retain_distinct_prefab_instances() {
    let interfaces = read(&mut prefabs(), "a").unwrap();
    assert_eq!(interfaces.len(), 2);
    for (i, interface) in interfaces.iter().enumerate() {
        assert_eq!(interface.identity.asset, "b");
        assert_eq!(interface.identity.index, 1);
        assert_eq!(interface.identity.instances, [("a".into(), i + 1)]);
        assert_eq!(interface.bus.index, 0);
        assert_eq!(interface.bus.instances, interface.identity.instances);
        assert_eq!(interface.fields.len(), 2);
    }
    assert_ne!(interfaces[0].identity, interfaces[1].identity);
    assert!(read(&mut prefabs(), "").is_err());
    let mut memory = prefabs();
    memory.put(
        "b",
        0,
        "Blueprint",
        &[("Interface", Value::Pointer(Pointer::Null))],
    );
    assert!(read(&mut memory, "a").is_err());
    let mut memory = prefabs();
    memory.put("b", 1, "Other", &[("Fields", Value::Array(vec![]))]);
    assert!(read(&mut memory, "a").is_err());
}

#[test]
fn repeated_prefab_fields_count_towards_composition_bound() {
    let mut memory = prefabs();
    memory.put(
        "b",
        1,
        "InterfaceDescriptorData",
        &[(
            "Fields",
            Value::Array(
                (0..=(MAX_PROPERTIES / 2))
                    .map(|i| row(i as i32, Access::Target, ""))
                    .collect(),
            ),
        )],
    );
    assert!(read(&mut memory, "a").is_err());
}
