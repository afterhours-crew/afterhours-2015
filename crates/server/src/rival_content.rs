// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed roaming-racer definitions, ephemeral vehicle presets and the
//! road graph the host drives them on.
use crate::{Failure, vehicle_content};
use nfs_world::{items::Collection, road_graph::Graph};
use std::{path::Path, sync::Arc};

pub mod population;

#[derive(Clone, Debug)]
pub struct Content {
    data: nfs_content::rivals::Content,
    vehicles: vehicle_content::Content,
    presets: Vec<Arc<Collection>>,
    /// Required by the population: rivals are simulated on it (E987).
    graph: Option<Arc<Graph>>,
}

impl Content {
    pub fn load(
        path: &Path,
        vehicles: vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, Failure> {
        let data = nfs_content::rivals::Content::load(path).map_err(|_| Failure::ProfileConfig)?;
        Self::new(data, vehicles, items)
    }

    pub fn new(
        data: nfs_content::rivals::Content,
        vehicles: vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, Failure> {
        // Public callers can construct Content directly; apply the same closure
        // and bounds checks as the on-disk loader before retaining anything.
        let data = nfs_content::rivals::Content::from_json(&data.to_json())
            .map_err(|_| Failure::ProfileConfig)?;
        let catalog = items.inventory().ok_or(Failure::ProfileConfig)?;
        let slots = items.purchase_offers().ok_or(Failure::ProfileConfig)?;
        let presets = data
            .personas
            .iter()
            .map(|persona| {
                vehicles
                    .definition(&persona.preset.vehicle)
                    .ok_or(Failure::ProfileConfig)?;
                vehicles
                    .rival_preset(&persona.preset, catalog, slots)
                    .map(Arc::new)
                    .map_err(|_| Failure::ProfileConfig)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            data,
            vehicles,
            presets,
            graph: None,
        })
    }

    /// Attach the installed road graph the host moves inactive rivals on.
    pub fn with_graph(mut self, graph: Arc<Graph>) -> Self {
        self.graph = Some(graph);
        self
    }

    pub fn racers(&self) -> usize {
        self.data.racers.len()
    }
}
