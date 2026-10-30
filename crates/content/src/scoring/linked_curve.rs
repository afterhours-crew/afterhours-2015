// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! CurveWrapper binds an external CurveEntity through its schematic channel.
use super::{
    events::Identity,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub source: Property,
    pub input: f32,
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class == "CurveWrapperEntityData" {
            result.push(Node {
                identity: Identity::from(&node.site),
                source: identifier(&mut graph, &node.site, 0x0cb0_1792)?,
                input: super::scalar(&node.object.fields, "Input")?,
            });
        }
    }
    Ok(result)
}
