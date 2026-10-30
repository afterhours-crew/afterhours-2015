// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;
fn object(values: Vec<(&str, Value)>) -> Value {
    Value::Struct(Fields(
        values.into_iter().map(|(k, v)| (k.into(), v)).collect(),
    ))
}
fn event(mode: i32, source: usize, target: usize) -> Value {
    object(vec![
        ("Source", local(source)),
        ("Target", local(target)),
        ("SourceEvent", object(vec![("Id", Value::Int32(-7))])),
        ("TargetEvent", object(vec![("Id", Value::UInt32(123))])),
        (
            "TargetType",
            Value::Enum {
                value: mode,
                name: Some(format!(
                    "EventConnectionTargetType_{}",
                    match mode {
                        2 => "Client",
                        3 => "Server",
                        4 => "NetworkedClient",
                        5 => "NetworkedClientAndServer",
                        _ => "Unknown",
                    }
                )),
            },
        ),
    ])
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1), local(2)])),
            (
                "EventConnections",
                Value::Array(vec![
                    event(3, 1, 2),
                    event(2, 1, 2),
                    event(4, 1, 2),
                    event(5, 1, 2),
                ]),
            ),
        ],
    );
    m.put("a", 1, "CompareBoolEntityData", &[]);
    m.put(
        "a",
        2,
        "DirectPresentationContextEntityData",
        &[("Key", Value::String("ConstructedPresentation".into()))],
    );
    m
}
fn run(m: &mut Memory) -> Result<Vec<Event>, Error> {
    let mut g = Graph::new(m);
    let n = g.read(Site::root("a"))?;
    extract(&mut g, &n)
}
fn set(m: &mut Memory, asset: &str, index: usize, key: &str, value: Value) {
    let f = &mut m.0.get_mut(&(asset.into(), index)).unwrap().fields.0;
    f.retain(|(k, _)| k != key);
    f.push((key.into(), value));
}
#[test]
fn raw_ordinals_preserve_local_rows_delivery_and_named_targets() {
    let events = run(&mut fixture()).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events
            .iter()
            .map(|e| (e.bus, e.ordinal, e.delivery))
            .collect::<Vec<_>>(),
        vec![(1, 2, Delivery::Client), (1, 3, Delivery::ClientAndServer)]
    );
    assert_eq!(events[0].source.port, (-7i32) as u32);
    assert_eq!(
        events[0].target.presentation_key.as_deref(),
        Some("ConstructedPresentation")
    );
    assert_eq!(events[0].target.port, 123);
}
#[test]
fn repeated_prefabs_have_separate_bus_occurrences() {
    let mut m = fixture();
    m.put(
        "a",
        3,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("p", 0))],
    );
    m.put(
        "a",
        4,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("p", 0))],
    );
    set(
        &mut m,
        "a",
        0,
        "Objects",
        Value::Array(vec![local(1), local(2), local(3), local(4)]),
    );
    m.put(
        "p",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Flags", Value::UInt32(1)),
            ("Objects", Value::Array(vec![local(1), local(2)])),
            ("EventConnections", Value::Array(vec![event(4, 1, 2)])),
        ],
    );
    m.put("p", 1, "ConstructedSource", &[]);
    m.put("p", 2, "ConstructedTarget", &[]);
    let events = run(&mut m).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.bus, e.ordinal))
            .collect::<Vec<_>>(),
        vec![(1, 2), (1, 3), (2, 0), (3, 0)]
    );
    assert_eq!(events[2].source.asset, "p");
    assert_eq!(events[3].source.asset, "p");
    set(&mut m, "p", 0, "Flags", Value::UInt16(0));
    assert!(run(&mut m).is_err());
    set(&mut m, "p", 0, "EventConnections", Value::Array(vec![]));
    assert_eq!(run(&mut m).unwrap().len(), 2);
}
#[test]
fn invalid_enum_missing_pointer_and_foreign_endpoint_fail() {
    for mode in [0, 6, 123] {
        let mut m = fixture();
        set(
            &mut m,
            "a",
            0,
            "EventConnections",
            Value::Array(vec![event(mode, 1, 2)]),
        );
        assert!(run(&mut m).is_err());
    }
    for target in [3, 4] {
        let mut m = fixture();
        m.put("a", 3, "ConstructedForeign", &[]);
        set(
            &mut m,
            "a",
            0,
            "EventConnections",
            Value::Array(vec![event(4, 1, target)]),
        );
        assert!(run(&mut m).is_err());
    }
    let mut m = fixture();
    let mut row = event(4, 1, 2);
    let Value::Struct(ref mut fields) = row else {
        unreachable!()
    };
    fields.0.retain(|(k, _)| k != "Target");
    set(&mut m, "a", 0, "EventConnections", Value::Array(vec![row]));
    assert!(run(&mut m).is_err());
}
#[test]
fn own_interface_is_allowed_but_other_interfaces_and_duplicate_buses_fail() {
    let mut m = fixture();
    m.put("a", 3, "InterfaceDescriptorData", &[]);
    set(&mut m, "a", 0, "Interface", local(3));
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![event(4, 1, 3)]),
    );
    assert_eq!(run(&mut m).unwrap()[0].target.index, 3);
    set(&mut m, "a", 0, "Interface", local(2));
    assert!(run(&mut m).is_err());
    let mut m = fixture();
    let mut g = Graph::new(&mut m);
    let mut n = g.read(Site::root("a")).unwrap();
    n.push(n[0].clone());
    assert!(extract(&mut g, &n).is_err());
    let mut m = fixture();
    let mut g = Graph::new(&mut m);
    let mut n = g.read(Site::root("a")).unwrap();
    n.push(n[1].clone());
    assert!(extract(&mut g, &n).is_err());
}
#[test]
fn table_bounds_and_unregistered_roots_are_rejected() {
    let mut m = fixture();
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![event(3, 1, 2); MAX_EVENTS + 1]),
    );
    assert!(run(&mut m).is_err());
    let mut m = fixture();
    set(&mut m, "a", 0, "Flags", Value::UInt16(0));
    assert!(run(&mut m).is_err());
    set(&mut m, "a", 0, "EventConnections", Value::Array(vec![]));
    assert!(run(&mut m).is_err());
}
#[test]
fn debug_hides_select_only_property_debug_hide_targets_in_order() {
    let mut m = fixture();
    m.put("a", 3, "PropertyDebugEntityData", &[]);
    set(
        &mut m,
        "a",
        0,
        "Objects",
        Value::Array(vec![local(1), local(2), local(3)]),
    );
    let hide = |source| {
        let mut row = event(4, source, 3);
        if let Value::Struct(Fields(f)) = &mut row {
            f.retain(|(k, _)| k != "TargetEvent");
            f.push((
                "TargetEvent".into(),
                object(vec![("Id", Value::UInt32(DEBUG_HIDE))]),
            ));
        }
        row
    };
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![hide(1), event(4, 1, 3), event(4, 1, 2), hide(2)]),
    );
    let events = run(&mut m).unwrap();
    let driven = |m: &mut Memory| {
        let mut g = Graph::new(m);
        let n = g.read(Site::root("a")).unwrap();
        property_targets(&mut g, &n).unwrap()
    };
    // Only controllers with a driving property connection fire.
    assert!(debug_hides(&events, &driven(&mut m)).is_err());
    set(
        &mut m,
        "a",
        0,
        "PropertyConnections",
        Value::Array(vec![edge(local(2), 5, local(1), 6)]),
    );
    assert_eq!(debug_hides(&events, &driven(&mut m)).unwrap(), vec![(1, 0)]);
    set(
        &mut m,
        "a",
        0,
        "PropertyConnections",
        Value::Array(vec![
            edge(local(2), 5, local(1), 6),
            edge(local(1), 5, local(2), 6),
        ]),
    );
    assert_eq!(
        debug_hides(&events, &driven(&mut m)).unwrap(),
        vec![(1, 0), (1, 3)]
    );
    // A root without any hide is a content mismatch, not an empty plan.
    assert!(debug_hides(&run(&mut fixture()).unwrap(), &driven(&mut m)).is_err());
}
#[test]
fn intro_return_rows_are_the_single_client_filter_row() {
    let mut m = fixture();
    m.put("a", 3, "ParticipantProviderEventFilterEntityData", &[]);
    set(
        &mut m,
        "a",
        0,
        "Objects",
        Value::Array(vec![local(1), local(2), local(3)]),
    );
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![event(4, 1, 2), event(4, 3, 2), event(5, 3, 2)]),
    );
    let events = run(&mut m).unwrap();
    // Only the client-delivered filter row; the client-and-server one is not.
    assert_eq!(filtered_rows(&events).unwrap(), vec![(1, 1)]);
    assert!(filtered_rows(&run(&mut fixture()).unwrap()).is_err());
    set(
        &mut m,
        "a",
        0,
        "EventConnections",
        Value::Array(vec![event(4, 3, 2), event(4, 3, 1)]),
    );
    assert!(filtered_rows(&run(&mut m).unwrap()).is_err());
}
