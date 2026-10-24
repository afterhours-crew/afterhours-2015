// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read the participant-context category query used by music/restart logic.
use super::{
    boolean,
    events::Identity,
    field,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad, bits32},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub provider: Identity,
    pub source: Property,
    pub categories: Vec<u32>,
    pub active: bool,
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "ParticipantContextReporterEntityData" {
            continue;
        }
        let providers = graph.links(&node.site, 0xb68b_0dbb)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("context reporter provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("context reporter own provider"));
        }
        // Native 04782cb0 gives the linked category-list precedence. Normal
        // event logics use the authored list; refuse an unresolved override.
        if !graph.links(&node.site, 0x0683_3b36)?.is_empty() {
            return Err(bad("context reporter linked category list unsupported"));
        }
        let rows = field(&node.object.fields, "Categories")?
            .as_array()
            .filter(|r| r.len() <= 128)
            .ok_or_else(|| bad("context reporter category bound"))?;
        let mut categories = Vec::new();
        for row in rows {
            let key = graph
                .source
                .target(
                    &node.site.key.0,
                    row.as_pointer()
                        .ok_or_else(|| bad("context reporter category pointer"))?,
                )?
                .ok_or_else(|| bad("context reporter null category"))?;
            let category = graph.source.object(&key)?;
            if category.class != "GameModeCategory" {
                return Err(bad("context reporter category class"));
            }
            categories.push(bits32(field(&category.fields, "CategoryHash")?)?);
        }
        let _ = field(&node.object.fields, "Id")?
            .as_pointer()
            .ok_or_else(|| bad("context reporter ID pointer"))?;
        result.push(Node {
            identity: Identity::from(&node.site),
            provider: Identity::from(&provider.site),
            source: identifier(&mut graph, &node.site, 0xb611_c196)?,
            categories,
            active: boolean(&node.object.fields, "Active")?,
        });
    }
    Ok(result)
}
