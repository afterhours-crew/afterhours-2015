// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compose native callback owners in the same rollback boundary as the graphs.
//! The edge still owns specialized scene, sequence, spawn and network services;
//! an unhandled native call is delegated explicitly, never accepted implicitly.
use super::{
    BooleanSource, Effect, SetSource,
    accessors::{ParticipantAccessors, Source},
    channels::{Router, install::Graph},
    dispatch::Bindings,
    graphs::{Driver, Reader, Step},
    links::{Object, Registry},
    root_updates::{Resume, RootUpdates},
    set_accessors::SetAccessors,
    shared_properties::SharedProperties,
    updates::Updates,
};
use nfs_content::events::lifecycle::program::ModeSources;
use nfs_world::scoring::{
    Error,
    events::native::{Call, Request},
    program::Program,
};
use std::collections::{BTreeMap, BTreeSet};

/// Distinct IDs reserved by the caller for this owner's six components.
/// The enclosing edge must use other continuation and observer namespaces.
#[derive(Clone, Copy, Debug)]
pub struct Namespaces {
    pub channels: u32,
    pub updates: u32,
    pub shared: u32,
    pub participants: u32,
    pub sets: u32,
    pub root: u32,
}
impl Namespaces {
    fn validate(self) -> Result<(), Error> {
        let values = [
            self.channels,
            self.updates,
            self.shared,
            self.participants,
            self.sets,
            self.root,
        ];
        if values.contains(&0) || values.into_iter().collect::<BTreeSet<_>>().len() != 6 {
            return Err(Error::InvalidGraph);
        }
        Ok(())
    }
}

/// Maps exact source occurrences to stores already owned by this Host. Do not
/// manufacture these from an asset/schema name shared by several occurrences.
#[derive(Clone, Debug, Default)]
pub struct Stores {
    pub scalars: BTreeMap<Object, Source>,
    pub sets: BTreeMap<Object, SetSource>,
}

#[derive(Clone, Debug)]
pub struct Owner<E> {
    bindings: Bindings,
    links: Registry,
    channels: Router,
    shared: SharedProperties,
    updates: Updates,
    root: RootUpdates,
    participants: ParticipantAccessors,
    sets: SetAccessors,
    booleans: BTreeMap<Object, BooleanSource>,
    edge: E,
}
impl<E: Driver> Owner<E> {
    pub fn new(
        bindings: Bindings,
        links: Registry,
        stores: Stores,
        namespaces: Namespaces,
        edge: E,
    ) -> Result<Self, Error> {
        namespaces.validate()?;
        for object in stores.scalars.keys().chain(stores.sets.keys()) {
            links.node(*object)?;
        }
        let booleans = stores
            .scalars
            .iter()
            .filter_map(|(object, source)| match source {
                Source::Boolean(source) => Some((*object, source.clone())),
                _ => None,
            })
            .collect();
        Ok(Self {
            bindings,
            links,
            channels: Router::new(u64::from(namespaces.channels)),
            shared: SharedProperties::new(namespaces.shared)?,
            updates: Updates::new(u64::from(namespaces.updates))?,
            root: RootUpdates::new(u64::from(namespaces.root))?,
            participants: ParticipantAccessors::new(namespaces.participants, stores.scalars)?,
            sets: SetAccessors::new(namespaces.sets, stores.sets)?,
            booleans,
            edge,
        })
    }
    pub fn edge(&self) -> &E {
        &self.edge
    }
    pub fn links(&self) -> &Registry {
        &self.links
    }
    pub fn install_channels(&mut self, graphs: &[Graph<'_>]) -> Result<(), Error> {
        self.channels.install(graphs, &self.links)
    }
    /// Register mode services after channel topology is bound and before graph
    /// startup. Subscription order is still driven by native activation calls.
    pub fn install_mode(&mut self, sources: &ModeSources, program: &Program) -> Result<(), Error> {
        let mut next = self.clone();
        let instance = self.bindings.instance;
        let root = sources.serializers.first().ok_or(Error::InvalidGraph)?;
        next.root
            .install(self.links.object(instance, root)?, program, &self.links)?;
        next.shared.install(
            instance,
            &sources.services,
            program,
            &self.links,
            &self.booleans,
        )?;
        next.updates.install(
            instance,
            &self.bindings.plan,
            &sources.serializers,
            &sources.services,
            &self.links,
            &self.bindings,
        )?;
        *self = next;
        Ok(())
    }
}
impl<E: Driver> Driver for Owner<E> {
    fn resume(
        &mut self,
        owner: u64,
        token: u64,
        values: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.channels.resume(owner, token, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.shared.resume(owner, token, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.updates.resume(owner, token)? {
            return Ok(steps);
        }
        if let Some(resume) = self.root.resume(owner, token, &self.links)? {
            return match resume {
                Resume::Steps(steps) => Ok(steps),
                Resume::Root(request) => self.edge.invoke(request, values),
            };
        }
        self.edge.resume(owner, token, values)
    }
    fn invoke(&mut self, request: Request, values: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.root.invoke(&request, &self.links)? {
            return Ok(steps);
        }
        if let Some(steps) = self.participants.invoke(&request, &self.links, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.sets.invoke(&request, &self.links, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.channels.invoke(&request, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.shared.invoke(&request, values)? {
            return Ok(steps);
        }
        if let Some(steps) = self.updates.invoke(&request, &self.links)? {
            return Ok(steps);
        }
        if request.call == Call::Retire {
            // Updateable participant states also have base-state teardown.
            // Detach their update subscription before delegating that teardown.
            self.updates.detach(Object {
                instance: request.instance,
                node: request.node,
            })?;
        }
        if let Some(steps) = self.bindings.invoke(&request, &self.links)? {
            return Ok(steps);
        }
        self.edge.invoke(request, values)
    }
    fn effect(
        &mut self,
        effect: &Effect,
        machine: &dyn nfs_world::participant_states::Membership,
        values: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        if let Some(steps) = self.participants.effect(effect)? {
            return Ok(steps);
        }
        if let Some(steps) = self.sets.effect(effect)? {
            return Ok(steps);
        }
        if let Some(steps) = self.shared.effect(effect)? {
            return Ok(steps);
        }
        if let Some(steps) = self.bindings.effect(effect)? {
            return Ok(steps);
        }
        self.edge.effect(effect, machine, values)
    }
}

#[cfg(test)]
mod tests;
