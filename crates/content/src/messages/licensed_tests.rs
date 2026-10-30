// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field, PartitionWriter, connection, guid};

const PREFAB: &str = "test/objective";
const CHANNEL: &str = "test/messages_channel";
const MESSAGE: &str = "test/messages/preorderclaimparts";
const AND_A: u32 = 193_450_864;
const AND_B: u32 = 193_450_867;
const LOGIC_OUT: u32 = 193_453_899;
const VALUE: u32 = 225_375_086;
const SEND_MESSAGE: u32 = 1_037_321_618;
const SENDER_CHANNEL: u32 = 4_117_124_221;
const CHANNEL_FIELD: u32 = 4_115_606_960;
const COMPLETED: u32 = 1_514_830_046;
const SET_TRUE: u32 = 2_753_900_881;

type Rows = Vec<Vec<(&'static str, Field)>>;

fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn event(source: u32, out: u32, target: u32, input: u32) -> Vec<(&'static str, Field)> {
    let mut c = connection(source, 0, target, 0);
    c.retain(|(k, _)| ["Source", "Target"].contains(k));
    c.extend([
        (
            "SourceEvent",
            Field::Struct("EventSpec", vec![("Id", Field::UInt(out))]),
        ),
        (
            "TargetEvent",
            Field::Struct("EventSpec", vec![("Id", Field::UInt(input))]),
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
    c
}
fn tables(links: Rows, properties: Rows, events: Rows) -> Vec<(&'static str, Field)> {
    vec![
        ("LinkConnections", Field::Structs("LinkConnection", links)),
        (
            "PropertyConnections",
            Field::Structs("PropertyConnection", properties),
        ),
        (
            "EventConnections",
            Field::Structs("EventConnection", events),
        ),
    ]
}
fn asset(
    seed: u8,
    name: &str,
    class: &str,
    fields: Vec<(&'static str, Field)>,
    objects: Vec<(&'static str, Vec<(&'static str, Field)>)>,
) -> Asset {
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

struct Fixture {
    dir: std::path::PathBuf,
    index: AssetIndex,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The installed shape (`progression_intro` #57): And(Bool set by the
/// licence EventSync, the prefab objective's completion) enables a unique
/// sender whose channel's gameplay instance links to a message manager. A
/// second sender has no SendMessage input. `licence` false drops the
/// EventSync's SetTrue.
fn fixture(licence: bool) -> Fixture {
    let rules = synthetic::progression::rules();
    let stable = rules.ports["stable_id"];
    let mut events = vec![];
    if licence {
        events.push(event(4, LOGIC_OUT, 3, SET_TRUE));
    }
    let intro = asset(
        1,
        &rules.intro,
        "ParticipantLogicBlueprint",
        [
            vec![
                ("Flags", Field::UInt(1)),
                ("Objects", Field::RawPointers((2..9).collect())),
            ],
            tables(
                vec![connection(6, SENDER_CHANNEL, 7, CHANNEL_FIELD)],
                vec![
                    connection(3, VALUE, 5, AND_A),
                    connection(2, COMPLETED, 5, AND_B),
                    connection(5, LOGIC_OUT, 6, SEND_MESSAGE),
                ],
                events,
            ),
        ]
        .concat(),
        vec![
            (
                "LogicPrefabReferenceObjectData",
                vec![("Blueprint", import(2))],
            ), // 2
            ("BoolEntityData", vec![("DefaultValue", Field::Bool(false))]), // 3
            // Flags & 0x1FFFFFF = 23126408.
            (
                "EventSyncEntityData",
                vec![("Flags", Field::UInt(56_680_840))],
            ), // 4
            ("AndEntityData", vec![]), // 5
            (
                "UniqueMessageSenderEntityData",
                vec![("MessageAsset", import(5))],
            ), // 6
            (
                "LinkedSchematicChannelReferenceEntityData",
                vec![("ChannelAsset", import(4))],
            ), // 7
            (
                "UniqueMessageSenderEntityData",
                vec![("MessageAsset", import(5))],
            ), // 8
        ],
    );
    let prefab = asset(
        2,
        PREFAB,
        "LogicPrefabBlueprint",
        [
            vec![
                ("Objects", Field::RawPointers(vec![3, 4])),
                ("Interface", Field::RawPointer(2)),
            ],
            tables(
                vec![connection(3, stable, 4, 0)],
                vec![connection(3, COMPLETED, 2, COMPLETED)],
                vec![],
            ),
        ]
        .concat(),
        vec![
            ("InterfaceDescriptorData", vec![]),
            ("RaceObjectiveEntityData", vec![]),
            (
                "StableUniqueIdEntityData",
                vec![("StableUniqueId64", Field::Long(41))],
            ),
        ],
    );
    let channel = asset(4, CHANNEL, "SchematicChannelAsset", vec![], vec![]);
    let message = asset(
        5,
        MESSAGE,
        "MessageAsset",
        vec![("HashId", Field::UInt(985_423_157))],
        vec![],
    );
    let gameplay = asset(
        6,
        GAMEPLAY,
        "SubWorldData",
        [
            vec![("Objects", Field::RawPointers(vec![2, 3, 4]))],
            tables(vec![connection(2, CHANNEL_FIELD, 4, 0)], vec![], vec![]),
        ]
        .concat(),
        vec![
            (
                "LinkedSchematicChannelEntityData",
                vec![("ChannelAsset", import(4))],
            ),
            (MANAGER, vec![]),
            (MANAGER, vec![]),
        ],
    );
    let dir = std::env::temp_dir().join(format!(
        "nfs-licensed-messages-{}-{:?}-{licence}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut options, _) =
        synthetic::write(&dir, &[intro, prefab, channel, message, gameplay]).unwrap();
    options.identity.scenes = crate::BuildIdentity::supported().scenes;
    let index = AssetIndex::open(&dir, &options).unwrap();
    Fixture { dir, index }
}

#[test]
fn a_licensed_sender_names_its_report_objective_message_and_manager() {
    let mut f = fixture(true);
    let managers: Vec<_> = f
        .index
        .scene_bindings(GAMEPLAY)
        .unwrap()
        .into_iter()
        .filter(|b| b.class == MANAGER)
        .collect();
    let [_, second] = managers.as_slice() else {
        panic!("two managers")
    };
    assert_eq!(
        f.index.licensed_messages().unwrap(),
        [LicensedMessage {
            accept: 23_126_408,
            objective: 41,
            hash: 985_423_157,
            manager: second.serializer,
        }]
    );
}

#[test]
fn a_sender_without_a_licence_report_is_not_licensed() {
    assert!(fixture(false).index.licensed_messages().unwrap().is_empty());
}
