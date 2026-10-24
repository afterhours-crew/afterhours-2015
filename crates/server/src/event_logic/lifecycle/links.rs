// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resolve provider links within their live graph/channel instances. A channel
//! asset describes a schema; it is never the identity of a live provider.
use nfs_content::{
    events::lifecycle::channels::Definition,
    scoring::events::{Composition, Identity, Node},
};
use nfs_world::scoring::{Error, participant_properties::Property, properties::Port};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_INSTANCES: usize = 256;
const MAX_DEPTH: usize = 64;
const MAX_TARGETS: usize = 256;
const MAX_STEPS: usize = 4096;
/// Server linked-relay lookup, native 040558f0.
pub const RELAY_SOURCE: u32 = 0x2780_2951;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Object {
    pub instance: u64,
    pub node: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Endpoint {
    pub object: Object,
    pub port: u32,
}
#[derive(Clone, Debug)]
struct Instance {
    composition: Arc<Composition>,
    nodes: BTreeMap<Identity, u16>,
    channels: BTreeMap<u16, Definition>,
    links: BTreeMap<Port, Vec<Port>>,
}
#[derive(Clone, Debug, Default)]
pub struct Registry {
    instances: BTreeMap<u64, Instance>,
    bindings: BTreeMap<Object, Object>,
    boundaries: BTreeMap<Object, Object>,
}

impl Registry {
    /// The two ends of a loaded subworld boundary live in different graphs.
    /// Internal prefab forwarding remains part of the immutable composition.
    pub fn bind_boundary(&mut self, reference: Object, interface: Object) -> Result<(), Error> {
        let r = self.node(reference)?;
        let i = self.node(interface)?;
        if reference.instance == interface.instance
            || r.class != "SubWorldReferenceObjectData"
            || i.class != "InterfaceDescriptorData"
            || r.forward.is_some()
            || i.forward.is_some()
        {
            return Err(Error::InvalidGraph);
        }
        for (from, to) in [(reference, interface), (interface, reference)] {
            if self.boundaries.get(&from).is_some_and(|old| *old != to) {
                return Err(Error::InvalidGraph);
            }
        }
        if self.boundaries.len() >= MAX_INSTANCES * 2 && !self.boundaries.contains_key(&reference) {
            return Err(Error::Bound);
        }
        self.boundaries.insert(reference, interface);
        self.boundaries.insert(interface, reference);
        Ok(())
    }
    pub fn unbind_boundary(&mut self, object: Object) -> Result<(), Error> {
        self.node(object)?;
        if let Some(other) = self.boundaries.remove(&object) {
            self.boundaries.remove(&other);
        }
        Ok(())
    }
    pub fn install(
        &mut self,
        id: u64,
        composition: Arc<Composition>,
        channels: &BTreeMap<Identity, Definition>,
    ) -> Result<(), Error> {
        if id == 0 || self.instances.contains_key(&id) {
            return Err(Error::InvalidGraph);
        }
        if self.instances.len() >= MAX_INSTANCES
            || composition.nodes.len() > nfs_content::scoring::events::MAX_SCENE_NODES
            || composition.links.len() > nfs_world::scoring::events::MAX_CONNECTIONS
            || channels.len() > 256
        {
            return Err(Error::Bound);
        }
        let mut nodes = BTreeMap::new();
        for (index, node) in composition.nodes.iter().enumerate() {
            if nodes.insert(node.identity.clone(), index as u16).is_some()
                || node
                    .forward
                    .is_some_and(|n| usize::from(n) >= composition.nodes.len())
            {
                return Err(Error::InvalidGraph);
            }
        }
        let mut definitions = BTreeMap::new();
        for (identity, definition) in channels {
            let index = *nodes.get(identity).ok_or(Error::UnknownNode)?;
            let class = &composition.nodes[usize::from(index)].class;
            if class
                != if definition.relay {
                    "LinkedSchematicChannelRelayEntityData"
                } else if definition.reference {
                    "LinkedSchematicChannelReferenceEntityData"
                } else {
                    "LinkedSchematicChannelEntityData"
                }
                || definition.asset.is_empty()
                || (definition.relay && !definition.reference)
                || definition.asset.len() > 512
                || definition.links.contains(&0)
                || definition.links.iter().collect::<BTreeSet<_>>().len() != definition.links.len()
            {
                return Err(Error::InvalidGraph);
            }
            if definition.links.len() > 512 {
                return Err(Error::Bound);
            }
            definitions.insert(index, definition.clone());
        }
        let mut links: BTreeMap<Port, Vec<Port>> = BTreeMap::new();
        for link in &composition.links {
            if usize::from(link.source.node) >= composition.nodes.len()
                || usize::from(link.target.node) >= composition.nodes.len()
            {
                return Err(Error::UnknownNode);
            }
            links.entry(link.source).or_default().push(link.target);
        }
        self.instances.insert(
            id,
            Instance {
                composition,
                nodes,
                channels: definitions,
                links,
            },
        );
        Ok(())
    }

    pub fn object(&self, instance: u64, identity: &Identity) -> Result<Object, Error> {
        let node = *self
            .instances
            .get(&instance)
            .ok_or(Error::UnknownNode)?
            .nodes
            .get(identity)
            .ok_or(Error::UnknownNode)?;
        Ok(Object { instance, node })
    }
    pub fn node(&self, object: Object) -> Result<&Node, Error> {
        self.instances
            .get(&object.instance)
            .ok_or(Error::UnknownNode)?
            .composition
            .nodes
            .get(usize::from(object.node))
            .ok_or(Error::UnknownNode)
    }
    /// Authored channel metadata at this exact live occurrence.
    pub fn channel(&self, object: Object) -> Result<&Definition, Error> {
        self.instances
            .get(&object.instance)
            .ok_or(Error::UnknownNode)?
            .channels
            .get(&object.node)
            .ok_or(Error::InvalidGraph)
    }
    pub fn bind(&mut self, reference: Object, definition: Object) -> Result<(), Error> {
        let r = self.channel(reference)?;
        let d = self.channel(definition)?;
        if !r.reference || r.relay || d.reference || r.asset != d.asset || r.links != d.links {
            return Err(Error::InvalidGraph);
        }
        match self.bindings.get(&reference) {
            Some(old) if *old != definition => Err(Error::InvalidGraph),
            Some(_) => Ok(()),
            None => {
                self.bindings.insert(reference, definition);
                Ok(())
            }
        }
    }
    pub fn binding(&self, reference: Object) -> Option<Object> {
        self.bindings.get(&reference).copied()
    }
    /// Structural relay source. The runtime channel router retains this binding
    /// when it subscribes; this registry only resolves immutable graph topology.
    pub fn relay_source(&self, relay: Object) -> Result<Object, Error> {
        self.relay_target(relay, &mut BTreeSet::new(), &mut 0)
    }
    fn relay_target(
        &self,
        relay: Object,
        active: &mut BTreeSet<Endpoint>,
        steps: &mut usize,
    ) -> Result<Object, Error> {
        let definition = self.channel(relay)?;
        if !definition.relay {
            return Err(Error::InvalidGraph);
        }
        let endpoint = Endpoint {
            object: relay,
            port: RELAY_SOURCE,
        };
        if !active.insert(endpoint) {
            return Err(Error::InvalidGraph);
        }
        let mut targets = Vec::new();
        self.edges(endpoint, active, steps, &mut targets)?;
        active.remove(&endpoint);
        let [target] = targets.as_slice() else {
            return Err(Error::MissingInput);
        };
        let source = self.channel(*target)?;
        if source.reference || source.asset != definition.asset || source.links != definition.links
        {
            return Err(Error::InvalidGraph);
        }
        Ok(*target)
    }
    pub fn unbind(&mut self, reference: Object) -> Result<(), Error> {
        if !self.channel(reference)?.reference {
            return Err(Error::InvalidGraph);
        }
        self.bindings.remove(&reference);
        Ok(())
    }
    /// Detach dependent references first. Never leave a reference pointing at
    /// a new graph that later happens to reuse the removed instance ID.
    pub fn remove(&mut self, instance: u64) -> Result<(), Error> {
        if self
            .boundaries
            .keys()
            .any(|object| object.instance == instance)
        {
            return Err(Error::InvalidGraph);
        }
        if self
            .bindings
            .iter()
            .any(|(r, d)| d.instance == instance && r.instance != instance)
        {
            return Err(Error::InvalidGraph);
        }
        if self.instances.remove(&instance).is_none() {
            return Err(Error::UnknownNode);
        }
        self.bindings.retain(|r, _| r.instance != instance);
        Ok(())
    }

    /// Follow a channel link to real provider objects. Empty authored links
    /// remain empty; unbound references, cycles and unknown ports are errors.
    pub fn resolve(&self, endpoint: Endpoint) -> Result<Vec<Object>, Error> {
        let mut output = Vec::new();
        self.visit(endpoint, false, &mut BTreeSet::new(), &mut 0, &mut output)?;
        Ok(output)
    }
    /// Start an object-input query at a concrete consumer, preserving which
    /// channel occurrence it links to even when several share the same schema.
    pub fn input(&self, endpoint: Endpoint) -> Result<Vec<Object>, Error> {
        let mut output = Vec::new();
        self.visit(endpoint, true, &mut BTreeSet::new(), &mut 0, &mut output)?;
        Ok(output)
    }

    /// Legacy accessor keys carry a schema and port. Resolve them only within
    /// the requesting graph. Repeated references must resolve identically;
    /// otherwise the key lacks enough identity and is rejected as ambiguous.
    pub fn property(&self, instance: u64, key: &Property) -> Result<Vec<Object>, Error> {
        key.validate()?;
        let i = self.instances.get(&instance).ok_or(Error::UnknownNode)?;
        let mut result = None;
        for (&node, channel) in &i.channels {
            if !channel.reference || channel.asset != key.channel {
                continue;
            }
            let resolved = self.resolve(Endpoint {
                object: Object { instance, node },
                port: key.port,
            })?;
            if result.as_ref().is_some_and(|old| *old != resolved) {
                return Err(Error::InvalidGraph);
            }
            result = Some(resolved);
        }
        result.ok_or(Error::MissingInput)
    }

    fn visit(
        &self,
        endpoint: Endpoint,
        query: bool,
        active: &mut BTreeSet<Endpoint>,
        steps: &mut usize,
        output: &mut Vec<Object>,
    ) -> Result<(), Error> {
        *steps += 1;
        if *steps > MAX_STEPS || active.len() >= MAX_DEPTH {
            return Err(Error::Bound);
        }
        let node = self.node(endpoint.object)?;
        let forward = self.boundaries.get(&endpoint.object).copied().or_else(|| {
            node.forward.map(|node| Object {
                instance: endpoint.object.instance,
                node,
            })
        });
        if endpoint.port == 0 && !query && forward.is_none() {
            if output.len() >= MAX_TARGETS {
                return Err(Error::Bound);
            }
            output.push(endpoint.object);
            return Ok(());
        }
        if !active.insert(endpoint) {
            return Err(Error::InvalidGraph);
        }
        let i = &self.instances[&endpoint.object.instance];
        if let Some(channel) = i.channels.get(&endpoint.object.node) {
            if !channel.links.contains(&endpoint.port) {
                return Err(Error::InvalidGraph);
            }
            if channel.reference {
                let definition = if channel.relay {
                    self.relay_target(endpoint.object, active, steps)?
                } else {
                    self.binding(endpoint.object).ok_or(Error::MissingInput)?
                };
                self.visit(
                    Endpoint {
                        object: definition,
                        port: endpoint.port,
                    },
                    true,
                    active,
                    steps,
                    output,
                )?;
            } else {
                self.edges(endpoint, active, steps, output)?;
            }
        } else if query {
            self.edges(endpoint, active, steps, output)?;
        } else if let Some(next) = forward {
            self.visit(
                Endpoint {
                    object: next,
                    ..endpoint
                },
                true,
                active,
                steps,
                output,
            )?;
        } else {
            return Err(Error::InvalidGraph);
        }
        active.remove(&endpoint);
        Ok(())
    }
    fn edges(
        &self,
        endpoint: Endpoint,
        active: &mut BTreeSet<Endpoint>,
        steps: &mut usize,
        output: &mut Vec<Object>,
    ) -> Result<(), Error> {
        if let Some(targets) = self.instances[&endpoint.object.instance].links.get(&Port {
            node: endpoint.object.node,
            field: endpoint.port,
        }) {
            for target in targets {
                self.visit(
                    Endpoint {
                        object: Object {
                            instance: endpoint.object.instance,
                            node: target.node,
                        },
                        port: target.field,
                    },
                    false,
                    active,
                    steps,
                    output,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
