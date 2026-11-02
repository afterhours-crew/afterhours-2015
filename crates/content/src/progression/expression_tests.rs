// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::progression::graph::tests::{Memory, edge, local, ptr};

fn condition_owner_fixture() -> Memory {
    let rules = crate::synthetic::progression::rules();
    let p = &rules.ports;
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            (
                "PropertyConnections",
                Value::Array(vec![edge(local(3), p["bool_value"], local(1), p["active"])]),
            ),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), p["stable_id"], local(2), 0),
                    edge(local(4), p["objectives"], local(1), 0),
                    edge(local(5), p["stable_id"], local(6), 0),
                    edge(local(7), p["ints"], local(5), 0),
                ]),
            ),
        ],
    );
    m.put(
        "a",
        1,
        "RaceObjectiveEntityData",
        &[("Active", Value::Bool(false))],
    );
    m.put(
        "a",
        2,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(101))],
    );
    m.put(
        "a",
        3,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(true))],
    );
    m.put("a", 4, "ObjectivePersistenceEntityData", &[]);
    m.put("a", 5, "IntValueData", &[]);
    m.put(
        "a",
        6,
        "StableUniqueIdEntityData",
        &[("StableUniqueId64", Value::UInt64(202))],
    );
    m.put("a", 7, "ValuesPersistenceEntityData", &[]);
    m
}
fn owned_conditions(
    m: &mut Memory,
) -> Result<crate::progression::conditions::owners::StateOwners, Error> {
    use crate::progression::conditions::Fact;
    let mut graph = Graph::new(m);
    let nodes = graph.read(Site::root("a"))?;
    condition_owners(
        &mut graph,
        &crate::synthetic::progression::rules(),
        &[nodes],
        &BTreeSet::from([
            Fact::Active(101),
            Fact::Completed(101),
            Fact::PositiveInt(202),
        ]),
    )
}
#[test]
fn condition_owners_keep_connected_initial_activation_and_exact_persistent_ids() {
    let mut m = condition_owner_fixture();
    let owners = owned_conditions(&mut m).unwrap();
    assert_eq!(
        owners.objectives,
        BTreeMap::from([(101, crate::progression::Expr::Constant(true))])
    );
    assert_eq!(owners.integers, BTreeSet::from([202]));
    m.put(
        "a",
        3,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    assert_eq!(
        owned_conditions(&mut m).unwrap().objectives[&101],
        crate::progression::Expr::Constant(false)
    );
}
#[test]
fn a_matching_stable_id_is_not_enough_without_its_persistence_owner() {
    let mut m = condition_owner_fixture();
    let links =
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "LinkConnections")
            .unwrap();
    let Value::Array(rows) = &mut links.1 else {
        unreachable!()
    };
    rows.remove(1);
    assert!(
        owned_conditions(&mut m)
            .unwrap_err()
            .to_string()
            .contains("owner missing")
    );
    let mut m = condition_owner_fixture();
    m.put("a", 5, "UnrelatedIntegerEntityData", &[]);
    assert!(
        owned_conditions(&mut m)
            .unwrap_err()
            .to_string()
            .contains("persistence endpoint")
    );
}
#[test]
fn duplicate_persistence_and_unknown_activation_do_not_become_default_false() {
    let mut m = condition_owner_fixture();
    let links =
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "LinkConnections")
            .unwrap();
    let Value::Array(rows) = &mut links.1 else {
        unreachable!()
    };
    rows.push(rows[1].clone());
    assert!(owned_conditions(&mut m).is_err());
    let mut m = condition_owner_fixture();
    m.put(
        "a",
        3,
        "UnknownBooleanEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    assert!(owned_conditions(&mut m).is_err());
}
#[test]
fn blueprint_registration_preserves_stored_flag_widths() {
    for value in [Value::UInt16(0x8001), Value::UInt32(1), Value::Int32(-1)] {
        assert!(registered_bus(Some(&value)).unwrap());
    }
    assert!(!registered_bus(Some(&Value::UInt16(0x8000))).unwrap());
    assert!(registered_bus(Some(&Value::Float32(1.))).is_err());
    assert!(registered_bus(None).is_err());
}
fn site(name: &str, index: usize) -> Site {
    Site {
        key: (name.into(), index),
        ..Site::root(name)
    }
}
#[test]
fn participant_alias_uses_unique_channel_port_publisher_across_branches() {
    let rules = crate::synthetic::progression::rules();
    let p = &rules.ports;
    let mut source = Memory::default();
    for name in ["a", "b"] {
        source.put(
            name,
            0,
            "ParticipantLogicBlueprint",
            &[
                ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
                (
                    "LinkConnections",
                    Value::Array(vec![edge(
                        local(1),
                        p["participant_channel"],
                        local(2),
                        444,
                    )]),
                ),
                (
                    "PropertyConnections",
                    Value::Array(if name == "a" {
                        vec![]
                    } else {
                        vec![edge(local(3), p["bool_value"], local(1), p["bool_value"])]
                    }),
                ),
            ],
        );
        source.put(name, 1, "BoolParticipantPropertyAccessorEntityData", &[]);
        source.put(
            name,
            2,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        source.put(
            name,
            3,
            "BoolEntityData",
            &[("DefaultValue", Value::Bool(true))],
        );
    }
    source.put("c", 0, "ChannelAsset", &[]);
    let mut graph = Graph::new(&mut source);
    let branches = vec![
        graph.read(Site::root("a")).unwrap(),
        graph.read(Site::root("b")).unwrap(),
    ];
    let publishers = publishers(&mut graph, &rules, &branches).unwrap();
    let mut expr = Expressions {
        graph: &mut graph,
        rules: &rules,
        publishers: &publishers,
        active: BTreeSet::new(),
        count: 0,
    };
    assert_eq!(
        expr.expression(Endpoint {
            site: site("a", 1),
            port: p["participant_output"]
        })
        .unwrap(),
        json!({"constant":true})
    );
    let duplicates = vec![branches[1].clone(), branches[1].clone()];
    assert!(super::publishers(&mut graph, &rules, &duplicates).is_err());
    let candidates = publisher_candidates(&mut graph, &rules, &duplicates).unwrap();
    let mut expr = Expressions {
        graph: &mut graph,
        rules: &rules,
        publishers: &candidates,
        active: BTreeSet::new(),
        count: 0,
    };
    assert_eq!(
        expr.expression(Endpoint {
            site: site("b", 3),
            port: p["bool_value"],
        })
        .unwrap(),
        json!({"constant":true})
    );
    assert!(
        expr.expression(Endpoint {
            site: site("a", 1),
            port: p["participant_output"],
        })
        .unwrap_err()
        .to_string()
        .contains("ambiguous participant")
    );
    let empty = BTreeMap::new();
    let mut expr = Expressions {
        graph: &mut graph,
        rules: &rules,
        publishers: &empty,
        active: BTreeSet::new(),
        count: 0,
    };
    assert!(
        expr.expression(Endpoint {
            site: site("a", 1),
            port: p["participant_output"]
        })
        .unwrap_err()
        .to_string()
        .contains("missing participant")
    );
}
#[test]
fn boolean_graph_preserves_input_order_and_rejects_cycles_unknown_ports_and_depth() {
    let rules = crate::synthetic::progression::rules();
    let p = &rules.ports;
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "Blueprint",
        &[(
            "PropertyConnections",
            Value::Array(vec![
                edge(local(2), p["bool_value"], local(1), 99),
                edge(local(3), p["logic_output"], local(1), 88),
                edge(local(4), p["setting_output"], local(3), p["not_input"]),
            ]),
        )],
    );
    source.put("a", 1, "AndEntityData", &[("InputCount", Value::Int32(2))]);
    source.put(
        "a",
        2,
        "BoolEntityData",
        &[("DefaultValue", Value::Bool(false))],
    );
    source.put("a", 3, "NotEntityData", &[]);
    source.put(
        "a",
        4,
        "SettingEntityData",
        &[(
            "BoolSettingName",
            Value::String("Gameplay.NFS16ConsumerBeta".into()),
        )],
    );
    let empty = BTreeMap::new();
    let mut graph = Graph::new(&mut source);
    let mut expr = Expressions {
        graph: &mut graph,
        rules: &rules,
        publishers: &empty,
        active: BTreeSet::new(),
        count: 0,
    };
    assert_eq!(
        expr.expression(Endpoint {
            site: site("a", 1),
            port: p["logic_output"]
        })
        .unwrap(),
        json!({"all":[{"constant":false},{"negate":{"setting":"Gameplay.NFS16ConsumerBeta"}}]})
    );
    assert!(
        expr.expression(Endpoint {
            site: site("a", 2),
            port: 1
        })
        .is_err()
    );
    for cycle in [true, false] {
        let edges = (1..=18)
            .map(|i| {
                edge(
                    local(if cycle { 1 } else { i + 1 }),
                    p["logic_output"],
                    local(i),
                    p["not_input"],
                )
            })
            .collect();
        source.put(
            "a",
            0,
            "Blueprint",
            &[("PropertyConnections", Value::Array(edges))],
        );
        for i in 1..=19 {
            source.put("a", i, "NotEntityData", &[]);
        }
        let mut graph = Graph::new(&mut source);
        let mut expr = Expressions {
            graph: &mut graph,
            rules: &rules,
            publishers: &empty,
            active: BTreeSet::new(),
            count: 0,
        };
        let error = expr
            .expression(Endpoint {
                site: site("a", 1),
                port: p["logic_output"],
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains(if cycle { "cycle" } else { "bound" }));
    }
}
