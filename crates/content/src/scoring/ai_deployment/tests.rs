// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::{Fields, Value};

fn site(asset: &str, index: usize) -> Site {
    Site {
        key: (asset.into(), index),
        ..Site::root(asset)
    }
}

#[test]
fn boolean_membership_requires_the_same_bound_property_and_export() {
    let mut m = Memory::default();
    let port = 2_526_680_631;
    m.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2)])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(1), port, local(2), 11)]),
            ),
        ],
    );
    m.put("r", 1, "BoolParticipantPropertyAccessorEntityData", &[]);
    m.put(
        "r",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(
                [11, 12]
                    .into_iter()
                    .map(|id| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(id))])))
                    .collect(),
            ),
        )],
    );
    m.put("s", 1, "LinkedSchematicChannelEntityData", &[]);
    m.put("s", 2, "BoolParticipantPropertyEntityData", &[]);
    m.put("s", 3, "ParticipantSetBoolPropertyAdapterEntityData", &[]);
    m.put("s", 4, "BoolParticipantPropertyEntityData", &[]);
    let aliases = BTreeMap::from([(site("r", 2), site("s", 1))]);
    for target in [2, 4] {
        m.put(
            "s",
            0,
            "Blueprint",
            &[(
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 11, local(2), 0),
                    edge(local(1), 12, local(3), 0),
                    edge(local(3), port, local(target), 0),
                ]),
            )],
        );
        let mut graph = Graph::new(&mut m);
        let branch = graph.read(Site::root("r")).unwrap();
        let actual = boolean_sets(&mut graph, &branch, &aliases);
        if target == 2 {
            assert_eq!(
                actual.unwrap(),
                vec![BooleanSet {
                    property: Property {
                        channel: "c".into(),
                        port: 11
                    },
                    set: Property {
                        channel: "c".into(),
                        port: 12
                    },
                    definition: Identity::from(&site("s", 3)),
                }]
            );
        } else {
            assert!(actual.is_err());
        }
    }
    m.put(
        "s",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), 11, local(2), 0),
                edge(local(1), 12, local(3), 0),
                edge(local(1), 12, local(3), 0),
                edge(local(3), port, local(2), 0),
            ]),
        )],
    );
    let mut graph = Graph::new(&mut m);
    let branch = graph.read(Site::root("r")).unwrap();
    assert!(boolean_sets(&mut graph, &branch, &aliases).is_err());
}

#[test]
fn services_follow_bound_occurrence_and_reject_missing_ambiguous_or_wrong_kind() {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![edge(local(1), 10, local(2), 20)]),
        )],
    );
    m.put("r", 1, participant_pacing::CLASS, &[]);
    m.put("r", 2, "LinkedSchematicChannelReferenceEntityData", &[]);
    m.put("s", 1, "LinkedSchematicChannelEntityData", &[]);
    m.put("s", 2, "AiPacingManagerEntityData", &[]);
    m.put("s", 3, "AiPacingManagerEntityData", &[]);
    let aliases = BTreeMap::from([(site("r", 2), site("s", 1))]);
    for (links, expected) in [
        (vec![edge(local(1), 20, local(3), 0)], Some(3)),
        (vec![edge(local(1), 20, local(2), 0)], Some(2)),
        (vec![], None),
        (
            vec![
                edge(local(1), 20, local(2), 0),
                edge(local(1), 20, local(3), 0),
            ],
            None,
        ),
    ] {
        m.put(
            "s",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(links))],
        );
        let actual = one(
            &mut Graph::new(&mut m),
            &site("r", 1),
            10,
            &aliases,
            "AiPacingManagerEntityData",
        );
        match expected {
            Some(index) => assert_eq!(actual.unwrap(), site("s", index)),
            None => assert!(actual.is_err()),
        }
    }
    m.put(
        "s",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![edge(local(1), 20, local(2), 0)]),
        )],
    );
    m.put("s", 2, "AiActorManagerEntityData", &[]);
    assert!(
        one(
            &mut Graph::new(&mut m),
            &site("r", 1),
            10,
            &aliases,
            "AiPacingManagerEntityData"
        )
        .is_err()
    );
    assert!(
        one(
            &mut Graph::new(&mut m),
            &site("r", 1),
            10,
            &BTreeMap::new(),
            "AiPacingManagerEntityData"
        )
        .is_err()
    );
}

fn curve(top: f32) -> Value {
    Value::Array(
        [[0., 0., 1.], [1., top, 2.], [0., 0., 0.], [1., 1., 0.]]
            .into_iter()
            .map(|v| {
                Value::Struct(Fields(vec![
                    ("x".into(), Value::Float32(v[0])),
                    ("y".into(), Value::Float32(v[1])),
                    ("z".into(), Value::Float32(v[2])),
                ]))
            })
            .collect(),
    )
}

#[test]
fn library_preserves_link_order_empty_overrides_and_duplicate_rejection() {
    let mut m = Memory::default();
    m.put("s", 1, "PersonalityLibraryEntityData", &[]);
    m.put(
        "s",
        2,
        "PersonalityEntityData",
        &[("NOSRebuildPacingCurve", curve(4.))],
    );
    m.put(
        "s",
        3,
        "PersonalityEntityData",
        &[("NOSRebuildPacingCurve", Value::Array(vec![]))],
    );
    m.put(
        "s",
        4,
        "PacingLibraryEntityData",
        &[("NOSRebuildPacingCurve", curve(12.))],
    );
    for (order, expected) in [
        (vec![3, 2], Some([6., 2.])),
        (vec![2, 3], Some([2., 6.])),
        (vec![2, 2], None),
        (vec![], None),
        (vec![4, 2], None),
    ] {
        m.put(
            "s",
            0,
            "Blueprint",
            &[(
                "LinkConnections",
                Value::Array(
                    order
                        .into_iter()
                        .map(|i| edge(local(1), 30, local(i), 0))
                        .collect(),
                ),
            )],
        );
        let actual = library(&mut Graph::new(&mut m), &site("s", 1), &site("s", 4), 30);
        match expected {
            Some(values) => {
                let actual = actual.unwrap();
                assert_eq!(
                    [
                        actual.evaluate(Some(0), 0.5).unwrap(),
                        actual.evaluate(Some(1), 0.5).unwrap()
                    ],
                    values
                );
            }
            None => assert!(actual.is_err()),
        }
    }
}

#[test]
fn spawner_context_keeps_boolean_and_set_lifetimes_separate() {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1)])),
            ("LinkConnections", Value::Array(vec![])),
        ],
    );
    m.put("r", 1, "ParticipantLogicEntityData", &[]);
    for (index, class) in [
        (1, "ParticipantLogicSpawnerEntityData"),
        (2, "ParticipantLogicSpawnerSetControlledEntityData"),
    ] {
        m.put(
            "s",
            index,
            class,
            &[("ParticipantLogicBlueprint", ptr("r", 0))],
        );
    }
    m.put(
        "s",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2)])),
            ("LinkConnections", Value::Array(vec![])),
        ],
    );
    for (class, expected) in [
        ("ParticipantLogicSpawnerEntityData", 1),
        ("ParticipantLogicSpawnerSetControlledEntityData", 2),
    ] {
        let actual = participant_logic::spawner_context(
            &mut Graph::new(&mut m),
            Site::root("s"),
            "r",
            class,
            10,
            20,
        )
        .unwrap();
        assert_eq!(actual.spawner, site("s", expected));
    }
    m.put(
        "s",
        3,
        "ParticipantLogicSpawnerSetControlledEntityData",
        &[("ParticipantLogicBlueprint", ptr("r", 0))],
    );
    m.put(
        "s",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
            ("LinkConnections", Value::Array(vec![])),
        ],
    );
    assert!(
        participant_logic::spawner_context(
            &mut Graph::new(&mut m),
            Site::root("s"),
            "r",
            "ParticipantLogicSpawnerSetControlledEntityData",
            10,
            20
        )
        .is_err()
    );
}
