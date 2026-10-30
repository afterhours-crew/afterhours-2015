// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{boolean, events::Identity, scalar};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub value: f32,
    pub enabled: bool,
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "RaceVehicleBurnoutTendencyOverrideEntityData" {
            continue;
        }
        let providers = graph.links(&node.site, 0xefe1_d0fe)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("burnout override provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicControllableProviderEntityData"
        {
            return Err(bad("burnout override own controllable provider"));
        }
        result.push(Node {
            identity: Identity::from(&node.site),
            value: scalar(&node.object.fields, "Value")?,
            enabled: boolean(&node.object.fields, "Enabled")?,
        });
    }
    Ok(result)
}
