// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! EventDetailsInfoAccessor binds its explicit channel at native 04598ca0.
use super::{
    events::Identity,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source},
};

pub(crate) fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<Vec<(Identity, Option<Property>)>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class == "EventDetailsInfoAccessorEntityData" {
            let source = if graph.links(&node.site, 0x746b_874b)?.is_empty() {
                None
            } else {
                Some(identifier(&mut graph, &node.site, 0x746b_874b)?)
            };
            result.push((Identity::from(&node.site), source));
        }
    }
    Ok(result)
}
