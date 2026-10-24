// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::{events::Identity, scalar};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
use nfs_world::scoring::burnout::Configuration;
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub config: Configuration,
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "RaceVehicleBurnoutQueryEntityData" {
            continue;
        }
        let providers = graph.links(&node.site, 0xefe1_d0fe)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("burnout query provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicControllableProviderEntityData"
        {
            return Err(bad("burnout query own provider"));
        }
        let f = &node.object.fields;
        result.push(Node {
            identity: Identity::from(&node.site),
            config: Configuration {
                max_linear_mph: scalar(f, "MaxLinearVelocityMph")?,
                max_angular: scalar(f, "MaxAngularVelocity")?,
                min_throttle: scalar(f, "MinimumThrottle")?,
                min_time: scalar(f, "MinimumBurnoutTime")?,
                min_slip_front: scalar(f, "MinimumTireSlipRatioFront")?,
                min_slip_rear: scalar(f, "MinimumTireSlipRatioRear")?,
            },
        });
    }
    Ok(result)
}
