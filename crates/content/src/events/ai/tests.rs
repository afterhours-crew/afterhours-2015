// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::{
    Frame,
    tests::{Memory, edge, local, ptr},
};
use nfs_frostbite::ebx::{Fields, Pointer};

fn ports() -> Links {
    Links {
        racers: 1,
        provider: 2,
        personas: 3,
        regions: 4,
        secondary_regions: 5,
        spawn: 6,
        participants: 7,
        actors: 8,
        personality_library: 9,
        personalities: 10,
        personality: 11,
    }
}
fn site(asset: &str, index: usize) -> Site {
    Site {
        key: (asset.into(), index),
        ..Site::root(asset)
    }
}
fn vector(v: [f32; 3]) -> Value {
    Value::Struct(Fields(
        ["x", "y", "z"]
            .into_iter()
            .zip(v)
            .map(|(k, v)| (k.into(), Value::Float32(v)))
            .collect(),
    ))
}
fn transform(basis: [[f32; 3]; 3], position: [f32; 3]) -> Value {
    Value::Struct(Fields(vec![
        ("right".into(), vector(basis[0])),
        ("up".into(), vector(basis[1])),
        ("forward".into(), vector(basis[2])),
        ("trans".into(), vector(position)),
    ]))
}
const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
fn fixture() -> Memory {
    let mut m = Memory::default();
    let p = ports();
    m.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), EVENT_SPAWNER, local(2), 0),
                edge(local(2), SPAWNS, local(3), 0),
                edge(local(2), SPAWNS, local(4), 0),
                edge(local(3), p.provider, local(5), 0),
                edge(local(4), p.provider, local(5), 0),
                edge(local(5), p.provider, local(6), 777),
                edge(local(7), 777, local(8), 0),
                edge(local(8), p.personas, local(10), 0),
                edge(local(8), p.personas, local(11), 0),
                edge(local(8), p.personas, local(12), 0),
                edge(local(19), p.personalities, local(20), 0),
                edge(local(19), p.personalities, local(21), 0),
                edge(local(11), p.personality, local(21), 0),
                edge(local(12), p.personality, local(20), 0),
                edge(local(3), SPAWN_LOCATION, local(14), 501),
                edge(local(4), SPAWN_LOCATION, local(14), 502),
            ]),
        )],
    );
    m.put("a", 1, "LogicPrefabReferenceObjectData", &[]);
    m.put(
        "a",
        2,
        "SpawnAiForEventEntityData",
        &[(
            "AiActionOnGameModeCompletion",
            Value::Enum {
                value: 3,
                name: Some("AiAction_GiveIndividualAiToDirectorOnGameModeCompletion".into()),
            },
        )],
    );
    for index in [3, 4] {
        m.put(
            "a",
            index,
            "AiSpawnEntityData",
            &[
                (
                    "Team",
                    Value::Enum {
                        value: 2,
                        name: Some("Team2".into()),
                    },
                ),
                ("CanOnlySpawnOnce", Value::Bool(false)),
                ("StoreParticipantPointer", Value::Bool(false)),
                ("IgnorePersonaAvailability", Value::Bool(false)),
            ],
        );
    }
    m.put(
        "a",
        5,
        "UnionVehicleProviderEntityData",
        &[("VehicleTags", Value::Array(vec![local(17)]))],
    );
    m.put(
        "a",
        6,
        "LinkedSchematicChannelRelayEntityData",
        &[("ChannelAsset", local(22))],
    );
    m.put(
        "a",
        7,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", local(22))],
    );
    m.put("a", 8, "PersonaSetEntityData", &[]);
    for index in [10, 11, 12] {
        m.put(
            "a",
            index,
            "PersonaEntityData",
            &[
                ("PersonaName", Value::String(format!("Persona {index}"))),
                ("PersonaNameId", Value::String(String::new())),
                (
                    "VehicleTags",
                    Value::Array(vec![local(if index == 10 { 18 } else { 17 })]),
                ),
                ("VehicleItemData", local(23)),
                ("VehicleStyle", Value::Pointer(Pointer::Null)),
                ("AdditionalItems", Value::Array(vec![])),
                ("OverrideCarColor", Value::Bool(false)),
                ("OverridePerformance", Value::Bool(false)),
                ("OverrideGripValue", Value::Bool(false)),
            ],
        );
    }
    m.put(
        "a",
        14,
        "PrefabReferenceObjectData",
        &[
            ("Blueprint", ptr("b", 0)),
            (
                "BlueprintTransform",
                transform([[0., 0., -1.], [0., 1., 0.], [1., 0., 0.]], [10., 20., 30.]),
            ),
        ],
    );
    m.put("a", 17, "VehicleTag", &[("TagHash", Value::UInt32(7))]);
    m.put("a", 18, "VehicleTag", &[("TagHash", Value::UInt32(8))]);
    m.put("a", 19, "PersonalityLibraryEntityData", &[]);
    m.put(
        "a",
        20,
        "PersonalityEntityData",
        &[("PacingSkill", Value::Float32(0.7))],
    );
    m.put(
        "a",
        21,
        "PersonalityEntityData",
        &[("PacingSkill", Value::Float32(0.3))],
    );
    m.put("a", 22, "LinkedSchematicChannelAsset", &[]);
    m.put("a", 23, "RaceVehicleItemData", &[]);
    m.0.get_mut(&("a".into(), 23)).unwrap().guid = Some([7; 16]);
    m.put(
        "b",
        0,
        "PrefabBlueprint",
        &[
            ("Interface", local(1)),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 501, local(2), 0),
                    edge(local(1), 502, local(3), 0),
                ]),
            ),
        ],
    );
    m.put("b", 1, "InterfaceDescriptorData", &[]);
    m.put(
        "b",
        2,
        "LocatorEntityData",
        &[("Transform", transform(IDENTITY, [2., 0., 0.]))],
    );
    m.put(
        "b",
        3,
        "LocatorEntityData",
        &[("Transform", transform(IDENTITY, [0., 0., 4.]))],
    );
    m
}
fn managers(m: &Memory) -> Vec<Node> {
    [7, 19]
        .map(|i| Node {
            site: site("a", i),
            object: m.0[&("a".into(), i)].clone(),
        })
        .to_vec()
}
fn read(m: &mut Memory) -> Result<Roster, Error> {
    let manager = managers(m);
    extract(&mut Graph::new(m), &site("a", 1), &manager, ports())
}
fn set(m: &mut Memory, asset: &str, index: usize, field: &str, value: Value) {
    m.0.get_mut(&(asset.into(), index))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == field)
        .unwrap()
        .1 = value;
}
#[test]
fn roster_follows_relay_filters_without_renumbering_and_composes_grid() {
    let roster = read(&mut fixture()).unwrap();
    assert_eq!(
        roster.completion,
        Some(Completion::GiveIndividualToDirectorAtCompletion)
    );
    assert_eq!(roster.personas.len(), 2);
    assert_eq!(roster.pacing_skills, vec![Some(0.3), Some(0.7)]);
    assert_eq!(roster.personas[0].provider_index, 1);
    assert_eq!(roster.personas[1].provider_index, 2);
    assert_eq!(roster.personas[0].personality_index, 1);
    assert_eq!(roster.personas[1].personality_index, 0);
    assert_eq!(roster.spawns.len(), 2);
    assert_eq!(roster.spawns[0].personas, vec![0, 1]);
    assert_eq!(roster.spawns[1].personas, vec![0, 1]);
    assert_eq!(roster.spawns[0].position, [10., 20., 28.]);
    assert_eq!(roster.spawns[1].position, [14., 20., 30.]);
    assert_eq!(
        roster.spawns[0].basis,
        [[0., 0., -1.], [0., 1., 0.], [1., 0., 0.]]
    );
    assert_eq!(roster.spawns[0].team, 2);
    assert!(
        !roster.spawns[0].once
            && !roster.spawns[0].store_participant
            && !roster.spawns[0].ignore_availability
    );
}
#[test]
fn refuses_ambiguous_channels_empty_providers_and_malformed_spawns() {
    let mut m = fixture();
    let mut nodes = managers(&m);
    nodes.push(nodes[0].clone());
    assert!(extract(&mut Graph::new(&mut m), &site("a", 1), &nodes, ports()).is_err());
    set(&mut m, "a", 17, "TagHash", Value::UInt32(9));
    set(&mut m, "a", 5, "VehicleTags", Value::Array(vec![local(18)]));
    // Only ordinal zero remains; adding an unmet second tag must fail.
    set(
        &mut m,
        "a",
        5,
        "VehicleTags",
        Value::Array(vec![local(17), local(18)]),
    );
    assert!(read(&mut m).is_err());
    let mut m = fixture();
    set(&mut m, "a", 3, "CanOnlySpawnOnce", Value::UInt32(0));
    assert!(read(&mut m).is_err());
    let mut m = fixture();
    set(
        &mut m,
        "b",
        2,
        "Transform",
        transform(IDENTITY, [f32::NAN, 0., 0.]),
    );
    assert!(read(&mut m).is_err());
    let mut m = fixture();
    let Value::Array(edges) = &mut m.0.get_mut(&("a".into(), 0)).unwrap().fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(2), SPAWNS, local(3), 0));
    assert!(read(&mut m).is_err());
}
#[test]
fn nested_instance_transforms_compose_from_the_locator_outwards() {
    let mut m = fixture();
    m.put(
        "a",
        13,
        "PrefabReferenceObjectData",
        &[("BlueprintTransform", transform(IDENTITY, [100., 0., 0.]))],
    );
    let mut s = site("b", 2);
    s.parents = vec![
        Frame {
            bus: ("a".into(), 0),
            reference: ("a".into(), 13),
        },
        Frame {
            bus: ("a".into(), 0),
            reference: ("a".into(), 14),
        },
    ];
    assert_eq!(
        location(&mut Graph::new(&mut m), &s).unwrap().position,
        [110., 20., 28.]
    );
}
#[test]
fn no_linked_spawn_is_an_empty_roster_and_counts_are_bounded() {
    let mut m = fixture();
    set(&mut m, "a", 0, "LinkConnections", Value::Array(vec![]));
    assert_eq!(read(&mut m).unwrap(), Roster::default());
    let mut m = fixture();
    set(
        &mut m,
        "a",
        0,
        "LinkConnections",
        Value::Array(vec![edge(local(1), EVENT_SPAWNER, local(2), 0)]),
    );
    let empty = read(&mut m).unwrap();
    assert!(empty.spawns.is_empty() && empty.personas.is_empty());
    assert_eq!(
        empty.completion,
        Some(Completion::GiveIndividualToDirectorAtCompletion)
    );
    let mut m = fixture();
    let Value::Array(edges) = &mut m.0.get_mut(&("a".into(), 0)).unwrap().fields.0[0].1 else {
        panic!()
    };
    edges.extend((0..MAX_PERSONAS).map(|_| edge(local(8), ports().personas, local(11), 0)));
    assert!(read(&mut m).is_err());
}
