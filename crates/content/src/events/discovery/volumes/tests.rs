// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local};
use membership::Fact;

fn site(index: usize) -> Site {
    Site {
        key: ("test/volumes".into(), index),
        ..Site::root("test/volumes")
    }
}

fn fixture(operation: &str) -> Memory {
    let mut source = Memory::default();
    source.put(
        "test/volumes",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), PARTICIPANTS, local(2), 0),
                edge(local(2), name("0"), local(3), 0),
                edge(local(2), name("1"), local(4), 0),
                edge(local(3), name("0"), local(5), 0),
                edge(local(3), name("1"), local(6), 0),
                edge(local(5), name("ParticipantManager"), local(7), 0),
                edge(local(6), name("ParticipantStates"), local(8), 0),
                edge(local(4), 2_526_680_631, local(9), 0),
            ]),
        )],
    );
    for (index, class) in [
        (1, START_VOLUME),
        (4, "ParticipantSetBoolPropertyAdapterEntityData"),
        (5, "ParticipantSetPlayerAdapterEntityData"),
        (6, "ParticipantSetStateAdapterEntityData"),
        (7, "ParticipantManagerEntityData"),
        (8, "ParticipantStateEntityData"),
    ] {
        source.put("test/volumes", index, class, &[]);
    }
    for (index, operation) in [(2, "DIF"), (3, "INTERSECTION")] {
        source.put(
            "test/volumes",
            index,
            "ParticipantSetPredicateEntityData",
            &[
                ("NumberOfOperands", Value::UInt32(2)),
                (
                    "Operation",
                    Value::Enum {
                        value: 0,
                        name: Some(operation.into()),
                    },
                ),
            ],
        );
    }
    source.put(
        "test/volumes",
        9,
        "ParticipantInGameModeBoolPropertyEntityData",
        &[
            (
                "Operation",
                Value::Enum {
                    value: 0,
                    name: Some(operation.into()),
                },
            ),
            ("Categories", Value::Array(vec![local(10), local(11)])),
        ],
    );
    for (index, category) in [(10, 0x10), (11, 0x20)] {
        source.put(
            "test/volumes",
            index,
            "GameModeCategory",
            &[("CategoryHash", Value::UInt32(category))],
        );
    }
    source
}

fn compile(source: &mut Memory) -> Result<membership::Program, Error> {
    read(&mut Graph::new(source), &site(1), &mut |_, state| {
        assert_eq!(state, &site(8));
        Ok(Endpoint {
            scene_key: 9,
            serializer: 8,
        })
    })
}

#[test]
fn volume_input_preserves_player_state_and_mode_exclusion_as_separate_facts() {
    let program = compile(&mut fixture("GameModeCategoryOperation_Any")).unwrap();
    let expected = Fact::GameMode {
        categories: BTreeSet::from([0x10, 0x20]),
        all: false,
    };
    assert_eq!(program.facts().last(), Some(&expected));
    for (player, alive, held, wanted) in [
        (true, true, false, true),
        (true, true, true, false),
        (true, true, false, true), // release restores the same living participant
        (true, false, false, false),
        (false, true, false, false),
    ] {
        assert_eq!(
            program
                .evaluate(|fact| Some(match fact {
                    Fact::Player(id) => {
                        assert_eq!(id, &crate::scoring::events::Identity::from(&site(7)));
                        player
                    }
                    Fact::States(states) => {
                        assert_eq!(
                            states,
                            &[Endpoint {
                                scene_key: 9,
                                serializer: 8
                            }]
                        );
                        alive
                    }
                    Fact::GameMode { .. } => held,
                    _ => unreachable!(),
                }))
                .unwrap(),
            wanted
        );
    }
    assert!(
        program
            .evaluate(|f| (!matches!(f, Fact::GameMode { .. })).then_some(false))
            .is_err(),
        "missing mode ownership is unresolved even when another input is false"
    );
    let encoded = serde_json::to_value(&program).unwrap();
    assert_eq!(
        serde_json::from_value::<membership::Program>(encoded.clone()).unwrap(),
        program
    );
    let mut invalid = encoded;
    invalid["ops"][3]["Fact"]["GameMode"]["categories"] = serde_json::json!([]);
    assert!(serde_json::from_value::<membership::Program>(invalid).is_err());
}

#[test]
fn mode_predicate_retains_any_all_and_rejects_missing_or_wrong_category_data() {
    for (operation, all) in [
        ("GameModeCategoryOperation_Any", false),
        ("GameModeCategoryOperation_All", true),
    ] {
        let program = compile(&mut fixture(operation)).unwrap();
        assert_eq!(
            program.facts().last(),
            Some(&Fact::GameMode {
                categories: BTreeSet::from([0x10, 0x20]),
                all
            })
        );
    }
    assert!(compile(&mut fixture("unknown-operation")).is_err());
    let mut source = fixture("GameModeCategoryOperation_Any");
    source.put("test/volumes", 10, "BoolParticipantPropertyEntityData", &[]);
    assert!(compile(&mut source).is_err());
    source = fixture("GameModeCategoryOperation_Any");
    source.put("test/volumes", 9, "BoolParticipantPropertyEntityData", &[]);
    assert!(compile(&mut source).is_err());
    source = fixture("GameModeCategoryOperation_Any");
    source.put(
        "test/volumes",
        9,
        "ParticipantInGameModeBoolPropertyEntityData",
        &[
            (
                "Operation",
                Value::Enum {
                    value: 0,
                    name: Some("GameModeCategoryOperation_Any".into()),
                },
            ),
            ("Categories", Value::Array(vec![])),
        ],
    );
    assert!(compile(&mut source).is_err());
}

#[test]
fn missing_ambiguous_or_wrong_volume_input_cannot_be_a_partial_catalog() {
    for links in [
        vec![],
        vec![edge(local(1), PARTICIPANTS, local(2), 1)],
        vec![
            edge(local(1), PARTICIPANTS, local(2), 0),
            edge(local(1), PARTICIPANTS, local(3), 0),
        ],
    ] {
        let mut source = fixture("GameModeCategoryOperation_Any");
        source.put(
            "test/volumes",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(links))],
        );
        assert!(compile(&mut source).is_err());
    }
}
