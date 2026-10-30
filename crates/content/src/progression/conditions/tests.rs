// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;
const STABLE: u32 = 77;
const CHANNEL: u32 = 4_117_124_221;

#[test]
fn explicit_conditions_preserve_unknown_inputs_and_bound_the_program() {
    let p = Condition::all(&[Fact::Active(1), Fact::Completed(2)]).unwrap();
    assert!(!p.evaluate(|_| Some(false)).unwrap());
    assert!(
        p.evaluate(|f| match f {
            Fact::Active(_) => Some(false),
            _ => None,
        })
        .is_err()
    );
    assert!(Condition::all(&[]).is_err());
    assert!(Condition::all(&[Fact::Active(0)]).is_err());
    assert!(Condition::all(&[Fact::Active(1), Fact::Active(1)]).is_err());
    assert!(Condition::all(&(1..=MAX_NODES as u64).map(Fact::Active).collect::<Vec<_>>()).is_err());
}

fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=14).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), STABLE, local(2), 0),
                    edge(local(3), CHANNEL, local(8), CHANNEL),
                    edge(local(13), CHANNEL, local(8), CHANNEL),
                    edge(local(10), CHANNEL, local(11), 0),
                ]),
            ),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), ACTIVE, local(5), 1),
                    edge(local(3), READ, local(5), 2),
                    edge(local(5), OUT, local(6), IN),
                    edge(local(1), COMPLETED, local(7), IN),
                    edge(local(3), READ, local(12), 1),
                    edge(local(13), READ, local(12), 2),
                ]),
            ),
            ("EventConnections", Value::Array(vec![])),
        ],
    );
    m.put("a", 1, "RaceObjectiveEntityData", &[]);
    m.put(
        "a",
        2,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(101))],
    );
    for (sender, asset, hash) in [(3, 4, 71), (13, 14, 72)] {
        m.put(
            "a",
            sender,
            "UniqueMessageSenderEntityData",
            &[("MessageAsset", local(asset))],
        );
        m.put(
            "a",
            asset,
            "MessageAsset",
            &[("HashId", Value::UInt32(hash))],
        );
    }
    for (node, class) in [(5, "AndEntityData"), (12, "OrEntityData")] {
        m.put(
            "a",
            node,
            class,
            &[
                ("InputCount", Value::UInt32(2)),
                (
                    "Realm",
                    Value::Enum {
                        value: 0,
                        name: Some("Realm_Server".into()),
                    },
                ),
            ],
        );
    }
    m.put("a", 6, "SyncedBoolEntityData", &[]);
    m.put("a", 7, "NotEntityData", &[]);
    for (node, class) in [
        (8, "LinkedSchematicChannelReferenceEntityData"),
        (10, "LinkedSchematicChannelEntityData"),
    ] {
        m.put("a", node, class, &[("ChannelAsset", local(9))]);
    }
    m.put("a", 9, "ChannelAsset", &[]);
    m.put("a", 11, "MessageManagerEntityData", &[]);
    m
}
fn program(m: &mut Memory, node: usize, port: u32) -> Result<Condition, Error> {
    let mut graph = Graph::new(m);
    let world = graph.read(Site::root("a"))?;
    let managers = BTreeMap::from([(("a".into(), 11), 4)]);
    let mut compiler = Compiler {
        stable: STABLE,
        world: &world,
        managers: &managers,
        active: BTreeSet::new(),
        ops: Vec::new(),
    };
    compiler.visit(
        &mut graph,
        Endpoint {
            site: Site {
                key: ("a".into(), node),
                ..Site::root("a")
            },
            port,
        },
    )?;
    Ok(Condition { ops: compiler.ops })
}
fn rows<'a>(m: &'a mut Memory, field: &str) -> &'a mut Vec<Value> {
    let Value::Array(rows) = &mut m
        .0
        .get_mut(&("a".into(), 0))
        .unwrap()
        .fields
        .0
        .iter_mut()
        .find(|(k, _)| k == field)
        .unwrap()
        .1
    else {
        panic!()
    };
    rows
}

#[test]
fn activation_waits_for_read_state_and_repeated_evaluation_is_owned_by_caller() {
    let condition = program(&mut fixture(), 6, OUT).unwrap();
    assert_eq!(
        condition.facts().collect::<Vec<_>>(),
        [
            Fact::Active(101),
            Fact::MessageRead {
                hash: 71,
                manager: 4
            }
        ]
    );
    for (active, read, expected) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let evaluate = || {
            condition
                .evaluate(|fact| {
                    Some(match fact {
                        Fact::Active(101) => active,
                        Fact::MessageRead {
                            hash: 71,
                            manager: 4,
                        } => read,
                        _ => panic!(),
                    })
                })
                .unwrap()
        };
        assert_eq!(evaluate(), expected);
        assert_eq!(evaluate(), expected);
    }
    // Sending an invitation and reading it are separate facts. An unavailable
    // table must fail, even while a known inactive objective hides the event.
    assert!(
        condition
            .evaluate(|fact| match fact {
                Fact::Active(_) => Some(false),
                _ => None,
            })
            .is_err()
    );
    let complete = program(&mut fixture(), 7, OUT).unwrap();
    assert!(
        complete
            .evaluate(|f| (f == Fact::Completed(101)).then_some(false))
            .unwrap()
    );
    assert!(
        !complete
            .evaluate(|f| (f == Fact::Completed(101)).then_some(true))
            .unwrap()
    );
}

#[test]
fn alternate_messages_keep_their_own_identity_and_all_inputs_are_required() {
    let condition = program(&mut fixture(), 12, OUT).unwrap();
    for read_hash in [0, 71, 72] {
        assert_eq!(
            condition
                .evaluate(|fact| match fact {
                    Fact::MessageRead { hash, manager: 4 } => Some(hash == read_hash),
                    _ => None,
                })
                .unwrap(),
            read_hash != 0
        );
    }
    assert!(
        condition
            .evaluate(|f| (f
                == Fact::MessageRead {
                    hash: 71,
                    manager: 4
                })
            .then_some(true))
            .is_err()
    );
}

#[test]
fn unknown_inputs_ambiguous_manager_cycles_and_event_written_bools_are_rejected() {
    let mut m = fixture();
    assert!(program(&mut m, 3, READ + 1).is_err());
    rows(&mut m, "LinkConnections").push(edge(local(10), CHANNEL, local(11), 0));
    assert!(program(&mut m, 6, OUT).is_err());
    let mut m = fixture();
    rows(&mut m, "PropertyConnections")[0] = edge(local(6), OUT, local(5), 1);
    assert!(program(&mut m, 6, OUT).is_err());
    let mut m = fixture();
    rows(&mut m, "PropertyConnections").pop();
    assert!(program(&mut m, 12, OUT).is_err());
    let mut m = fixture();
    rows(&mut m, "EventConnections").push(Value::Struct(Fields(vec![("Target".into(), local(6))])));
    assert!(program(&mut m, 6, OUT).is_err());
}
