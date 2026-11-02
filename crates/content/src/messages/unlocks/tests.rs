// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn en(value: i32, name: &str) -> Value {
    Value::Enum {
        value,
        name: Some(name.into()),
    }
}
fn record(fields: &[(&str, Value)]) -> Value {
    Value::Struct(Fields(
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    ))
}
fn event_row(from: usize, output: u32, to: usize, input: u32) -> Value {
    record(&[
        ("Source", local(from)),
        ("Target", local(to)),
        ("SourceEvent", record(&[("Id", Value::UInt32(output))])),
        ("TargetEvent", record(&[("Id", Value::UInt32(input))])),
        ("TargetType", en(0, "EventConnectionTargetType_Server")),
    ])
}
fn put(m: &mut Memory, i: usize, class: &str, fields: &[(&str, Value)]) {
    let mut fields = fields.to_vec();
    fields.push(("Realm", en(0, "Realm_Server")));
    fields.push(("Flags", Value::UInt32((2 << 25) | (2 << 27))));
    m.put("a", i, class, &fields);
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    let mut links = vec![edge(local(10), MANAGER_TARGET, local(11), 0)];
    for sender in [1, 2] {
        links.extend([
            edge(local(sender), CONTEXT, local(8), 0),
            edge(local(sender), MANAGER_LINK, local(9), MANAGER_TARGET),
        ]);
    }
    for owner in [5, 6, 7] {
        links.extend([
            edge(local(owner), CONTEXT, local(8), 0),
            edge(local(owner), INVENTORY, local(9), INVENTORY_TARGET),
        ]);
    }
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array((1..=11).map(local).collect())),
            ("LinkConnections", Value::Array(links)),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(7), UNLOCKED_ID, local(2), PART),
                    edge(local(7), UNLOCKED_ID, local(6), FILTER_ID),
                ]),
            ),
            (
                "EventConnections",
                Value::Array(vec![
                    event_row(5, CATEGORY_SUCCESS, 3, IN),
                    event_row(3, OUT, 1, SEND),
                    event_row(7, UNLOCKED, 6, FILTER_IN),
                    event_row(6, FILTER_OUT, 4, IN),
                    event_row(4, OUT, 2, SEND),
                ]),
            ),
        ],
    );
    put(
        &mut m,
        1,
        "DynamicMessageSenderEntityData",
        &[
            ("MessageAsset", ptr("m", 0)),
            ("AttachmentInputParameters", Value::Array(vec![])),
        ],
    );
    put(
        &mut m,
        2,
        "DynamicMessageSenderEntityData",
        &[
            ("MessageAsset", ptr("n", 0)),
            (
                "AttachmentInputParameters",
                Value::Array(vec![record(&[
                    ("LinkIds", Value::Array(vec![])),
                    ("PropertyIds", Value::Array(vec![Value::UInt32(PART)])),
                ])]),
            ),
        ],
    );
    for (i, seconds) in [(3, 1.25), (4, 2.5)] {
        put(
            &mut m,
            i,
            "DelayEntityData",
            &[
                ("Delay", Value::Float32(seconds)),
                ("AutoStart", Value::Bool(false)),
                ("RunOnce", Value::Bool(false)),
                ("RemoveDuplicateEvents", Value::Bool(true)),
                ("TimeDeltaType", en(0, "TimeDeltaType_World")),
            ],
        );
    }
    put(
        &mut m,
        5,
        "UnlockItemCategoryEntityData",
        &[(
            "Categories",
            Value::Array(vec![
                en(2, "CategoryUnlockMask_TestA"),
                en(8, "CategoryUnlockMask_TestB"),
            ]),
        )],
    );
    put(
        &mut m,
        6,
        "CarPartFilterEntityData",
        &[
            ("IncludeFilter", Value::Bool(false)),
            ("FilterItems", Value::Array(vec![ptr("d", 0), ptr("d", 1)])),
        ],
    );
    put(&mut m, 7, "CarPartListenerEntityData", &[]);
    put(
        &mut m,
        8,
        "ParticipantLogicParticipantProviderEntityData",
        &[],
    );
    put(
        &mut m,
        9,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    put(
        &mut m,
        10,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    put(&mut m, 11, "MessageManagerEntityData", &[]);
    m.put("c", 0, "ChannelAsset", &[]);
    for (asset, hash, attachments) in [("m", 71, vec![]), ("n", 72, vec![local(1)])] {
        m.put(
            asset,
            0,
            "MessageAsset",
            &[
                ("HashId", Value::UInt32(hash)),
                ("Persisted", Value::Bool(true)),
                ("Attachments", Value::Array(attachments)),
            ],
        );
    }
    m.put("n", 1, "CarPartAttachmentAsset", &[]);
    for (i, id) in [(0, 0x10203040), (1, 0xff002381)] {
        m.put(
            "d",
            i,
            "PerformancePartItemData",
            &[("Id", record(&[("Id", Value::UInt32(id))]))],
        );
    }
    m
}
fn compile(m: &mut Memory, selected: &[&str]) -> Result<Vec<Notification>, Error> {
    let mut g = Graph::new(m);
    let nodes = g.read(Site::root("a"))?;
    read(
        &mut g,
        &nodes,
        &nodes,
        &BTreeMap::from([(("a".into(), 11), 23)]),
        &selected.iter().map(|s| (*s).into()).collect(),
    )
}
fn set(m: &mut Memory, asset: &str, i: usize, field: &str, value: Value) {
    let fields = &mut m.0.get_mut(&(asset.into(), i)).unwrap().fields.0;
    fields.iter_mut().find(|(k, _)| k == field).unwrap().1 = value;
}
fn rows<'a>(m: &'a mut Memory, field: &str) -> &'a mut Vec<Value> {
    let Value::Array(a) = &mut m
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == field)
        .unwrap()
        .1
    else {
        panic!()
    };
    a
}

#[test]
fn category_success_and_filtered_part_share_owned_manager_but_keep_their_delays() {
    let out = compile(&mut fixture(), &["m", "n"]).unwrap();
    assert_eq!(
        out,
        [
            Notification {
                hash: 71,
                manager: 23,
                delay: Duration::from_millis(1250),
                trigger: Trigger::Categories { mask: 10 }
            },
            Notification {
                hash: 72,
                manager: 23,
                delay: Duration::from_millis(2500),
                trigger: Trigger::Part {
                    include: false,
                    ids: BTreeSet::from([0x10203040, 0xff002381])
                }
            },
        ]
    );
    let mut m = fixture();
    set(&mut m, "a", 6, "IncludeFilter", Value::Bool(true));
    assert!(matches!(
        compile(&mut m, &["n"]).unwrap()[0].trigger,
        Trigger::Part { include: true, .. }
    ));
    assert!(compile(&mut fixture(), &["absent"]).is_err());
}

#[test]
fn attachment_inbox_belongs_to_the_wired_manager_not_every_capacity_field() {
    let mut m = Memory::default();
    m.put(
        "p",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=5).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), MANAGER_LINK, local(3), 0),
                    edge(local(2), MANAGER_LINK, local(4), 0),
                    edge(local(2), 123_756_709, local(5), 0),
                ]),
            ),
        ],
    );
    for i in [1, 2] {
        m.put(
            "p",
            i,
            "MessagePersistenceEntityData",
            &[("DynamicMessageLimit", Value::UInt32(7))],
        );
    }
    for i in [3, 4] {
        m.put("p", i, MANAGER, &[]);
    }
    m.put(
        "p",
        5,
        "PersistentTableWithDynamicSecondaryKeyEntityData",
        &[("TableName", Value::String("DynamicMessage".into()))],
    );
    let compile = |m: &mut Memory| {
        let mut g = Graph::new(m);
        let nodes = g.read(Site::root("p"))?;
        dynamic_inbox(
            &mut g,
            &nodes,
            &[(("p".into(), 3), 21), (("p".into(), 4), 22)].into(),
        )
    };
    assert_eq!(compile(&mut m).unwrap(), (22, 7));
    for capacity in [0, 1, 513, u32::MAX] {
        let mut bad = Memory(m.0.clone());
        set(
            &mut bad,
            "p",
            2,
            "DynamicMessageLimit",
            Value::UInt32(capacity),
        );
        assert!(compile(&mut bad).is_err());
    }
    let mut bad = Memory(m.0.clone());
    set(
        &mut bad,
        "p",
        5,
        "TableName",
        Value::String("StaticMessage".into()),
    );
    assert!(compile(&mut bad).is_err());
    for index in [1, 2] {
        let mut bad = Memory(m.0.clone());
        let fields = &mut bad.0.get_mut(&("p".into(), 0)).unwrap().fields;
        let Value::Array(edges) = &mut fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "LinkConnections")
            .unwrap()
            .1
        else {
            panic!()
        };
        edges.remove(index);
        assert!(compile(&mut bad).is_err());
    }
    let fields = &mut m.0.get_mut(&("p".into(), 0)).unwrap().fields;
    let Value::Array(edges) = &mut fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "LinkConnections")
        .unwrap()
        .1
    else {
        panic!()
    };
    edges.push(edge(local(1), 123_756_709, local(5), 0));
    assert!(
        compile(&mut m).is_err(),
        "two managers may not own one ring"
    );
}

#[test]
fn part_attachment_ids_are_definition_ids_and_aliases_are_rejected() {
    let mut m = Memory::default();
    m.put(
        "p",
        0,
        "ParticipantLogicBlueprint",
        &[("Objects", Value::Array(vec![local(1), local(2)]))],
    );
    for i in [1, 2] {
        m.put(
            "p",
            i,
            "UnlockItemTransactionEntityData",
            &[("UnlockedItem", ptr("i", i))],
        );
        m.put(
            "i",
            i,
            "AirFilterItemData",
            &[("Id", record(&[("Id", Value::UInt32(100 + i as u32))]))],
        );
        m.0.get_mut(&("i".into(), i)).unwrap().guid = Some([i as u8; 16]);
    }
    let compile = |m: &mut Memory| {
        let mut g = Graph::new(m);
        let nodes = g.read(Site::root("p"))?;
        part_ids(&mut g, &nodes)
    };
    assert_eq!(
        compile(&mut m).unwrap(),
        [
            Part {
                definition: [1; 16],
                id: 101
            },
            Part {
                definition: [2; 16],
                id: 102
            }
        ]
    );
    for id in [0, 101] {
        let mut bad = Memory(m.0.clone());
        set(&mut bad, "i", 2, "Id", record(&[("Id", Value::UInt32(id))]));
        assert!(compile(&mut bad).is_err());
    }
    for value in [Value::UInt32(102), record(&[("Wrong", Value::UInt32(102))])] {
        let mut bad = Memory(m.0.clone());
        set(&mut bad, "i", 2, "Id", value);
        assert!(
            compile(&mut bad).is_err(),
            "the installed ID is a nested field"
        );
    }
    for guid in [None, Some([0; 16]), Some([1; 16])] {
        let mut bad = Memory(m.0.clone());
        bad.0.get_mut(&("i".into(), 2)).unwrap().guid = guid;
        assert!(compile(&mut bad).is_err());
    }
}

#[test]
fn every_required_event_property_and_link_is_checked_including_realms() {
    for field in ["PropertyConnections", "LinkConnections", "EventConnections"] {
        let length = rows(&mut fixture(), field).len();
        for index in 0..length {
            let mut m = fixture();
            rows(&mut m, field).remove(index);
            assert!(
                compile(&mut m, &["m", "n"]).is_err(),
                "missing {field}[{index}]"
            );
            let mut m = fixture();
            let duplicate = rows(&mut m, field)[index].clone();
            rows(&mut m, field).push(duplicate);
            assert!(
                compile(&mut m, &["m", "n"]).is_err(),
                "duplicate {field}[{index}]"
            );
        }
    }
    for index in 0..5 {
        let mut m = fixture();
        let Value::Struct(f) = &mut rows(&mut m, "EventConnections")[index] else {
            panic!()
        };
        f.0.iter_mut().find(|(k, _)| k == "TargetType").unwrap().1 =
            en(0, "EventConnectionTargetType_Client");
        assert!(compile(&mut m, &["m", "n"]).is_err());
    }
}

#[test]
fn alternate_inputs_clocks_or_attachment_shapes_are_not_silently_reduced() {
    for (asset, i, field, value) in [
        ("a", 3, "Delay", Value::Float32(f32::NAN)),
        ("a", 3, "Delay", Value::Float32(0.)),
        ("a", 4, "Delay", Value::Float32(61.)),
        ("a", 3, "RunOnce", Value::Bool(true)),
        ("a", 4, "AutoStart", Value::Bool(true)),
        ("a", 4, "RemoveDuplicateEvents", Value::Bool(false)),
        ("a", 3, "TimeDeltaType", en(0, "TimeDeltaType_Real")),
        ("a", 1, "Flags", Value::UInt32(1 << 25)),
        ("a", 6, "Flags", Value::UInt32(2 << 25)),
        ("a", 8, "Realm", en(0, "Realm_Client")),
        ("m", 0, "Persisted", Value::Bool(false)),
        ("n", 0, "HashId", Value::UInt32(71)),
        ("n", 0, "Attachments", Value::Array(vec![])),
        (
            "n",
            0,
            "Attachments",
            Value::Array(vec![local(1), local(1)]),
        ),
        (
            "a",
            2,
            "AttachmentInputParameters",
            Value::Array(vec![record(&[
                ("LinkIds", Value::Array(vec![Value::UInt32(42)])),
                ("PropertyIds", Value::Array(vec![Value::UInt32(PART)])),
            ])]),
        ),
        (
            "a",
            5,
            "Categories",
            Value::Array(vec![en(3, "CategoryUnlockMask_Test")]),
        ),
        (
            "a",
            6,
            "FilterItems",
            Value::Array(vec![ptr("d", 0), ptr("d", 0)]),
        ),
    ] {
        let mut m = fixture();
        set(&mut m, asset, i, field, value);
        assert!(compile(&mut m, &["m", "n"]).is_err(), "{asset}:{i}:{field}");
    }
    for (target, port) in [(1, IN), (3, 42), (4, 43), (6, 44), (7, 45)] {
        let mut m = fixture();
        rows(&mut m, "EventConnections").push(event_row(5, CATEGORY_SUCCESS, target, port));
        assert!(compile(&mut m, &["m", "n"]).is_err());
    }
    let mut m = fixture();
    m.put("n", 1, "LiveryAttachmentAsset", &[]);
    assert!(compile(&mut m, &["n"]).is_err());
    // Explicitly unselected award types are not claimed as compiled support.
    assert!(compile(&mut m, &["m"]).is_ok());
}

#[test]
fn part_id_event_and_inventory_must_belong_to_the_same_participant() {
    for (field, from, source_port, to, target_port) in [
        ("PropertyConnections", 5, UNLOCKED_ID, 2, PART),
        ("PropertyConnections", 5, UNLOCKED_ID, 6, FILTER_ID),
        ("LinkConnections", 6, CONTEXT, 7, 0),
        ("LinkConnections", 7, INVENTORY, 10, INVENTORY_TARGET),
    ] {
        let mut m = fixture();
        let entries = rows(&mut m, field);
        let replacement = edge(local(from), source_port, local(to), target_port);
        let is_link = field == "LinkConnections";
        let i = entries
            .iter()
            .position(|v| {
                let f = v.as_fields().unwrap();
                if is_link {
                    f.get("Source").unwrap() == &local(from)
                        && f.get("SourceFieldId").unwrap().bits32() == Some(source_port)
                } else {
                    f.get("Target").unwrap() == &local(to)
                        && f.get("TargetFieldId").unwrap().bits32() == Some(target_port)
                }
            })
            .unwrap();
        entries[i] = replacement;
        assert!(
            compile(&mut m, &["m", "n"]).is_err(),
            "{field} {from}->{to}"
        );
    }
}

#[test]
fn sibling_scene_channel_requires_its_actual_startup_parent() {
    let mut m = fixture();
    // The channel provider lives in another subworld. Its interface leaves
    // through the level's reference and enters the manager's sibling scene.
    m.0.get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .push(("Interface".into(), local(12)));
    m.put("a", 12, "InterfaceDescriptorData", &[]);
    rows(&mut m, "LinkConnections").push(edge(local(12), 1234, local(11), 0));
    m.put(
        "b",
        0,
        "SubWorldData",
        &[
            ("Objects", Value::Array(vec![local(1)])),
            ("Interface", local(2)),
            ("PropertyConnections", Value::Array(vec![])),
            ("EventConnections", Value::Array(vec![])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(1), MANAGER_TARGET, local(2), 5678)]),
            ),
        ],
    );
    m.put(
        "b",
        1,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put("b", 2, "InterfaceDescriptorData", &[]);
    m.put(
        "l",
        0,
        "LevelData",
        &[
            ("Objects", Value::Array(vec![local(1), local(2)])),
            ("PropertyConnections", Value::Array(vec![])),
            ("EventConnections", Value::Array(vec![])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(1), 5678, local(2), 1234)]),
            ),
        ],
    );
    for (i, name) in [(1, "b"), (2, "a")] {
        m.put(
            "l",
            i,
            "SubWorldReferenceObjectData",
            &[("BundleName", Value::String(name.into()))],
        );
    }
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("a")).unwrap();
    let mut root = Site::root("b");
    root.parents.push(Frame {
        bus: ("l".into(), 0),
        reference: ("l".into(), 1),
    });
    let world = g.read(root).unwrap();
    let bindings = BTreeMap::from([(("a".into(), 11), 23)]);
    let selected = BTreeSet::from(["m".into(), "n".into()]);
    assert_eq!(
        read(&mut g, &nodes, &world, &bindings, &selected)
            .unwrap()
            .len(),
        2
    );
    let unbound = g.read(Site::root("b")).unwrap();
    assert!(read(&mut g, &nodes, &unbound, &bindings, &selected).is_err());
    // The same blueprint with an unrelated instance frame cannot certify it.
    let mut wrong = world;
    for node in &mut wrong {
        node.site.parents[0].reference.1 = 2;
    }
    assert!(read(&mut g, &nodes, &wrong, &bindings, &selected).is_err());
}
