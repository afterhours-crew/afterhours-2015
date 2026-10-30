// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-set links retain local node versus channel identity.
use super::{
    events::Identity,
    field,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source as AssetSource, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::participant_sets::{MAX_OPERANDS, Operation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    Channel(Property),
    Node(Identity),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    Adapter {
        source: Option<Property>,
        provider: Option<Identity>,
    },
    Predicate {
        operation: Operation,
        operands: Vec<Source>,
    },
    Counter {
        source: Option<Source>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub kind: Kind,
}

pub fn predicate(object: &Object) -> Result<(Operation, usize), Error> {
    let Value::Int32(count) = field(&object.fields, "NumberOfOperands")? else {
        return Err(bad("set operand count"));
    };
    let count = usize::try_from(*count).map_err(|_| bad("set operand count"))?;
    let operation = match field(&object.fields, "Operation")? {
        Value::Enum {
            value: 0,
            name: Some(n),
        } if n == "UNION" => Operation::Union,
        Value::Enum {
            value: 1,
            name: Some(n),
        } if n == "INTERSECTION" => Operation::Intersection,
        Value::Enum {
            value: 2,
            name: Some(n),
        } if n == "DIF" => Operation::Difference,
        _ => return Err(bad("set operation")),
    };
    if count > MAX_OPERANDS || (operation == Operation::Difference && count != 2) {
        return Err(bad("set operand bound"));
    }
    Ok((operation, count))
}

fn source<S: AssetSource>(
    g: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<Option<Source>, Error> {
    let links = g.links(site, port)?;
    match links.as_slice() {
        [] => Ok(None),
        [e] => match (g.object(&e.site)?.class.as_str(), e.port) {
            ("LinkedSchematicChannelReferenceEntityData", p) if p != 0 => {
                Ok(Some(Source::Channel(identifier(g, site, port)?)))
            }
            ("ParticipantSetAdapterEntityData" | "ParticipantSetPredicateEntityData", 0) => {
                Ok(Some(Source::Node(Identity::from(&e.site))))
            }
            _ => Err(bad("set source endpoint")),
        },
        _ => Err(bad("set source count")),
    }
}

pub(crate) fn read<S: AssetSource>(
    source_assets: &mut S,
    blueprint: &str,
) -> Result<Vec<Node>, Error> {
    let mut g = Graph::new(source_assets);
    let mut result = Vec::new();
    for n in g.read(Site::root(blueprint))? {
        let kind = match n.object.class.as_str() {
            "ParticipantSetAdapterEntityData" => {
                let source = if g.links(&n.site, 0x4012_c55a)?.is_empty() {
                    None
                } else {
                    Some(identifier(&mut g, &n.site, 0x4012_c55a)?)
                };
                let provider = match g.links(&n.site, 0xb68b_0dbb)?.as_slice() {
                    [] => None,
                    [e] if e.port == 0
                        && g.object(&e.site)?.class
                            == "ParticipantLogicParticipantProviderEntityData" =>
                    {
                        Some(Identity::from(&e.site))
                    }
                    _ => return Err(bad("set adapter participant provider")),
                };
                Kind::Adapter { source, provider }
            }
            "ParticipantSetCounterEntityData" => Kind::Counter {
                source: source(&mut g, &n.site, 0x1224_af09)?,
            },
            "ParticipantSetPredicateEntityData" => {
                let (operation, count) = predicate(&n.object)?;
                let mut operands = Vec::new();
                for i in 0..count {
                    let port = nfs_world::scoring::integers::port("", i as u16);
                    operands.push(
                        source(&mut g, &n.site, port)?.ok_or_else(|| bad("set operand missing"))?,
                    );
                }
                Kind::Predicate {
                    operation,
                    operands,
                }
            }
            _ => continue,
        };
        result.push(Node {
            identity: Identity::from(&n.site),
            kind,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use nfs_frostbite::ebx::Fields;
    #[test]
    fn local_adapter_and_predicate_links_are_not_mistaken_for_channel_ports() {
        let mut m = Memory::default();
        m.put(
            "r",
            0,
            "ParticipantLogicBlueprint",
            &[
                ("Objects", Value::Array((1..=5).map(local).collect())),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(1), 0x4012_c55a, local(5), 17),
                        edge(local(1), 0xb68b_0dbb, local(4), 0),
                        edge(
                            local(2),
                            nfs_world::scoring::integers::port("", 0),
                            local(1),
                            0,
                        ),
                        edge(
                            local(2),
                            nfs_world::scoring::integers::port("", 1),
                            local(5),
                            18,
                        ),
                        edge(local(3), 0x1224_af09, local(2), 0),
                    ]),
                ),
            ],
        );
        m.put("r", 1, "ParticipantSetAdapterEntityData", &[]);
        m.put(
            "r",
            2,
            "ParticipantSetPredicateEntityData",
            &[
                ("NumberOfOperands", Value::Int32(2)),
                (
                    "Operation",
                    Value::Enum {
                        value: 1,
                        name: Some("INTERSECTION".into()),
                    },
                ),
            ],
        );
        m.put("r", 3, "ParticipantSetCounterEntityData", &[]);
        m.put("r", 4, "ParticipantLogicParticipantProviderEntityData", &[]);
        m.put(
            "r",
            5,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[(
                "Links",
                Value::Array(
                    [17, 18]
                        .into_iter()
                        .map(|id| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(id))])))
                        .collect(),
                ),
            )],
        );
        let nodes = read(&mut m, "r").unwrap();
        assert_eq!(nodes.len(), 3);
        assert!(
            matches!(&nodes[0].kind,Kind::Adapter { source:Some(p),provider:Some(id) } if p.port==17 && id.index==4)
        );
        assert!(
            matches!(&nodes[1].kind,Kind::Predicate { operands,.. } if matches!(&operands[0],Source::Node(id) if id.index==1) && matches!(&operands[1],Source::Channel(p) if p.port==18))
        );
        assert!(
            matches!(&nodes[2].kind,Kind::Counter { source:Some(Source::Node(id)) } if id.index==2)
        );
    }
}
