// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, local, ptr};
use nfs_frostbite::ebx::Fields;

fn realm(value: i32, name: &str) -> Value {
    Value::Enum {
        value,
        name: Some(name.into()),
    }
}
fn object(class: &str, value: Option<Value>) -> Object {
    Object {
        class: class.into(),
        guid: None,
        fields: Fields(value.map(|v| vec![("Realm".into(), v)]).unwrap_or_default()),
    }
}

#[test]
fn creation_enum_is_not_the_listener_bit_encoding() {
    for (value, name, expected) in [
        (0, "Realm_Client", Realm::Client),
        (1, "Realm_Server", Realm::Server),
        (2, "Realm_ClientAndServer", Realm::Both),
        (3, "Realm_None", Realm::Neither),
    ] {
        for class in [
            "FloatEntityData",
            "DelayEntityData",
            "RaceVehiclePhysicsQueryEntityData",
        ] {
            let mut data = object(class, Some(realm(value, name)));
            // Deliberately conflicting listener bits do not change eligibility.
            data.fields.0.push(("Flags".into(), Value::UInt32(0)));
            assert_eq!(compile(&data).unwrap(), Some(expected));
        }
    }
}

#[test]
fn fixed_creation_realms_ignore_an_authored_realm_and_listener_flags() {
    for (class, expected) in [
        ("RaceVehicleDonutQueryEntityData", Realm::Server),
        ("RaceVehicleNosQueryEntityData", Realm::Server),
        ("RaceVehicleNearMissQueryEntityData", Realm::Both),
        ("SyncedFloatEntityData", Realm::Both),
        ("RaceVehicleAnyCollisionEvaluatorEntityData", Realm::Client),
    ] {
        for value in [None, Some(realm(3, "Realm_None")), Some(Value::Bool(false))] {
            assert_eq!(compile(&object(class, value)).unwrap(), Some(expected));
        }
    }
}

#[test]
fn unknown_missing_pipeline_and_mismatched_enums_fail_closed() {
    for value in [
        None,
        Some(Value::UInt32(1)),
        Some(realm(0, "Realm_Server")),
        Some(realm(4, "Realm_Pipeline")),
        Some(realm(-1, "Realm_None")),
        Some(Value::Enum {
            value: 1,
            name: None,
        }),
    ] {
        assert!(compile(&object("FloatEntityData", value)).is_err());
    }
    assert!(compile(&object("UnknownEntityData", Some(realm(1, "Realm_Server")))).is_err());
    for class in [
        "LogicPrefabBlueprint",
        "ParticipantLogicBlueprint",
        "InterfaceDescriptorData",
    ] {
        assert_eq!(compile(&object(class, None)).unwrap(), None);
    }
}

fn prefabs() -> Memory {
    let mut memory = Memory::default();
    memory.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[("Objects", Value::Array(vec![local(1), local(2)]))],
    );
    for i in [1, 2] {
        memory.put(
            "a",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
    }
    memory.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[("Objects", Value::Array(vec![local(1), local(2)]))],
    );
    memory.put(
        "b",
        1,
        "FloatEntityData",
        &[("Realm", realm(1, "Realm_Server"))],
    );
    memory.put(
        "b",
        2,
        "BoolCacheEntityData",
        &[("Realm", realm(0, "Realm_Client"))],
    );
    memory
}
#[test]
fn repeated_prefabs_keep_each_occurrence_without_claiming_activation_order() {
    let nodes = read(&mut prefabs(), "a").unwrap();
    assert_eq!(nodes.len(), 6);
    for (i, occurrence) in nodes.as_chunks::<3>().0.iter().enumerate() {
        assert_eq!(occurrence[0].identity.asset, "a");
        assert_eq!(occurrence[0].identity.index, i + 1);
        assert!(occurrence[0].identity.instances.is_empty());
        for (j, node) in occurrence[1..].iter().enumerate() {
            assert_eq!(node.identity.asset, "b");
            assert_eq!(node.identity.index, j + 1);
            assert_eq!(node.identity.instances, [("a".into(), i + 1)]);
        }
        assert_eq!(occurrence[1].realm, Realm::Server);
        assert_eq!(occurrence[2].realm, Realm::Client);
    }
    assert_eq!(
        nodes.iter().filter(|n| n.realm.includes_server()).count(),
        4
    );
    assert_eq!(
        nodes.iter().filter(|n| n.realm.includes_client()).count(),
        4
    );
    let mut broken = prefabs();
    broken.put("b", 1, "UnknownEntityData", &[]);
    assert!(read(&mut broken, "a").is_err());
}

#[test]
fn creation_reader_rejects_cycles_invalid_roots_and_oversized_compositions() {
    assert!(read(&mut prefabs(), "").is_err());
    assert!(read(&mut prefabs(), &"a".repeat(513)).is_err());
    let mut cycle = prefabs();
    cycle.put(
        "b",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("a", 0))],
    );
    assert!(read(&mut cycle, "a").is_err());
    let mut large = Memory::default();
    large.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[("Objects", Value::Array(vec![local(1); MAX_NODES]))],
    );
    large.put("a", 1, "SyncedFloatEntityData", &[]);
    assert!(read(&mut large, "a").is_err());
}
