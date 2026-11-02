// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
const STABLE: u32 = 77;
const CHANNEL: u32 = 4_117_124_221;

fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array((1..=6).map(local).collect())),
            (
                "PropertyConnections",
                Value::Array(vec![edge(local(1), ACTIVE, local(3), SEND)]),
            ),
            ("EventConnections", Value::Array(vec![])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), STABLE, local(2), 0),
                    edge(local(3), CHANNEL, local(4), CHANNEL),
                    edge(local(5), CHANNEL, local(6), 0),
                ]),
            ),
        ],
    );
    m.put("a", 1, "RaceObjectiveEntityData", &[]);
    m.put(
        "a",
        2,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(101))],
    );
    m.put(
        "a",
        3,
        "UniqueMessageSenderEntityData",
        &[
            ("MessageAsset", ptr("m", 0)),
            ("HasMessageRestrictor", Value::Bool(false)),
            ("SendMessage", Value::Bool(false)),
        ],
    );
    m.put(
        "m",
        0,
        "MessageAsset",
        &[
            ("HashId", Value::UInt32(71)),
            ("Persisted", Value::Bool(true)),
        ],
    );
    m.put("c", 0, "ChannelAsset", &[]);
    m.put(
        "a",
        4,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "a",
        5,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put("a", 6, "MessageManagerEntityData", &[]);
    m
}
fn compile(m: &mut Memory, selected: &str) -> Result<Vec<Invitation>, Error> {
    let mut g = Graph::new(m);
    let nodes = g.read(Site::root("a"))?;
    read(
        &mut g,
        &nodes,
        &nodes,
        &BTreeMap::from([(("a".into(), 6), 4)]),
        STABLE,
        &BTreeSet::from([selected.into()]),
    )
}
#[test]
fn direct_activation_resolves_its_objective_message_and_manager() {
    assert_eq!(
        compile(&mut fixture(), "m").unwrap(),
        [Invitation {
            objective: 101,
            hash: 71,
            manager: 4
        }]
    );
    assert!(compile(&mut fixture(), "missing").is_err());
}
#[test]
fn restricted_delayed_nonpersistent_and_duplicate_triggers_are_not_direct_activation() {
    let key = ("a".into(), 3);
    for field in ["HasMessageRestrictor", "SendMessage"] {
        let mut m = fixture();
        m.0.get_mut(&key)
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == field)
            .unwrap()
            .1 = Value::Bool(true);
        assert!(compile(&mut m, "m").is_err());
    }
    let mut m = fixture();
    m.put("a", 1, "PropertyGateEntityData", &[]);
    assert!(compile(&mut m, "m").is_err());
    let mut m = fixture();
    m.put(
        "m",
        0,
        "MessageAsset",
        &[
            ("HashId", Value::UInt32(71)),
            ("Persisted", Value::Bool(false)),
        ],
    );
    assert!(compile(&mut m, "m").is_err());
    let mut m = fixture();
    let mut g = Graph::new(&mut m);
    let mut nodes = g.read(Site::root("a")).unwrap();
    nodes.push(nodes.iter().find(|n| n.site.key.1 == 3).unwrap().clone());
    assert!(
        read(
            &mut g,
            &nodes,
            &nodes,
            &BTreeMap::from([(("a".into(), 6), 4)]),
            STABLE,
            &BTreeSet::from(["m".into()])
        )
        .is_err()
    );
}
