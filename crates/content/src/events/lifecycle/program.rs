// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The ordinary graph and its explicitly host-owned lifecycle entities share
//! one composition. Their property cells and callback order are not duplicated.
use crate::{
    Error,
    assets::AssetIndex,
    events::GameMode,
    graph::bad,
    scoring::{events::Identity, program::Sources},
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct ModeSources {
    pub sources: Sources,
    pub serializers: Vec<Identity>,
    pub native: BTreeSet<Identity>,
    pub services: std::collections::BTreeMap<Identity, super::shared::Service>,
    pub channels: std::collections::BTreeMap<Identity, super::channels::Definition>,
}

impl AssetIndex {
    pub fn event_mode_sources(&mut self, mode: &GameMode) -> Result<ModeSources, Error> {
        let serializers = self.event_serializer_identities(&mode.layout, "GameModeEntityData")?;
        let mut native = BTreeSet::from([serializers
            .first()
            .ok_or_else(|| bad("game mode root"))?
            .clone()]);
        for (field, identity) in mode.layout.fields.iter().zip(&serializers) {
            if field.binding.is_some() {
                native.insert(identity.clone());
            }
        }
        for serializer in mode
            .lifecycle
            .states
            .iter()
            .map(|s| s.serializer)
            .chain(mode.lifecycle.transitions.iter().map(|t| t.serializer))
        {
            native.insert(
                serializers
                    .get(serializer)
                    .ok_or_else(|| bad("lifecycle serializer"))?
                    .clone(),
            );
        }
        let b = &mode.lifecycle.bindings;
        native.extend(b.set_counters.iter().map(|n| n.identity.clone()));
        native.extend(b.rank_queries.iter().map(|n| n.identity.clone()));
        native.extend(b.properties.iter().map(|n| n.identity.clone()));
        native.extend(b.contexts.iter().map(|n| n.identity.clone()));
        native.extend(b.reporters.iter().map(|n| n.identity.clone()));
        native.extend(b.integers.iter().cloned());
        native.extend(b.mirror_managers.iter().cloned());
        native.extend(b.mirrors.iter().map(|n| n.identity.clone()));
        native.extend(b.mirror_providers.iter().map(|n| n.identity.clone()));
        native.extend(b.distances.iter().map(|n| n.identity.clone()));
        native.extend(b.sorters.iter().map(|n| n.identity.clone()));
        native.extend(b.results.iter().map(|n| n.identity.clone()));
        native.extend(b.spawners.iter().map(|n| n.identity.clone()));
        native.extend(b.sets.iter().map(|n| n.identity.clone()));
        native.extend(b.predicates.iter().map(|n| n.identity.clone()));
        native.extend(b.update_managers.iter().map(|n| n.identity.clone()));
        native.extend(b.ai_controls.iter().map(|n| n.identity.clone()));
        native.extend(b.ai_commands.keys().cloned());
        let services = super::shared::read(&mut self.assets, &mode.layout.blueprint)?;
        native.extend(services.keys().cloned());
        let channels = super::channels::definitions(&mut self.assets, &mode.layout.blueprint)?;
        native.extend(channels.keys().cloned());
        let sources = self.scoring_sources_with_native(&mode.layout.blueprint, &native)?;
        for identity in &native {
            if !sources
                .composition
                .nodes
                .iter()
                .any(|n| n.identity == *identity)
            {
                return Err(bad("native lifecycle entity outside mode composition"));
            }
        }
        Ok(ModeSources {
            sources,
            serializers,
            native,
            services,
            channels,
        })
    }
}
