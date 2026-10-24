// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explicit sorter-channel binding and signed query index.
use super::{
    events::Identity,
    field,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub sorter: Property,
    pub index: i32,
}

pub fn configuration(object: &Object) -> Result<i32, Error> {
    if object.class != "IntParticipantPropertySorterQueryByIndexEntityData" {
        return Err(bad("rank query class"));
    }
    let Value::Int32(index) = field(&object.fields, "Index")? else {
        return Err(bad("rank query index"));
    };
    Ok(*index)
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class == "IntParticipantPropertySorterQueryByIndexEntityData" {
            result.push(Node {
                identity: Identity::from(&node.site),
                sorter: identifier(&mut graph, &node.site, 0x970a_d7f8)?,
                index: configuration(&node.object)?,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use nfs_frostbite::ebx::Fields;

    #[test]
    fn reads_signed_index_and_declared_sorter_channel() {
        let mut m = Memory::default();
        m.put(
            "r",
            0,
            "ParticipantLogicBlueprint",
            &[
                ("Objects", Value::Array(vec![local(1), local(2)])),
                (
                    "LinkConnections",
                    Value::Array(vec![edge(local(1), 0x970a_d7f8, local(2), 91)]),
                ),
            ],
        );
        m.put(
            "r",
            1,
            "IntParticipantPropertySorterQueryByIndexEntityData",
            &[("Index", Value::Int32(-1))],
        );
        m.put(
            "r",
            2,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[(
                "Links",
                Value::Array(vec![Value::Struct(Fields(vec![(
                    "Id".into(),
                    Value::UInt32(91),
                )]))]),
            )],
        );
        let n = read(&mut m, "r").unwrap().remove(0);
        assert_eq!(n.index, -1);
        assert_eq!(
            n.sorter,
            Property {
                channel: "c".into(),
                port: 91
            }
        );
        m.put(
            "r",
            1,
            "IntParticipantPropertySorterQueryByIndexEntityData",
            &[("Index", Value::UInt32(0))],
        );
        assert!(read(&mut m, "r").is_err());
    }
}
