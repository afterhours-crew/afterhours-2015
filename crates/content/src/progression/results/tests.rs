// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;
const STABLE: u32 = 77;
const ACTIVE: u32 = 1_010_522_579;

fn event(source: usize, output: u32, target: usize, input: u32, delivery: &str) -> Value {
    let value = |id| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(id))]));
    Value::Struct(Fields(vec![
        ("Source".into(), local(source)),
        ("SourceEvent".into(), value(output)),
        ("Target".into(), local(target)),
        ("TargetEvent".into(), value(input)),
        (
            "TargetType".into(),
            Value::Enum {
                value: 0,
                name: Some(delivery.into()),
            },
        ),
    ]))
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=8).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), STABLE, local(2), 0),
                    edge(local(3), DETAILS, local(4), 0),
                    edge(local(3), MANAGER, local(6), 0),
                    edge(local(3), PARTICIPANT, local(7), 0),
                ]),
            ),
            (
                "PropertyConnections",
                Value::Array(vec![edge(local(1), ACTIVE, local(3), ENABLED)]),
            ),
            (
                "EventConnections",
                Value::Array(vec![event(
                    3,
                    REPORTED,
                    1,
                    COMPLETE,
                    "EventConnectionTargetType_Server",
                )]),
            ),
        ],
    );
    m.put("a", 1, "RaceObjectiveEntityData", &[]);
    m.put(
        "a",
        2,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(101))],
    );
    m.put(
        "a",
        3,
        "ParticipantResultReporterEntityData",
        &[
            ("Id", Value::Pointer(Pointer::Null)),
            ("Categories", Value::Array(vec![])),
            ("Enabled", Value::Bool(true)),
            ("MatchAnyCategory", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        4,
        "DetailsRefererEntityData",
        &[
            ("Id", Value::Pointer(Pointer::Null)),
            ("Categories", Value::Array(vec![local(5)])),
        ],
    );
    m.put(
        "a",
        5,
        "GameModeCategory",
        &[("CategoryHash", Value::UInt32(91))],
    );
    m.put("a", 6, "ResultManagerEntityData", &[]);
    m.put("a", 7, "ParticipantLogicParticipantProviderEntityData", &[]);
    m.put("a", 8, "ScoreResultFilterEntityData", &[]);
    m
}
fn field(m: &mut Memory, node: usize, name: &str, value: Value) {
    let object = m.0.get_mut(&("a".into(), node)).unwrap();
    object
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == name)
        .unwrap()
        .1 = value;
}
fn program(m: &mut Memory, selected: BTreeSet<u64>) -> Result<Vec<Completion>, Error> {
    let mut graph = Graph::new(m);
    let nodes = graph.read(Site::root("a"))?;
    read(
        &mut graph,
        &nodes,
        &nodes,
        &BTreeMap::new(),
        STABLE,
        &selected,
        &BTreeMap::new(),
    )
}
#[test]
fn direct_completion_preserves_activation_categories_and_separate_result_filters() {
    let mut m = fixture();
    field(
        &mut m,
        0,
        "EventConnections",
        Value::Array(vec![
            event(3, REPORTED, 1, COMPLETE, "EventConnectionTargetType_Server"),
            event(3, REPORTED, 8, 7, "EventConnectionTargetType_Server"),
        ]),
    );
    let rules = program(&mut m, BTreeSet::from([101])).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].objective, 101);
    assert_eq!(rules[0].categories, BTreeSet::from([91]));
    assert_eq!(
        rules[0].enabled.facts().collect::<Vec<_>>(),
        [Fact::Active(101)]
    );
    assert!(!rules[0].enabled.evaluate(|_| Some(false)).unwrap());
    assert!(rules[0].enabled.evaluate(|_| Some(true)).unwrap());
    assert!(rules[0].enabled.evaluate(|_| None).is_err());
}
#[test]
fn incomplete_ambiguous_and_stateful_result_contracts_are_rejected() {
    let selected = BTreeSet::from([101]);
    for (node, name, value) in [
        (3, "MatchAnyCategory", Value::Bool(true)),
        (3, "Enabled", Value::Bool(false)),
        (3, "Categories", Value::Array(vec![local(5)])),
        (4, "Id", local(5)),
        (4, "Categories", Value::Array(vec![])),
        (0, "PropertyConnections", Value::Array(vec![])),
    ] {
        let mut m = fixture();
        field(&mut m, node, name, value);
        assert!(program(&mut m, selected.clone()).is_err(), "{node}:{name}");
    }
    for events in [
        vec![event(
            3,
            REPORTED,
            1,
            COMPLETE,
            "EventConnectionTargetType_Client",
        )],
        vec![event(
            8,
            REPORTED,
            1,
            COMPLETE,
            "EventConnectionTargetType_Server",
        )],
        vec![
            event(3, REPORTED, 1, COMPLETE, "EventConnectionTargetType_Server"),
            event(3, REPORTED, 1, COMPLETE, "EventConnectionTargetType_Server"),
        ],
        vec![
            event(3, REPORTED, 1, COMPLETE, "EventConnectionTargetType_Server"),
            event(8, 7, 3, 8, "EventConnectionTargetType_Server"),
        ],
    ] {
        let mut m = fixture();
        field(&mut m, 0, "EventConnections", Value::Array(events));
        assert!(program(&mut m, selected.clone()).is_err());
    }
    let mut foreign = fixture();
    foreign.0.get_mut(&("a".into(), 7)).unwrap().class =
        "SomeOtherParticipantProviderEntityData".into();
    assert!(program(&mut foreign, selected).is_err());
    assert!(program(&mut fixture(), BTreeSet::from([101, 102])).is_err());
}
