// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::events::Identity;
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Identity>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "RaceVehicleInfoQueryEntityData" {
            continue;
        }
        let providers = graph.links(&node.site, 0xefe1_d0fe)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("vehicle info provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicControllableProviderEntityData"
        {
            return Err(bad("vehicle info own provider"));
        }
        result.push(Identity::from(&node.site));
    }
    Ok(result)
}
