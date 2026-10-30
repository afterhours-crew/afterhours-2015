// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{self, Asset, Field, PartitionWriter, connection, guid};

const CHAPTER: &str = "test/chapter";
const PREFAB: &str = "test/prefab";
const LAUNCH: u32 = 1_530_399_826;
const ACTIVE: u32 = 1_010_522_579;
const AND_A: u32 = 193_450_864;
const AND_B: u32 = 193_450_867;

type Rows = Vec<Vec<(&'static str, Field)>>;

fn import(seed: u8) -> Field {
    Field::Import(guid(seed), guid(seed ^ 0x80))
}
fn server() -> (&'static str, Field) {
    ("Realm", Field::Enum("Realm", "Realm_Server", 1))
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

struct Fixture {
    dir: std::path::PathBuf,
    index: AssetIndex,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A chapter with three gates: latched at the prefab's launch, 40 s after
/// it, and while the prefab's objective is active after the launch.
fn fixture(edit: impl FnOnce(&mut Rows)) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "nfs-tutorial-gates-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let stable = synthetic::progression::rules().ports["stable_id"];
    let bool_object = || {
        (
            "BoolEntityData",
            vec![server(), ("DefaultValue", Field::Bool(false))],
        )
    };
    let objects = vec![
        (
            "LogicPrefabReferenceObjectData",
            vec![("Blueprint", import(3))],
        ), // 2
        bool_object(), // 3
        (
            "DelayEntityData",
            vec![
                server(),
                ("Delay", Field::Float(40.0)),
                (
                    "TimeDeltaType",
                    Field::Enum("TimeDeltaType", "TimeDeltaType_World", 2),
                ),
            ],
        ), // 4
        bool_object(), // 5
        ("SyncedBoolEntityData", vec![]), // 6
        bool_object(), // 7
        ("AndEntityData", vec![server()]), // 8
        (ACCESSOR, vec![("BoolIdentifier", import(4))]), // 9
        (ACCESSOR, vec![("BoolIdentifier", import(5))]), // 10
        (ACCESSOR, vec![("BoolIdentifier", import(6))]), // 11
    ];
    let mut events = vec![
        event(2, LAUNCH, 3, BOOL_SET_TRUE),
        event(2, LAUNCH, 4, nfs_world::scoring::nodes::IN),
        event(4, nfs_world::scoring::nodes::OUT, 5, BOOL_SET_TRUE),
        event(6, SYNCED_ON_TRUE, 7, BOOL_SET_TRUE),
        event(6, SYNCED_ON_FALSE, 7, BOOL_SET_FALSE),
    ];
    edit(&mut events);
    let properties = vec![
        connection(3, 0, 9, ACCESSOR_VALUE),
        connection(5, 0, 10, ACCESSOR_VALUE),
        connection(8, 0, 11, ACCESSOR_VALUE),
        connection(7, 0, 8, AND_A),
        connection(3, 0, 8, AND_B),
        connection(2, ACTIVE, 6, SYNCED_IN),
    ];
    let mut chapter = vec![(
        "Objects",
        Field::RawPointers((2..objects.len() as u32 + 2).collect()),
    )];
    chapter.extend(tables(vec![], properties, events));
    let mut prefab = vec![
        ("Objects", Field::RawPointers(vec![3, 4])),
        ("Interface", Field::RawPointer(2)),
    ];
    prefab.extend(tables(
        vec![connection(3, stable, 4, 0)],
        vec![connection(3, ACTIVE, 2, ACTIVE)],
        vec![],
    ));
    let gate = |seed: u8, name: &str| Asset {
        name: name.into(),
        partition: PartitionWriter::default().build(
            guid(seed),
            "IdentifiedBoolAsset",
            guid(seed ^ 0x80),
            vec![],
        ),
    };
    let assets = vec![
        Asset {
            name: CHAPTER.into(),
            partition: PartitionWriter::default().build_many(
                guid(2),
                "LogicPrefabBlueprint",
                guid(2 ^ 0x80),
                chapter,
                objects,
            ),
        },
        Asset {
            name: PREFAB.into(),
            partition: PartitionWriter::default().build_many(
                guid(3),
                "LogicPrefabBlueprint",
                guid(3 ^ 0x80),
                prefab,
                vec![
                    ("InterfaceDescriptorData", vec![]),
                    ("RaceObjectiveEntityData", vec![]),
                    (
                        "StableUniqueIdEntityData",
                        vec![("StableUniqueId64", Field::Long(77))],
                    ),
                ],
            ),
        },
        gate(4, "test/gate/launched"),
        gate(5, "test/gate/later"),
        gate(6, "test/gate/active"),
    ];
    let (options, _) = synthetic::write(&dir, &assets).unwrap();
    let index = AssetIndex::open(&dir, &options).unwrap();
    Fixture { dir, index }
}

#[test]
fn latches_delays_and_objective_follows_become_gate_rules() {
    let mut f = fixture(|_| {});
    let rules = f.index.tutorial_gates(CHAPTER).unwrap();
    let launch = |seconds| Term::Since {
        trigger: Trigger::PrefabEvent {
            prefab: PREFAB.into(),
            event: LAUNCH,
        },
        seconds,
    };
    let rule = |gate: &str| rules.iter().find(|r| r.gate == gate).unwrap().all.clone();
    assert_eq!(rules.len(), 3);
    assert_eq!(rule("test/gate/launched"), [launch(0.0)]);
    assert_eq!(rule("test/gate/later"), [launch(40.0)]);
    let active = rule("test/gate/active");
    assert_eq!(active.len(), 2);
    assert!(active.contains(&launch(0.0)));
    assert!(active.contains(&Term::While(Trigger::PrefabProperty {
        prefab: PREFAB.into(),
        property: ACTIVE,
        objective: 77,
    })));
}

#[test]
fn unsupported_setters_fail_closed() {
    // A Bool set by an unknown event source.
    let mut f = fixture(|events| events[0] = event(9, 4, 3, BOOL_SET_TRUE));
    assert!(f.index.tutorial_gates(CHAPTER).is_err());
    // A follow Bool reset by something other than its SyncedBool.
    let mut f = fixture(|events| events[4] = event(2, LAUNCH, 7, BOOL_SET_FALSE));
    assert!(f.index.tutorial_gates(CHAPTER).is_err());
}
