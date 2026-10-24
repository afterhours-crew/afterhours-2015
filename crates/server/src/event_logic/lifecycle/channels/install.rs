// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Join compiled scalar/event channel handles to the resolved object topology.
//! Registration is atomic and does not activate or seed any reference.
use super::*;
use crate::event_logic::lifecycle::links::{Object, Registry};
use nfs_content::{events::lifecycle::channels::Definition, scoring::events::Identity};
use nfs_world::scoring::program::{Behavior, Program};

pub struct Graph<'a> {
    pub instance: u64,
    pub program: &'a Program,
    pub channels: &'a BTreeMap<Identity, Definition>,
}

impl Router {
    /// Definitions are registered before references, independent of graph input
    /// order. Already owned definitions may be used by a newly spawned graph.
    pub fn install(&mut self, graphs: &[Graph<'_>], links: &Registry) -> Result<(), Error> {
        if graphs.len() > 256 {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        let mut references = Vec::new();
        let mut instances = BTreeSet::new();
        for graph in graphs {
            if !instances.insert(graph.instance) {
                return Err(Error::InvalidGraph);
            }
            graph.program.validate()?;
            for (identity, definition) in graph.channels {
                let object = links.object(graph.instance, identity)?;
                if links.channel(object)? != definition {
                    return Err(Error::InvalidGraph);
                }
                let node = graph
                    .program
                    .nodes
                    .get(usize::from(object.node))
                    .ok_or(Error::UnknownNode)?;
                if node.identity.asset != identity.asset
                    || usize::try_from(node.identity.index).ok() != Some(identity.index)
                    || node.identity.instances.len() != identity.instances.len()
                    || node
                        .identity
                        .instances
                        .iter()
                        .zip(&identity.instances)
                        .any(|((a, i), (b, j))| a != b || usize::try_from(*i).ok() != Some(*j))
                    || node.class != links.node(object)?.class
                {
                    return Err(Error::InvalidGraph);
                }
                if !node.created {
                    continue;
                }
                if node.behavior != Behavior::Native {
                    return Err(Error::InvalidGraph);
                }
                let location = Location {
                    instance: object.instance,
                    node: object.node,
                };
                let bound = |field| {
                    graph.program.cells.iter().any(|cell| {
                        cell.ports.contains(&Port {
                            node: object.node,
                            field,
                        })
                    })
                };
                if definition.reference {
                    let source = if definition.relay {
                        links.relay_source(object)?
                    } else {
                        links.binding(object).ok_or(Error::MissingInput)?
                    };
                    let fields = definition
                        .properties
                        .iter()
                        .chain(&definition.reference_properties)
                        .copied()
                        .filter(|field| bound(*field))
                        .collect();
                    references.push((location, source, fields));
                } else {
                    next.definition(
                        location,
                        definition
                            .properties
                            .iter()
                            .chain(&definition.reference_properties)
                            .copied()
                            .filter(|f| bound(*f))
                            .collect(),
                    )?;
                }
            }
        }
        for (reference, Object { instance, node }, fields) in references {
            next.reference(reference, Location { instance, node }, fields)?;
        }
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
