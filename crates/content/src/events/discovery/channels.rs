// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Loaded scalar channel publishers. A Boolean default is initial owned state,
//! never a proof that its later event writes cannot occur.
use super::*;
use crate::scoring::{
    events::{self, Identity},
    nodes as scalar,
    realms::Listeners,
};
use nfs_world::scoring::{events::Delivery, nodes};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Signal {
    pub source: Identity,
    pub event: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Publisher {
    pub channel: Identity,
    pub input: Identity,
    pub configuration: scalar::Configuration,
    /// Incoming source outputs and their commands, in authored connection order.
    pub writes: Vec<(Signal, u32)>,
}

fn server_channel(object: &Object) -> bool {
    matches!(
        object.fields.get("Realm").and_then(Value::as_enum_name),
        Some("Realm_Server" | "Realm_ClientAndServer")
    )
}

pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    roots: &[Site],
    selected: &BTreeSet<Property>,
) -> Result<BTreeMap<Property, Vec<Publisher>>, Error> {
    if selected.len() > 16 || roots.len() > 256 {
        return Err(bad("map control channel bound"));
    }
    if selected.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut result: BTreeMap<_, Vec<_>> = selected.iter().cloned().map(|p| (p, vec![])).collect();
    let mut seen = BTreeSet::new();
    for root in roots {
        for node in g.read(root.clone())? {
            if node.object.class != "SchematicChannelEntityData" {
                continue;
            }
            let schema_key = g.target_field(&node.site, "Channel")?;
            if !selected.iter().any(|p| p.channel == schema_key.0) {
                continue;
            }
            let identity = Identity::from(&node.site);
            if !seen.insert(identity.clone()) {
                continue;
            }
            let schema = g.source.object(&schema_key)?;
            if schema.class != "SchematicChannelAsset" || !server_channel(&node.object) {
                return Err(bad("map control channel schema or realm"));
            }
            let ports = crate::events::lifecycle::channels::property_ids(
                &node.object,
                &schema,
                "InputProperties",
            )?;
            let references = crate::events::lifecycle::channels::property_ids(
                &node.object,
                &schema,
                "InputRefProperties",
            )?;
            for property in selected.iter().filter(|p| p.channel == schema_key.0) {
                if references.contains(&property.port) {
                    return Err(bad("map control reference publisher"));
                }
                if !ports.contains(&property.port) {
                    continue;
                }
                if !Listeners::read(&node.object)?.is_some_and(|l| l.properties.includes_server()) {
                    return Err(bad("map control channel property listener"));
                }
                let inputs = g.inputs(&node.site, property.port)?;
                let [input] = inputs.as_slice() else {
                    return Err(bad("map control publisher input"));
                };
                let object = g.object(&input.site)?;
                if object.class != "BoolEntityData"
                    || input.port != nodes::FLOAT_VALUE
                    || !g.input_ports(&input.site)?.is_empty()
                    || !crate::scoring::creation::compile(&object)?
                        .is_some_and(|r| r.includes_server())
                    || !Listeners::read(&object)?.is_some_and(|l| l.events.includes_server())
                {
                    return Err(bad("map control Boolean owner"));
                }
                let owner = Identity::from(&input.site);
                let topology = events::read_scene(
                    g.source,
                    Site {
                        key: input.site.bus.clone(),
                        ..input.site.clone()
                    },
                )?;
                let mut writes = Vec::new();
                for row in &topology.connections {
                    if topology.nodes[usize::from(row.target.node)].identity != owner {
                        continue;
                    }
                    if row.delivery != Delivery::Local
                        || !matches!(
                            row.target.event,
                            nodes::SET_TRUE | nodes::SET_FALSE | nodes::TOGGLE
                        )
                    {
                        return Err(bad("map control Boolean command"));
                    }
                    if writes.len() >= 64 {
                        return Err(bad("map control Boolean writer bound"));
                    }
                    writes.push((
                        Signal {
                            source: topology.nodes[usize::from(row.source.node)]
                                .identity
                                .clone(),
                            event: row.source.event,
                        },
                        row.target.event,
                    ));
                }
                let publishers = result.get_mut(property).expect("selected channel");
                if publishers.len() >= 64 {
                    return Err(bad("map control publisher bound"));
                }
                publishers.push(Publisher {
                    channel: identity.clone(),
                    input: owner,
                    configuration: scalar::compile(&object)?,
                    writes,
                });
            }
        }
    }
    if result.values().any(Vec::is_empty) {
        return Err(bad("map control channel has no loaded publisher"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use nfs_frostbite::ebx::{Fields, Pointer};

    fn row(fields: &[(&str, Value)]) -> Value {
        Value::Struct(Fields(
            fields
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        ))
    }
    fn realm() -> Value {
        Value::Enum {
            value: 1,
            name: Some("Realm_Server".into()),
        }
    }
    fn event(input: u32) -> Value {
        row(&[
            ("Source", local(3)),
            ("Target", local(1)),
            ("SourceEvent", row(&[("Id", Value::UInt32(11))])),
            ("TargetEvent", row(&[("Id", Value::UInt32(input))])),
            (
                "TargetType",
                Value::Enum {
                    value: 3,
                    name: Some("EventConnectionTargetType_Server".into()),
                },
            ),
        ])
    }
    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[(
                "Properties",
                Value::Array(vec![row(&[("Id", Value::UInt32(51)), ("Realm", realm())])]),
            )],
        );
        for asset in ["a", "b"] {
            let mut property = edge(local(1), nodes::FLOAT_VALUE, local(2), 51);
            let Value::Struct(f) = &mut property else {
                unreachable!()
            };
            f.0.push(("Flags".into(), Value::UInt32(3)));
            m.put(
                asset,
                0,
                "LogicPrefabBlueprint",
                &[
                    ("Objects", Value::Array((1..=3).map(local).collect())),
                    ("Interface", Value::Pointer(Pointer::Null)),
                    ("Flags", Value::UInt16(0)),
                    ("PropertyConnections", Value::Array(vec![property])),
                    ("LinkConnections", Value::Array(vec![])),
                    (
                        "EventConnections",
                        Value::Array(vec![event(nodes::SET_TRUE)]),
                    ),
                ],
            );
            m.put(
                asset,
                1,
                "BoolEntityData",
                &[
                    ("Realm", realm()),
                    ("Flags", Value::UInt32(2 << 25)),
                    ("DefaultValue", Value::Bool(false)),
                ],
            );
            m.put(
                asset,
                2,
                "SchematicChannelEntityData",
                &[
                    ("Realm", realm()),
                    ("Flags", Value::UInt32(2 << 27)),
                    ("Channel", ptr("c", 0)),
                    ("InputProperties", Value::Array(vec![Value::UInt32(0)])),
                    ("InputRefProperties", Value::Array(vec![])),
                ],
            );
            m.put(asset, 3, "ExternalEventSource", &[]);
        }
        m
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
    fn selected() -> BTreeSet<Property> {
        BTreeSet::from([Property {
            channel: "c".into(),
            port: 51,
        }])
    }
    fn load(m: &mut Memory) -> Result<BTreeMap<Property, Vec<Publisher>>, Error> {
        read(
            &mut Graph::new(m),
            &[Site::root("a"), Site::root("b")],
            &selected(),
        )
    }
    #[test]
    fn loaded_publishers_keep_independent_state_and_future_commands() {
        let mut m = fixture();
        set(&mut m, "b", 1, "DefaultValue", Value::Bool(true));
        set(
            &mut m,
            "b",
            0,
            "EventConnections",
            Value::Array(vec![event(nodes::SET_FALSE)]),
        );
        let channels = load(&mut m).unwrap();
        let publishers = channels.values().next().unwrap();
        assert_eq!(publishers.len(), 2);
        assert_ne!(publishers[0].input, publishers[1].input);
        for (i, p) in publishers.iter().enumerate() {
            assert_eq!(
                p.configuration.definition,
                nodes::Definition::Boolean { default: i == 1 }
            );
            assert_eq!(p.writes.len(), 1);
            assert_eq!(p.writes[0].0.source.asset, p.input.asset);
            assert_eq!(p.writes[0].0.event, 11);
            assert_eq!(
                p.writes[0].1,
                if i == 0 {
                    nodes::SET_TRUE
                } else {
                    nodes::SET_FALSE
                }
            );
        }
        assert!(read(&mut Graph::new(&mut m), &[], &selected()).is_err());
    }
    #[test]
    fn malformed_or_unknown_publishers_do_not_become_false_defaults() {
        let mut m = fixture();
        set(
            &mut m,
            "b",
            0,
            "EventConnections",
            Value::Array(vec![event(99)]),
        );
        assert!(
            load(&mut m)
                .unwrap_err()
                .to_string()
                .contains("Boolean command")
        );
        let mut m = fixture();
        set(&mut m, "b", 2, "Flags", Value::UInt32(1 << 27));
        assert!(
            load(&mut m)
                .unwrap_err()
                .to_string()
                .contains("property listener")
        );
        let mut m = fixture();
        set(
            &mut m,
            "b",
            2,
            "InputRefProperties",
            Value::Array(vec![Value::UInt32(0)]),
        );
        assert!(
            load(&mut m)
                .unwrap_err()
                .to_string()
                .contains("reference publisher")
        );
        let mut m = fixture();
        let properties = m.0[&("b".into(), 0)]
            .fields
            .get("PropertyConnections")
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec();
        set(
            &mut m,
            "b",
            0,
            "PropertyConnections",
            Value::Array(properties.clone().into_iter().chain(properties).collect()),
        );
        // The shared graph reader can reject the duplicate row before the
        // channel reader checks its fan-in. Either rejection preserves ownership.
        assert!(load(&mut m).is_err());
    }
}
