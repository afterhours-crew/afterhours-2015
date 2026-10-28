// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field, PartitionWriter, connection, guid};

const CHAPTER: &str = "test/chapter";
const PREFAB: &str = "test/objective";
const CHANNEL: &str = "test/channel";
const GAMEPLAY: &str = "levels/genesis01/gameplay";
const TARGET: &str = "levels/test/markers";
/// Chapter interface field, progression interface field, target field.
const FIELD: u32 = 501;
const FORWARDED: u32 = 502;
const MARKER: u32 = 503;

type Rows = Vec<Vec<(&'static str, Field)>>;

fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn tables(links: Rows, properties: Rows) -> Vec<(&'static str, Field)> {
    vec![
        ("LinkConnections", Field::Structs("LinkConnection", links)),
        (
            "PropertyConnections",
            Field::Structs("PropertyConnection", properties),
        ),
        (
            "EventConnections",
            Field::Structs("EventConnection", vec![]),
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

/// `not` puts a Not between the objective output and the cast.
fn fixture(not: bool) -> Fixture {
    let rules = synthetic::progression::rules();
    let stable = rules.ports["stable_id"];
    // Intro participant logic: the chapter reference and its channel.
    let intro = asset(
        1,
        &rules.intro,
        "ParticipantLogicBlueprint",
        [
            vec![
                ("Flags", Field::UInt(1)),
                ("Objects", Field::RawPointers(vec![2, 3])),
            ],
            tables(vec![connection(2, FIELD, 3, FIELD)], vec![]),
        ]
        .concat(),
        vec![
            (
                "LogicPrefabReferenceObjectData",
                vec![("Blueprint", import(2))],
            ),
            (
                "LinkedSchematicChannelReferenceEntityData",
                vec![("ChannelAsset", import(4))],
            ),
        ],
    );
    // The chapter: Int accessor = cast(IsActive) or cast(not IsActive).
    let mut objects = vec![
        ("InterfaceDescriptorData", vec![]), // 2
        (
            "LogicPrefabReferenceObjectData",
            vec![("Blueprint", import(3))],
        ), // 3
        ("PropertyCastEntityData", vec![]),  // 4
        ("IntParticipantPropertyAccessorEntityData", vec![]), // 5
    ];
    let mut properties = vec![connection(4, CAST_INT, 5, ACCESSOR_VALUE)];
    if not {
        objects.push(("NotEntityData", vec![])); // 6
        properties.push(connection(3, OBJECTIVE_ACTIVE, 6, NOT_IN));
        properties.push(connection(6, LOGIC_OUT, 4, CAST_BOOL));
    } else {
        properties.push(connection(3, OBJECTIVE_ACTIVE, 4, CAST_BOOL));
    }
    let chapter = asset(
        2,
        CHAPTER,
        "LogicPrefabBlueprint",
        [
            vec![
                (
                    "Objects",
                    Field::RawPointers((3..objects.len() as u32 + 2).collect()),
                ),
                ("Interface", Field::RawPointer(2)),
            ],
            tables(vec![connection(5, INT_CHANNEL, 2, FIELD)], properties),
        ]
        .concat(),
        objects,
    );
    let prefab = asset(
        3,
        PREFAB,
        "LogicPrefabBlueprint",
        [
            vec![
                ("Objects", Field::RawPointers(vec![3, 4])),
                ("Interface", Field::RawPointer(2)),
            ],
            tables(
                vec![connection(3, stable, 4, 0)],
                vec![connection(3, OBJECTIVE_ACTIVE, 2, OBJECTIVE_ACTIVE)],
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
    // The progression scene forwards the field through its interface.
    let progression = asset(
        5,
        &rules.scene,
        "SubWorldData",
        [
            vec![
                ("Objects", Field::RawPointers(vec![3])),
                ("Interface", Field::RawPointer(2)),
            ],
            tables(vec![connection(3, FIELD, 2, FORWARDED)], vec![]),
        ]
        .concat(),
        vec![
            ("InterfaceDescriptorData", vec![]),
            (
                "LinkedSchematicChannelEntityData",
                vec![("ChannelAsset", import(4))],
            ),
        ],
    );
    let gameplay = asset(
        6,
        GAMEPLAY,
        "SubWorldData",
        [
            vec![("Objects", Field::RawPointers(vec![2, 3]))],
            tables(vec![connection(2, FORWARDED, 3, MARKER)], vec![]),
        ]
        .concat(),
        vec![
            (
                "SubWorldReferenceObjectData",
                vec![("BundleName", Field::CString(rules.scene.clone()))],
            ),
            (
                "SubWorldReferenceObjectData",
                vec![("BundleName", Field::CString(TARGET.into()))],
            ),
        ],
    );
    let target = asset(
        7,
        TARGET,
        "SubWorldData",
        [
            vec![
                ("Objects", Field::RawPointers(vec![3, 4])),
                ("Interface", Field::RawPointer(2)),
            ],
            tables(
                vec![connection(2, MARKER, 3, 0), connection(4, 7, 3, 0)],
                vec![],
            ),
        ]
        .concat(),
        vec![
            ("InterfaceDescriptorData", vec![]),
            ("IntParticipantPropertyEntityData", vec![]),
            ("IntParticipantPropertyClientNotificationEntityData", vec![]),
        ],
    );
    let dir = std::env::temp_dir().join(format!(
        "nfs-chapter-notifications-{}-{:?}-{not}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut options, _) = synthetic::write(
        &dir,
        &[
            intro,
            chapter,
            prefab,
            channel,
            progression,
            gameplay,
            target,
        ],
    )
    .unwrap();
    options.identity.scenes = crate::BuildIdentity::supported().scenes;
    let index = AssetIndex::open(&dir, &options).unwrap();
    Fixture { dir, index }
}

#[test]
fn a_chapter_value_reaches_the_target_scene_notification() {
    for (not, value) in [
        (false, Expr::Active(41)),
        (true, Expr::Not(Box::new(Expr::Active(41)))),
    ] {
        let mut f = fixture(not);
        let serializer = f
            .index
            .scene_bindings(TARGET)
            .unwrap()
            .into_iter()
            .find(|b| b.class == "IntParticipantPropertyClientNotificationEntityData")
            .unwrap()
            .serializer;
        assert_eq!(
            f.index.chapter_notifications().unwrap(),
            [Notification {
                kind: Kind::Int,
                value,
                scene_key: crate::assets::scene_key(TARGET),
                serializer,
                default: 0,
                entry_box: false,
            }]
        );
    }
}

#[test]
fn expressions_evaluate_over_objective_states() {
    let state = |id| match id {
        1 => (true, false),
        2 => (false, true),
        _ => (false, false),
    };
    assert!(Expr::Active(1).eval(&state));
    assert!(!Expr::Active(2).eval(&state));
    assert!(Expr::Completed(2).eval(&state));
    assert!(Expr::Any(vec![Expr::Active(2), Expr::Active(1)]).eval(&state));
    assert!(!Expr::All(vec![Expr::Active(1), Expr::Active(2)]).eval(&state));
    assert!(Expr::Not(Box::new(Expr::Completed(1))).eval(&state));
}
