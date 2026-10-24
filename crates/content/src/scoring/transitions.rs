// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-logic transitions bind ordered sources and their own provider.
use super::{
    events::Identity,
    participant_properties::{Property, channel_endpoint},
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    /// None is the generic triggered transition, using ParticipantEvent input.
    pub provider: Option<Identity>,
    pub sources: Vec<Property>,
    pub target: Option<Property>,
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if !matches!(
            node.object.class.as_str(),
            "ParticipantLogicParticipantTransitionEntityData"
                | "TriggeredParticipantTransitionEntityData"
        ) {
            continue;
        }
        let provider = if node.object.class == "ParticipantLogicParticipantTransitionEntityData" {
            let providers = graph.links(&node.site, 0xb68b_0dbb)?;
            let [provider] = providers.as_slice() else {
                return Err(bad("logic transition provider"));
            };
            if provider.port != 0
                || graph.object(&provider.site)?.class
                    != "ParticipantLogicParticipantProviderEntityData"
            {
                return Err(bad("logic transition own provider"));
            }
            Some(Identity::from(&provider.site))
        } else {
            None
        };
        let sources = graph.links(&node.site, 0xf02f_c8dc)?;
        if sources.len() > nfs_world::participant_states::MAX_STATES {
            return Err(bad("logic transition sources bound"));
        }
        let sources = sources
            .iter()
            .map(|e| channel_endpoint(&mut graph, e))
            .collect::<Result<Vec<_>, _>>()?;
        let targets = graph.links(&node.site, 0x856e_2e03)?;
        let target = match targets.as_slice() {
            [] => None,
            [target] => Some(channel_endpoint(&mut graph, target)?),
            _ => return Err(bad("logic transition target count")),
        };
        result.push(Node {
            identity: Identity::from(&node.site),
            provider,
            sources,
            target,
        });
    }
    Ok(result)
}
