// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The server tuning query's participant and vehicle item manager links.
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
    pub source: Property,
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    read_class(source, blueprint, "VehicleTuningQueryEntityData")
}

pub(crate) fn read_class<S: Source>(
    source: &mut S,
    blueprint: &str,
    class: &str,
) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != class {
            continue;
        }
        let providers = graph.links(&node.site, 0xb68b_0dbb)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("vehicle tuning provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("vehicle tuning own provider"));
        }
        result.push(Node {
            identity: Identity::from(&node.site),
            provider: Identity::from(&provider.site),
            source: identifier(&mut graph, &node.site, 0xbb98_36ab)?,
        });
    }
    Ok(result)
}
