// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! SetTeleportLocation selects its own participant and two manager links.
use super::{
    events::Identity,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub provider: Identity,
    pub location: Property,
    pub manager: Property,
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "SetTeleportLocationEntityData" {
            continue;
        }
        let providers = graph.links(&node.site, 0xb68b_0dbb)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("teleport location provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("teleport location own provider"));
        }
        result.push(Node {
            identity: Identity::from(&node.site),
            provider: Identity::from(&provider.site),
            location: identifier(&mut graph, &node.site, 0x86c9_3999)?,
            manager: identifier(&mut graph, &node.site, 0xb3a2_8b57)?,
        });
    }
    Ok(result)
}
