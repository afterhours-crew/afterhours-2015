// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn set(m: &mut Memory, index: usize, name: &str, value: Value) {
    let fields = &mut m.0.get_mut(&("r".into(), index)).unwrap().fields.0;
    if let Some((_, old)) = fields.iter_mut().find(|(key, _)| key == name) {
        *old = value;
    } else {
        fields.push((name.into(), value));
    }
}

#[test]
fn enabler_reader_does_not_compile_commands_owned_by_the_mode() {
    let mut m = fixture();
    m.0.get_mut(&("r".into(), 3)).unwrap().fields.0.clear();
    let selected = inclusions(&mut m, "r").unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].1, Inclusion::All);
    assert!(read(&mut m, "r").is_err());
}
fn links() -> Vec<Value> {
    vec![
        edge(local(1), 0xe5c2_fc2d, local(2), 0),
        edge(local(1), 0x2824_4ad5, local(7), 10),
        edge(local(1), 0xe2de_1081, local(7), 11),
        edge(local(3), 0xf0d7_8ff6, local(2), 0),
        edge(local(3), 0x9135_1a19, local(4), 0),
        edge(local(3), 0x9135_1a19, local(5), 0),
        edge(local(3), 0x7073_de02, local(7), 12),
        edge(local(3), 0x775e_e84e, local(7), 10),
        edge(local(3), 0x7835_cd73, local(7), 13),
        edge(local(4), 0x5f93_70e3, local(6), 0),
        edge(local(4), 0x75c6_5578, local(7), 14),
    ]
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            ("LinkConnections", Value::Array(links())),
        ],
    );
    m.put(
        "r",
        1,
        "EnableAiOnParticipantEntityData",
        &[
            (
                "PlayerInclusionMode",
                Value::Enum {
                    value: 2,
                    name: Some("PlayerInclusion_All".into()),
                },
            ),
            ("AddParticipantToStateOnEnableAi", Value::Bool(true)),
            ("RemoveParticipantFromStateOnDisableAi", Value::Bool(false)),
        ],
    );
    m.put(
        "r",
        2,
        "ParticipantStateEntityData",
        &[
            ("HandlesMarkedForDelete", Value::Bool(false)),
            ("SendRpcToClientSide", Value::Bool(true)),
        ],
    );
    m.put(
        "r",
        3,
        "ControlAiInGameModeStateEntityData",
        &[
            ("ContextId", ptr("a", 0)),
            ("ApplyCommandsOneByOne", Value::Bool(false)),
            (
                "ApplyCommandsToParticipantsAlreadyInState",
                Value::Bool(true),
            ),
        ],
    );
    let transform = Value::Struct(Fields(
        ["right", "up", "forward", "trans"]
            .into_iter()
            .enumerate()
            .map(|(i, key)| {
                (
                    key.into(),
                    Value::Struct(Fields(
                        ["x", "y", "z"]
                            .into_iter()
                            .enumerate()
                            .map(|(j, key)| {
                                (key.into(), Value::Float32(if i == j { 1. } else { 0. }))
                            })
                            .collect(),
                    )),
                )
            })
            .collect(),
    ));
    m.put(
        "r",
        4,
        "GotoWaypointCommandEntityData",
        &[
            ("DoDrift", Value::Bool(true)),
            ("LocationTransform", transform),
        ],
    );
    m.put(
        "r",
        5,
        "ScriptedInputCommandEntityData",
        &[
            ("Throttle", Value::Float32(1.)),
            ("Steering", Value::Float32(-0.25)),
            ("Brake", Value::Float32(1.)),
            ("HandBrake", Value::Float32(0.)),
            ("UseNos", Value::Bool(false)),
        ],
    );
    m.put(
        "r",
        6,
        "GotoWaypointCommandPrecisionEntityData",
        &[
            ("LocationReachedRadius", Value::Float32(120.)),
            ("StopAtWaypoint", Value::Bool(true)),
        ],
    );
    m.put(
        "r",
        7,
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
                (10..=14)
                    .map(|id| Value::Struct(Fields(vec![("Id".into(), Value::Int32(id))])))
                    .collect(),
            ),
        )],
    );
    m.put("a", 0, "AiContextId", &[("ContextHash", Value::Int32(-1))]);
    m
}

#[test]
fn commands_resolve_their_own_context_state_and_declared_manager_links() {
    let nodes = read(&mut fixture(), "r").unwrap();
    assert_eq!(nodes.len(), 5);
    let Definition::Enable(enable) = &nodes[0].definition else {
        panic!()
    };
    assert_eq!(enable.inclusion, Inclusion::All);
    assert_eq!(enable.state.as_ref().unwrap().index, 2);
    assert_eq!(enable.actor_manager.channel, "c");
    assert_eq!(enable.actor_manager.port, 10);
    let Definition::Control(control) = &nodes[2].definition else {
        panic!()
    };
    assert_eq!(control.context_hash, u32::MAX);
    assert!(control.apply_existing);
    assert_eq!(
        control.commands.iter().map(|i| i.index).collect::<Vec<_>>(),
        [4, 5]
    );
    let Definition::Goto(goto) = &nodes[3].definition else {
        panic!()
    };
    assert_eq!(goto.radius, 120.);
    assert!(goto.stop && goto.drift);
    assert_eq!(goto.location.as_ref().unwrap().port, 14);
    let Definition::Scripted(scripted) = &nodes[4].definition else {
        panic!()
    };
    assert_eq!(scripted.steering, -0.25);
}

#[test]
fn precision_and_provider_are_optional_but_missing_or_wrong_targets_are_not() {
    let mut m = fixture();
    set(
        &mut m,
        0,
        "LinkConnections",
        Value::Array(links()[..9].to_vec()),
    );
    let nodes = read(&mut m, "r").unwrap();
    let Definition::Goto(goto) = &nodes[3].definition else {
        panic!()
    };
    assert_eq!(goto.radius, 150.);
    assert!(!goto.stop);
    assert_eq!(goto.location, None);
    let mut m = fixture();
    m.put("r", 6, "UnrelatedPrecision", &[]);
    assert!(read(&mut m, "r").is_err());
    let mut m = fixture();
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[("Links", Value::Array(vec![]))],
    );
    assert!(read(&mut m, "r").is_err());
}

#[test]
fn inclusion_discriminants_context_classes_and_unsupported_input_curves_are_checked() {
    for (value, name, expected) in [
        (0, "PlayerInclusion_OnlyHuman", Inclusion::Human),
        (1, "PlayerInclusion_OnlyAi", Inclusion::Ai),
        (2, "PlayerInclusion_All", Inclusion::All),
    ] {
        let mut m = fixture();
        set(
            &mut m,
            1,
            "PlayerInclusionMode",
            Value::Enum {
                value,
                name: Some(name.into()),
            },
        );
        let nodes = read(&mut m, "r").unwrap();
        let Definition::Enable(enable) = &nodes[0].definition else {
            panic!()
        };
        assert_eq!(enable.inclusion, expected);
        set(
            &mut m,
            1,
            "PlayerInclusionMode",
            Value::Enum {
                value: value + 1,
                name: Some(name.into()),
            },
        );
        assert!(read(&mut m, "r").is_err());
    }
    let mut m = fixture();
    m.put(
        "a",
        0,
        "UnrelatedContext",
        &[("ContextHash", Value::Int32(1))],
    );
    assert!(read(&mut m, "r").is_err());
    let mut m = fixture();
    let mut edges = links();
    edges.push(edge(local(5), 0xbc0e_49ac, local(6), 0));
    set(&mut m, 0, "LinkConnections", Value::Array(edges));
    assert!(
        read(&mut m, "r")
            .unwrap_err()
            .to_string()
            .contains("curves")
    );
    let mut m = fixture();
    let mut edges = links();
    edges[3] = edge(local(3), 0xf0d7_8ff6, local(5), 0);
    set(&mut m, 0, "LinkConnections", Value::Array(edges));
    assert!(read(&mut m, "r").is_err());
}
