// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ResultEntity's authored score-tag/property links. Link names hash the asset
//! Name, while the result map retains ScoreTag asset identity and TagHash.
use super::*;

pub const TRIGGER: u32 = 0x4ab4_7b0b;
pub const MAX_TAGS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Tag {
    pub asset: String,
    pub object: usize,
    pub name: String,
    pub hash: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Definition {
    pub identity: Identity,
    /// Authored order, including repeated tags. Unconnected sources contribute
    /// no rows, rather than zero scores for the event's other participants.
    pub scores: Vec<(Tag, Option<IntegerSource>)>,
    pub collector: Link,
}

pub(super) fn read<S: Source>(graph: &mut Graph<S>, node: &Node) -> Result<Definition, Error> {
    let tags = node
        .object
        .fields
        .get("ScoreTags")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= MAX_TAGS)
        .ok_or_else(|| bad("result score tags"))?;
    let mut scores = Vec::new();
    for tag in tags {
        let key = graph
            .source
            .target(
                &node.site.key.0,
                tag.as_pointer().ok_or_else(|| bad("score tag pointer"))?,
            )?
            .ok_or_else(|| bad("null score tag"))?;
        let object = graph.source.object(&key)?;
        if object.class != "ScoreTag" {
            return Err(bad("score tag class"));
        }
        let name = object
            .fields
            .get("Name")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 512 && !s.contains('\0'))
            .ok_or_else(|| bad("score tag name"))?;
        let source = integer_source(graph, &node.site, crate::events::values::name(name))?;
        scores.push((
            Tag {
                asset: key.0,
                object: key.1,
                name: name.into(),
                hash: bits32(
                    object
                        .fields
                        .get("TagHash")
                        .ok_or_else(|| bad("score tag hash"))?,
                )?,
            },
            source,
        ));
    }
    let collector = controller(graph, &node.site, 0x1ac1_d96f)?
        .ok_or_else(|| bad("result collector missing"))?;
    if !(collector.port == 0 && collector.class == "ResultCollectorEntityData"
        || collector.channel.is_some())
    {
        return Err(bad("result collector class"));
    }
    Ok(Definition {
        identity: Identity::from(&node.site),
        scores,
        collector,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};

    #[test]
    fn tag_asset_name_selects_link_while_tag_hash_and_missing_rows_are_preserved() {
        let mut m = Memory::default();
        m.put(
            "mode",
            0,
            "GameModeBlueprint",
            &[
                ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(
                            local(1),
                            crate::events::values::name("Score/Time"),
                            local(2),
                            0,
                        ),
                        edge(local(1), 0x1ac1_d96f, local(3), 0),
                    ]),
                ),
            ],
        );
        m.put(
            "mode",
            1,
            "ResultEntityData",
            &[("ScoreTags", Value::Array(vec![ptr("t", 0), ptr("u", 0)]))],
        );
        m.put("mode", 2, "IntParticipantPropertyEntityData", &[]);
        m.put("mode", 3, "ResultCollectorEntityData", &[]);
        for (asset, name, hash) in [("t", "Score/Time", 55), ("u", "Score/Other", 77)] {
            m.put(
                asset,
                0,
                "ScoreTag",
                &[
                    ("Name", Value::String(name.into())),
                    ("TagHash", Value::UInt32(hash)),
                ],
            );
        }
        let mut g = Graph::new(&mut m);
        let nodes = g.read(Site::root("mode")).unwrap();
        let result = read(
            &mut g,
            nodes
                .iter()
                .find(|n| n.object.class == "ResultEntityData")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result.scores[0].0.hash, 55);
        assert_eq!(result.scores[0].0.name, "Score/Time");
        assert!(result.scores[0].1.is_some());
        assert!(result.scores[1].1.is_none());
        assert_eq!(result.collector.class, "ResultCollectorEntityData");
    }
}
