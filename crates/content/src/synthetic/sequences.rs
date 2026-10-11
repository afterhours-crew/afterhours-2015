// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed sequence topology, with unrelated channel/spline decoys.
use super::{Asset, Field, ObjectFields, PartitionWriter, SceneObjects, connection};
use crate::sequences::Rules;

pub const WAITING: &str = "synthetic/sequences/waiting";
pub const EXIT: &str = "synthetic/sequences/exit";
pub const ENTRY: &str = "synthetic/sequences/entry";
const GAMEPLAY: &str = "levels/runtime/gameplay";
const GARAGE: &str = "levels/runtime/garage";
fn guid(seed: u8) -> [u8; 16] {
    let mut value = [0xf4; 16];
    value[15] = seed;
    value
}
fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn asset(seed: u8, name: &str, class: &str, fields: ObjectFields, objects: SceneObjects) -> Asset {
    Asset {
        name: name.into(),
        partition: PartitionWriter::default().build_many(
            guid(seed),
            class,
            guid(seed ^ 0x80),
            fields,
            objects,
        ),
    }
}
pub fn rules() -> Rules {
    Rules {
        waiting: WAITING.into(),
        exit: EXIT.into(),
        entry: ENTRY.into(),
        entry_timer_events: [9005, 9006, 9007, 9008],
        waiting_scene: GAMEPLAY.into(),
        exit_scene: GARAGE.into(),
        manager_scene: GAMEPLAY.into(),
        root_channels: 9001,
        spawner_channels: 9002,
        spline_link: 9003,
        spline_class_id: 71,
        classes: [
            ("ParticipantStateSequenceBlueprint", None),
            ("ParticipantStateSequenceEntityData", Some("root")),
            (
                "StateSequenceParticipantProviderEntityData",
                Some("provider"),
            ),
            ("LocalParticipantEventEntityData", Some("event")),
            ("LinkedSchematicChannelReferenceEntityData", Some("channel")),
            ("ScriptedSplineCommandEntityData", None),
            ("DelayEntityData", None),
            ("LogicPrefabReferenceObjectData", None),
            ("LogicPrefabBlueprint", None),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.map(str::to_owned)))
        .collect(),
    }
}
pub fn assets_with(mut edit: impl FnMut(&str, &mut ObjectFields, &mut SceneObjects)) -> Vec<Asset> {
    let rules = rules();
    let mut out = Vec::new();
    for (seed, name, label, exit) in [
        (1, WAITING, "Gameplay/WaitingForGarageSequence", false),
        (2, EXIT, "Gameplay/Tests/GarageExitSequence", true),
        (7, ENTRY, "Gameplay/Tests/GarageEntrySequence", true),
    ] {
        let mut objects = vec![
            ("ParticipantStateSequenceEntityData", vec![]),
            ("StateSequenceParticipantProviderEntityData", vec![]),
        ];
        if exit {
            objects.extend([
                ("LocalParticipantEventEntityData", vec![]),
                ("LocalParticipantEventEntityData", vec![]),
            ]);
        }
        let channel = objects.len() as u32 + 2;
        objects.push((
            "LinkedSchematicChannelReferenceEntityData",
            vec![("ChannelAsset", import(3))],
        ));
        let mut links = vec![connection(2, rules.root_channels, channel, 0)];
        if exit {
            objects.push((
                "ScriptedSplineCommandEntityData",
                vec![("LoopWhenFinished", Field::Bool(true))],
            ));
            links.push(connection(channel + 1, rules.spline_link, channel, 9004));
        }
        let mut timer_events = Vec::new();
        if name == ENTRY {
            let delay = objects.len() as u32 + 2;
            objects.push((
                "DelayEntityData",
                vec![
                    ("Delay", Field::Float(7.5)),
                    ("Realm", Field::Enum("Realm", "Realm_Server", 1)),
                    (
                        "TimeDeltaType",
                        Field::Enum("TimeDeltaType", "TimeDeltaType_World", 2),
                    ),
                    ("AutoStart", Field::Bool(false)),
                ],
            ));
            let [started, start, elapsed, stop] = rules.entry_timer_events;
            for (source, target, out_event, in_event) in
                [(2, delay, started, start), (delay, 2, elapsed, stop)]
            {
                let mut c = connection(source, 0, target, 0);
                c.retain(|(k, _)| ["Source", "Target"].contains(k));
                c.extend([
                    (
                        "SourceEvent",
                        Field::Struct("EventSpec", vec![("Id", Field::UInt(out_event))]),
                    ),
                    (
                        "TargetEvent",
                        Field::Struct("EventSpec", vec![("Id", Field::UInt(in_event))]),
                    ),
                    (
                        "TargetType",
                        Field::Enum(
                            "EventConnectionTargetType",
                            "EventConnectionTargetType_Server",
                            1,
                        ),
                    ),
                ]);
                timer_events.push(c);
            }
        }
        if name == ENTRY {
            links.push(connection(
                2,
                rules.root_channels,
                objects.len() as u32 + 2,
                9009,
            ));
            objects.push((
                "LogicPrefabReferenceObjectData",
                vec![("Blueprint", import(8))],
            ));
        }
        let mut fields = vec![
            ("Name", Field::CString(label.into())),
            ("Flags", Field::UInt(1)),
            (
                "Objects",
                Field::RawPointers((2..=objects.len() as u32 + 1).collect()),
            ),
            ("LinkConnections", Field::Structs("LinkConnection", links)),
            (
                "EventConnections",
                Field::Structs("EventConnection", timer_events),
            ),
        ];
        edit(name, &mut fields, &mut objects);
        out.push(asset(
            seed,
            name,
            "ParticipantStateSequenceBlueprint",
            fields,
            objects,
        ));
    }
    let mut client_fields = vec![
        ("Flags", Field::UInt(14)),
        ("Interface", Field::RawPointer(3)),
        (
            "LinkConnections",
            Field::Structs("LinkConnection", vec![connection(3, 9009, 2, 0)]),
        ),
        ("Objects", Field::RawPointers(vec![2])),
    ];
    let mut client_objects = vec![(
        "LinkedSchematicChannelReferenceEntityData",
        vec![("ChannelAsset", import(9))],
    )];
    client_objects.push(("InterfaceDescriptorData", vec![]));
    edit(
        "synthetic/sequences/client_presentation",
        &mut client_fields,
        &mut client_objects,
    );
    out.push(asset(
        8,
        "synthetic/sequences/client_presentation",
        "LogicPrefabBlueprint",
        client_fields,
        client_objects,
    ));
    out.push(asset(
        9,
        "synthetic/sequences/presentation_channel",
        "SchematicChannelAsset",
        vec![],
        vec![],
    ));
    out.push(asset(
        3,
        "synthetic/sequences/channel",
        "SchematicChannelAsset",
        vec![],
        vec![],
    ));
    let mut fields = vec![
        ("Flags", Field::UInt(1)),
        ("Objects", Field::RawPointers(vec![2])),
    ];
    let mut objects = vec![("StateSequenceManagerEntityData", vec![])];
    edit("synthetic/sequences/manager", &mut fields, &mut objects);
    out.push(asset(
        4,
        "synthetic/sequences/manager",
        "LogicPrefabBlueprint",
        fields,
        objects,
    ));
    for seed in [5, 6] {
        out.push(asset(
            seed,
            &format!("synthetic/sequences/spline{seed}"),
            "ScriptedAISplineEntityData",
            vec![],
            vec![],
        ));
    }
    out
}
fn append(fields: &mut ObjectFields, objects: &mut SceneObjects, seed: u8) {
    let exit = seed != 1;
    let rules = rules();
    if !fields.iter().any(|(k, _)| *k == "Flags") {
        fields.push(("Flags", Field::UInt(1)));
    }
    let start = objects.len() as u32 + 2;
    let mut added = vec![
        (
            "StateSequenceRefererEntityData",
            vec![("Sequence", import(seed))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(3))],
        ),
        (
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(3))],
        ),
    ];
    if seed == 7 {
        added.push((
            "LinkedSchematicChannelEntityData",
            vec![("ChannelAsset", import(9))],
        ));
    }
    if !exit {
        added.push((
            "LogicPrefabReferenceObjectData",
            vec![("Blueprint", import(4))],
        ));
    }
    for (key, value) in fields.iter_mut() {
        if *key == "Objects" {
            let Field::RawPointers(values) = value else {
                panic!()
            };
            values.extend(start..start + added.len() as u32);
        }
        if *key == "LinkConnections" {
            let Field::Structs(_, values) = value else {
                panic!()
            };
            values.push(connection(start, rules.spawner_channels, start + 2, 0));
            if seed == 7 {
                values.push(connection(start, rules.spawner_channels, start + 3, 0));
            }
            if exit {
                let mut row = connection(start + 2, 9004, 0, 0);
                row.iter_mut().find(|(k, _)| *k == "Target").unwrap().1 = import(5);
                values.push(row);
            }
        }
    }
    objects.extend(added);
}
pub fn scene_assets_with(
    mut edit: impl FnMut(&str, &mut ObjectFields, &mut SceneObjects),
) -> Vec<Asset> {
    super::garage_layout_with(|gf, go, pf, po| {
        append(gf, go, 2);
        append(gf, go, 7);
        append(pf, po, 1);
        edit(GARAGE, gf, go);
        edit(GAMEPLAY, pf, po);
    })
    .into_iter()
    .filter(|a| [GAMEPLAY, GARAGE].contains(&a.name.as_str()))
    .collect()
}
pub fn install(assets: &mut Vec<Asset>) {
    assets.retain(|a| {
        ![
            GAMEPLAY,
            GARAGE,
            "levels/runtime/gameplay_networkregistry_win32",
            "levels/runtime/garage_networkregistry_win32",
        ]
        .contains(&a.name.as_str())
    });
    assets.extend(scene_assets_with(|_, _, _| {}));
    assets.extend(assets_with(|_, _, _| {}));
    let base = super::guid;
    for (seed, scene, mut pointers) in [
        (
            0x2a,
            GAMEPLAY,
            vec![(base(0x29), base(0x39)), (base(0x27), base(0x37))],
        ),
        (0x2b, GARAGE, vec![(base(0x28), base(0x38))]),
    ] {
        let sequence = if scene == GAMEPLAY { 1 } else { 2 };
        pointers.push((guid(sequence), guid(sequence ^ 0x80)));
        if scene == GARAGE {
            pointers.push((guid(7), guid(7 ^ 0x80)));
            for spline in [6, 5] {
                pointers.push((guid(spline), guid(spline ^ 0x80)));
            }
        }
        assets.push(super::registry(seed, scene, pointers));
    }
}
