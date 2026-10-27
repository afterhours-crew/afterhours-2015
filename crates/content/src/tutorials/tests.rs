// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field, PartitionWriter, connection, guid};

const SCENE: &str = "levels/test/gameplay";
const SET_PORT: u32 = 4;
const NOTIFY_PORT: u32 = 5;
const LOCATIONS: &str = "levels/test/locations";
const LOCATION_PORT: u32 = 6;

type Links = Vec<Vec<(&'static str, Field)>>;
type Case = (Box<dyn FnOnce(&mut Links)>, &'static str);

struct Fixture {
    dir: std::path::PathBuf,
    index: AssetIndex,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn operation(name: &'static str) -> (&'static str, Vec<(&'static str, Field)>) {
    (
        PREDICATE,
        vec![("Operation", Field::Enum("BooleanOperation", name, 0))],
    )
}
fn blueprint(seed: u8, name: &str, accepts: &[u32]) -> Asset {
    Asset {
        name: name.into(),
        partition: PartitionWriter::default().build_many(
            guid(seed),
            "ParticipantLogicBlueprint",
            guid(seed ^ 0x80),
            vec![
                ("Flags", Field::UInt(1)),
                (
                    "Objects",
                    Field::RawPointers((2..accepts.len() as u32 + 2).collect()),
                ),
                (
                    "EventConnections",
                    Field::Structs("EventConnection", vec![]),
                ),
            ],
            accepts
                .iter()
                .map(|id| {
                    (
                        "EventSyncEntityData",
                        vec![("Flags", Field::UInt(0x4000_0000 | id))],
                    )
                })
                .collect(),
        ),
    }
}

/// An ungated, an intro-gated and a location tutorial, plus a channel-port
/// link to a non-channel object that must not count as the channel.
fn fixture(edit: impl FnOnce(&mut Links)) -> Fixture {
    let objects = vec![
        (SPAWNER, vec![("ParticipantLogicBlueprint", import(2))]), // 2
        ("LinkedSchematicChannelEntityData", vec![]),              // 3
        operation("BooleanOperation_NOT"),                         // 4
        (PROPERTY, vec![]),                                        // 5
        (NOTIFICATION, vec![]),                                    // 6
        (SPAWNER, vec![("ParticipantLogicBlueprint", import(3))]), // 7
        ("LinkedSchematicChannelEntityData", vec![]),              // 8
        operation("BooleanOperation_AND"),                         // 9
        operation("BooleanOperation_NOT"),                         // 10
        (PROPERTY, vec![]),                                        // 11
        (PROPERTY, vec![]),                                        // 12
        (IDENTIFIED, vec![("BoolIdentifier", import(5))]),         // 13
        (SET_SPAWNER, vec![("ParticipantLogicBlueprint", import(4))]), // 14
        ("LinkedSchematicChannelEntityData", vec![]),              // 15
        (SET_PREDICATE, vec![]),                                   // 16
        (SET_FROM_PROPERTY, vec![]),                               // 17
        operation("BooleanOperation_NOT"),                         // 18
        (PROPERTY, vec![]),                                        // 19
        (
            "SubWorldReferenceObjectData",
            vec![("BundleName", Field::CString(LOCATIONS.into()))],
        ), // 20
    ];
    let [op0, op1] = OPERANDS;
    let mut links = vec![
        connection(2, SPAWNER_CHANNEL, 3, 0),
        connection(2, SPAWNER_CHANNEL, 5, 9),
        connection(2, SPAWNER_PREDICATE, 4, 0),
        connection(4, op0, 5, 0),
        connection(6, NOTIFY_PORT, 5, 0),
        connection(7, SPAWNER_CHANNEL, 8, 0),
        connection(7, SPAWNER_PREDICATE, 9, 0),
        connection(9, op0, 10, 0),
        connection(9, op1, 12, 0),
        connection(10, op0, 11, 0),
        connection(13, IDENTIFIED_TARGET, 12, 0),
        connection(14, SPAWNER_CHANNEL, 15, 0),
        connection(14, SPAWNER_PREDICATE, 16, 0),
        connection(16, op1, 17, 0),
        connection(17, SET_PORT, 18, 0),
        connection(18, op0, 19, 0),
        connection(16, op0, 20, LOCATION_PORT),
    ];
    edit(&mut links);
    let scene = Asset {
        name: SCENE.into(),
        partition: PartitionWriter::default().build_many(
            guid(1),
            "SubWorldData",
            guid(0x81),
            vec![
                (
                    "Objects",
                    Field::RawPointers((2..objects.len() as u32 + 2).collect()),
                ),
                ("LinkConnections", Field::Structs("LinkConnection", links)),
                (
                    "PropertyConnections",
                    Field::Structs("PropertyConnection", vec![]),
                ),
            ],
            objects,
        ),
    };
    let assets = vec![
        scene,
        blueprint(2, "gameplay/tutorial/a", &[1234]),
        blueprint(3, "gameplay/tutorial/b", &[]),
        blueprint(4, "gameplay/tutorial/c", &[77]),
        // Two location volumes; the interface field reaches the second.
        Asset {
            name: LOCATIONS.into(),
            partition: PartitionWriter::default().build_many(
                guid(6),
                "SubWorldData",
                guid(6 ^ 0x80),
                vec![
                    ("Objects", Field::RawPointers(vec![3, 4])),
                    ("Interface", Field::RawPointer(2)),
                    (
                        "LinkConnections",
                        Field::Structs("LinkConnection", vec![connection(2, LOCATION_PORT, 4, 0)]),
                    ),
                    (
                        "PropertyConnections",
                        Field::Structs("PropertyConnection", vec![]),
                    ),
                ],
                vec![
                    ("InterfaceDescriptorData", vec![]),
                    (VOLUME_SET, vec![]),
                    (VOLUME_SET, vec![]),
                ],
            ),
        },
        Asset {
            name: "gameplay/tutorial/introfinish".into(),
            partition: PartitionWriter::default().build(
                guid(5),
                "IdentifiedBoolAsset",
                guid(5 ^ 0x80),
                vec![],
            ),
        },
    ];
    let dir = std::env::temp_dir().join(format!(
        "nfs-tutorials-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut options, _) = synthetic::write(&dir, &assets).unwrap();
    options.identity.scenes = crate::BuildIdentity::supported().scenes;
    let index = AssetIndex::open(&dir, &options).unwrap();
    Fixture { dir, index }
}

#[test]
fn spawners_yield_channel_seen_gate_notification_and_accept() {
    let mut f = fixture(|_| {});
    let bindings: BTreeMap<usize, usize> = f
        .index
        .scene_bindings(SCENE)
        .unwrap()
        .into_iter()
        .map(|b| (b.object, b.serializer))
        .collect();
    let volumes: Vec<usize> = f
        .index
        .scene_bindings(LOCATIONS)
        .unwrap()
        .into_iter()
        .filter(|b| b.class == VOLUME_SET)
        .map(|b| b.serializer)
        .collect();
    let location = Volume {
        scene_key: crate::assets::scene_key(LOCATIONS),
        serializer: volumes[1],
    };
    let tutorials = f.index.tutorials_in(SCENE, SCENE).unwrap();
    // Object indices are 0-based: local pointer n is object n - 1.
    assert_eq!(
        tutorials,
        [
            Tutorial {
                blueprint: "gameplay/tutorial/a".into(),
                channel: bindings[&2],
                seen: 4,
                gate: Gate::None,
                notification: Some(bindings[&5]),
                accept: Some(1234),
                context: None,
            },
            Tutorial {
                blueprint: "gameplay/tutorial/b".into(),
                channel: bindings[&7],
                seen: 10,
                gate: Gate::Identified("gameplay/tutorial/introfinish".into()),
                notification: None,
                accept: None,
                context: None,
            },
            Tutorial {
                blueprint: "gameplay/tutorial/c".into(),
                channel: bindings[&14],
                seen: 18,
                gate: Gate::Location(location),
                notification: None,
                accept: Some(77),
                context: None,
            },
        ]
    );
}

#[test]
fn unsupported_or_ambiguous_wiring_fails_closed() {
    let [op0, _] = OPERANDS;
    let cases: Vec<Case> = vec![
        // A second channel instance for one spawner.
        (
            Box::new(|l| l.push(connection(2, SPAWNER_CHANNEL, 8, 0))),
            "tutorial channel count",
        ),
        // A second predicate port.
        (
            Box::new(|l| l.push(connection(2, 77, 4, 0))),
            "tutorial spawner predicate count",
        ),
        // NOT(NOT(seen)) is not an installed shape (link 3 is NOT -> seen).
        (
            Box::new(move |l| l[3] = connection(4, op0, 10, 0)),
            "tutorial predicate shape",
        ),
        // One notification on two seen values.
        (
            Box::new(|l| l.push(connection(6, NOTIFY_PORT, 11, 0))),
            "tutorial notification shared",
        ),
        // A location predicate on the plain spawner (link 2).
        (
            Box::new(|l| l[2] = connection(2, SPAWNER_PREDICATE, 16, 0)),
            "tutorial spawner kind",
        ),
    ];
    for (edit, error) in cases {
        let mut f = fixture(edit);
        let text = f.index.tutorials_in(SCENE, SCENE).unwrap_err().to_string();
        assert!(text.contains(error), "{text}");
    }
}

#[test]
fn installed_predicate_shapes_map_to_gates() {
    let seen = || Box::new(Expr::Seen(3));
    let not = |e| Box::new(Expr::Not(e));
    assert_eq!(inner_shape(Expr::Not(seen())).unwrap(), (3, Gate::None));
    assert_eq!(
        inner_shape(Expr::And(Box::new(Expr::Gate("g".into())), not(seen()))).unwrap(),
        (3, Gate::Identified("g".into()))
    );
    assert!(matches!(
        shape(Expr::Location(not(seen()), Site::root("x"))).unwrap(),
        (3, Shape::Location(_))
    ));
    for wrong in [
        Expr::Seen(3),
        Expr::Not(not(seen())),
        Expr::And(not(seen()), not(seen())),
        Expr::Location(
            Box::new(Expr::And(Box::new(Expr::Gate("g".into())), not(seen()))),
            Site::root("x"),
        ),
    ] {
        assert!(shape(wrong).is_err());
    }
}
