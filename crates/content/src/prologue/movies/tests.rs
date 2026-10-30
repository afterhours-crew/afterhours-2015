// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;
fn event(target: usize, port: u32, kind: i32) -> Value {
    Value::Struct(Fields(vec![
        ("Source".into(), local(1)),
        ("Target".into(), local(target)),
        (
            "SourceEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::Int32(77))])),
        ),
        (
            "TargetEvent".into(),
            Value::Struct(Fields(vec![("Id".into(), Value::Int32(port as i32))])),
        ),
        (
            "TargetType".into(),
            Value::Enum {
                value: kind,
                name: Some(
                    if kind == 3 {
                        "EventConnectionTargetType_Server"
                    } else {
                        "EventConnectionTargetType_NetworkedClient"
                    }
                    .into(),
                ),
            },
        ),
    ]))
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    let provider = (-1_232_400_965i32) as u32;
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Interface", local(1)),
            ("Objects", Value::Array((2..=15).map(local).collect())),
            (
                "EventConnections",
                Value::Array(vec![
                    event(3, 283_106_254, 3),
                    event(4, 283_106_254, 3),
                    event(2, 1_175_083_884, 3),
                ]),
            ),
            ("PropertyConnections", Value::Array(vec![])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(2), 777_057_321, local(5), 0),
                    edge(local(2), 1_234_675_151, local(6), 0),
                    edge(local(3), (-1_813_733_428i32) as u32, local(14), 0),
                    edge(local(4), (-1_813_733_428i32) as u32, local(15), 0),
                    edge(local(7), (-1_813_733_428i32) as u32, local(14), 0),
                    edge(local(8), (-1_813_733_428i32) as u32, local(15), 0),
                    edge(local(3), (-621_692_900i32) as u32, local(9), 0),
                    edge(local(4), (-621_692_900i32) as u32, local(10), 0),
                    edge(local(2), provider, local(11), 0),
                    edge(local(3), provider, local(11), 0),
                    edge(local(4), provider, local(11), 0),
                    edge(local(12), 99, local(13), 0),
                ]),
            ),
        ],
    );
    m.put("a", 1, "InterfaceDescriptorData", &[]);
    m.put("a", 2, "FMVSelectorEntityData", &[]);
    for i in [3, 4] {
        m.put(
            "a",
            i,
            "StateSequenceSelectorEntityData",
            &[("OverrideSequenceQueue", Value::Bool(true))],
        );
    }
    m.put(
        "a",
        5,
        "FMVRefererEntityData",
        &[("VideoAsset", ptr("v", 0))],
    );
    m.put("a", 6, "FMVParticipantStateEntityData", &[]);
    for i in [7, 8] {
        m.put("a", i, "StateSequenceParticipantStateEntityData", &[]);
    }
    for (i, name) in [(9, "b"), (10, "c")] {
        m.put(
            "a",
            i,
            "StateSequenceRefererEntityData",
            &[("Sequence", ptr(name, 0))],
        );
        m.put(name, 0, "ParticipantStateSequenceBlueprint", &[]);
        m.0.get_mut(&(name.into(), 0)).unwrap().guid = Some([i as u8; 16]);
    }
    m.put(
        "a",
        11,
        "ParticipantLogicParticipantProviderEntityData",
        &[],
    );
    m.put("a", 12, "RaceObjectiveEntityData", &[]);
    m.put(
        "a",
        13,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(9_007_199_254_740_999))],
    );
    for i in [14, 15] {
        m.put("a", i, "StateSequenceProviderEntityData", &[]);
    }
    m.put("v", 0, "VideoRefererAsset", &[]);
    m
}
#[test]
fn movie_follows_exact_order_and_participant_links() {
    let mut m = fixture();
    let raw = read(
        &mut Graph::new(&mut m),
        Site::root("a"),
        &BTreeMap::new(),
        99,
    )
    .unwrap();
    assert_eq!(raw.objective, 9_007_199_254_740_999);
    assert_eq!(raw.video, ("v".into(), 0));
    assert_eq!(raw.state.key, ("a".into(), 6));
    assert_eq!(raw.sequences[0].0, "b");
    assert_eq!(raw.sequences[1].0, "c");
    assert_ne!(raw.sequences[0].1, raw.sequences[1].1);
}
#[test]
fn movie_rejects_client_only_selection_wrong_assets_and_missing_or_shared_owner() {
    for variant in 0..5 {
        let mut m = fixture();
        match variant {
            0 => {
                m.put(
                    "a",
                    3,
                    "StateSequenceSelectorEntityData",
                    &[("OverrideSequenceQueue", Value::Bool(false))],
                );
            }
            1 => {
                m.put("v", 0, "OtherAsset", &[]);
            }
            2 => {
                m.put("a", 11, "ParticipantProviderEntityData", &[]);
            }
            3 => {
                let root = m.0.get_mut(&("a".into(), 0)).unwrap();
                root.fields
                    .0
                    .iter_mut()
                    .find(|(n, _)| n == "EventConnections")
                    .unwrap()
                    .1 = Value::Array(vec![
                    event(3, 283_106_254, 4),
                    event(4, 283_106_254, 3),
                    event(2, 1_175_083_884, 3),
                ]);
            }
            _ => {
                m.put(
                    "a",
                    13,
                    "StableUniqueIdEntityData",
                    &[("StableUniqueId64", Value::UInt64(0))],
                );
            }
        }
        assert!(
            read(
                &mut Graph::new(&mut m),
                Site::root("a"),
                &BTreeMap::new(),
                99
            )
            .is_err(),
            "variant {variant}"
        );
    }
}
