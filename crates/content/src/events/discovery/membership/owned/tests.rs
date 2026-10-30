// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn root(m: &mut Memory, asset: &str, edges: Vec<Value>) {
    m.put(
        asset,
        0,
        "Blueprint",
        &[
            ("Interface", local(1)),
            ("LinkConnections", Value::Array(edges)),
            ("PropertyConnections", Value::Array(vec![])),
            ("EventConnections", Value::Array(vec![])),
        ],
    );
    m.put(asset, 1, "InterfaceDescriptorData", &[]);
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    root(
        &mut m,
        "a",
        vec![
            edge(local(3), 10, local(2), 0),
            edge(local(4), 10, local(2), 0),
        ],
    );
    m.put("a", 2, "ParticipantSetEntityData", &[]);
    m.put(
        "a",
        3,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    m.put(
        "a",
        4,
        "SubWorldReferenceObjectData",
        &[("BundleName", Value::String("c".into()))],
    );
    for asset in ["b", "c"] {
        root(&mut m, asset, vec![edge(local(2), name("1"), local(1), 10)]);
        m.put(
            asset,
            2,
            "ParticipantSetPredicateEntityData",
            &[("NumberOfOperands", Value::UInt32(2))],
        );
    }
    m
}
fn selected() -> Site {
    Site {
        key: ("a".into(), 2),
        ..Site::root("a")
    }
}
fn read(m: &mut Memory) -> Result<bool, Error> {
    is_read_only(&mut Graph::new(m), &selected())
}
fn rows(m: &mut Memory, asset: &str, field: &str, value: Vec<Value>) {
    m.0.get_mut(&(asset.into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(n, _)| n == field)
        .unwrap()
        .1 = Value::Array(value);
}

#[test]
fn every_prefab_and_subworld_consumer_must_be_read_only() {
    let mut m = fixture();
    assert!(read(&mut m).unwrap());
    m.put("a", 3, "ReferenceObjectData", &[("Blueprint", ptr("b", 0))]);
    assert!(
        read(&mut m).unwrap(),
        "base blueprint references forward their exact interface too"
    );
    // The second branch must be inspected even after the first was proven safe.
    m.put("c", 2, "ParticipantSetAccessorEntityData", &[]);
    assert!(!read(&mut m).unwrap());
    m.put("c", 2, "LinkedSchematicChannelEntityData", &[]);
    assert!(!read(&mut m).unwrap());
    m.put(
        "c",
        2,
        "ParticipantSetPredicateEntityData",
        &[("NumberOfOperands", Value::UInt32(2))],
    );
    rows(
        &mut m,
        "c",
        "LinkConnections",
        vec![edge(local(2), name("2"), local(1), 10)],
    );
    assert!(
        !read(&mut m).unwrap(),
        "a predicate's unrelated port cannot certify a reader"
    );
    rows(
        &mut m,
        "c",
        "LinkConnections",
        vec![edge(local(2), name("0"), local(1), 10)],
    );
    assert!(read(&mut m).unwrap());
}

#[test]
fn event_writes_scalar_inputs_and_exposed_boundaries_remain_owned() {
    let mut m = fixture();
    rows(
        &mut m,
        "a",
        "EventConnections",
        vec![Value::Struct(Fields(vec![("Target".into(), local(2))]))],
    );
    assert!(!read(&mut m).unwrap());
    let mut m = fixture();
    rows(
        &mut m,
        "a",
        "PropertyConnections",
        vec![edge(local(3), 15, local(2), 9)],
    );
    assert!(!read(&mut m).unwrap());
    let mut m = fixture();
    rows(
        &mut m,
        "a",
        "LinkConnections",
        vec![edge(local(1), 10, local(2), 0)],
    );
    assert!(
        !read(&mut m).unwrap(),
        "unbound root export may have an external writer"
    );
    // Revisited exposed providers are still unresolved, not graph cycles.
    let mut active = BTreeSet::new();
    let target = Link {
        site: selected(),
        port: 0,
    };
    assert!(!consumers(&mut Graph::new(&mut m), target.clone(), &mut active, &mut 0).unwrap());
    assert!(active.is_empty());
    assert!(!consumers(&mut Graph::new(&mut m), target, &mut active, &mut 0).unwrap());
}

#[test]
fn interface_ports_are_not_confused_and_cycles_and_fanout_are_bounded() {
    let mut m = fixture();
    m.put("b", 3, "ParticipantSetAccessorEntityData", &[]);
    rows(
        &mut m,
        "b",
        "LinkConnections",
        vec![
            edge(local(2), name("1"), local(1), 10),
            edge(local(3), 772_610_520, local(1), 11),
        ],
    );
    assert!(
        read(&mut m).unwrap(),
        "the other provider port is independently mutable"
    );
    rows(
        &mut m,
        "b",
        "LinkConnections",
        vec![edge(local(1), 10, local(1), 10)],
    );
    rows(
        &mut m,
        "a",
        "LinkConnections",
        vec![
            edge(local(3), 10, local(2), 0),
            edge(local(3), 10, local(3), 10),
        ],
    );
    assert!(read(&mut m).is_err());
    let mut m = fixture();
    rows(
        &mut m,
        "b",
        "LinkConnections",
        vec![edge(local(2), name("1"), local(1), 10); MAX_CONSUMERS],
    );
    assert!(read(&mut m).unwrap_err().to_string().contains("bound"));
}
