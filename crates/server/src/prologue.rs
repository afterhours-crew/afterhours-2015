// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed story inputs shared by isolated player sessions. Account inventory
//! is never the storage for the temporary first-drive vehicle.
use crate::{
    sequence_content::authored::Authored,
    vehicle_content::{
        GarageContent,
        population::{PreviewCars, TemporaryVehicle},
    },
};
use nfs_content::{
    assets::AssetIndex,
    prologue::{Start, sequences},
};
use nfs_world::items::InventoryCatalog;
use nfs_world::{
    items::Collection,
    replication::{Error, entity::creation::Asset},
};
use std::sync::Arc;
pub mod driving;
pub mod loading;
pub mod movies;

#[derive(Clone, Debug)]
pub struct Content {
    pub start: Start,
    pub opening: Authored,
    pub driving: Option<(driving::Mode, driving::Mode)>,
    pub previews: Option<Arc<PreviewCars>>,
    pub loading: Option<loading::Loading>,
    pub selection: Option<Authored>,
    pub selection_program: Option<Arc<nfs_content::prologue::sequences::program::Definition>>,
    pub first_race: Option<Race>,
    pub second_race: Option<Race>,
    pub manu_movie: Option<movies::Content>,
    pub diner_movie: Option<movies::Content>,
    items: Arc<Collection>,
}
#[derive(Clone, Debug)]
pub struct Race {
    pub destination: nfs_content::prologue::Destination,
    pub arrival: Authored,
    pub mode: driving::Mode,
}
impl Race {
    fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        capability: &str,
        conditional: bool,
    ) -> Result<Self, nfs_content::Error> {
        let destination = if conditional {
            nfs_content::prologue::destination_before_launcher(index, capability)?
        } else {
            nfs_content::prologue::destination(index, capability)?
        };
        if sequences::timing::extract(index, &destination.opening_blueprint)? != Some(0.) {
            return Err(nfs_content::Error::Content(
                "intro race arrival timing".into(),
            ));
        }
        let arrival = Authored::new(
            sequences::extract_referenced(
                index,
                &destination.opening_blueprint,
                &destination.reference,
            )?,
            parent,
            vec![],
        )
        .map_err(|e| nfs_content::Error::Content(format!("race arrival: {e:?}")))?;
        Ok(Self {
            destination,
            arrival,
            mode: driving::Mode::prepare_from(
                index,
                parent,
                "gameplay/progression/intro/introracegamemode",
                Some(capability),
            )?,
        })
    }
}
impl Content {
    /// Resolve the authored story grid using the same prepared vehicle/item
    /// content as ordinary events. Missing opponents disable preparation.
    pub fn with_opponents(
        mut self,
        index: &mut AssetIndex,
        vehicles: crate::vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, nfs_content::Error> {
        for (race, capability) in [
            (
                &mut self.first_race,
                "gameplay/progression/intro/introracespikeandamy",
            ),
            (
                &mut self.second_race,
                "gameplay/progression/intro/intromeetandracemanu",
            ),
        ] {
            let race = race
                .as_mut()
                .ok_or_else(|| nfs_content::Error::Content("intro race missing".into()))?;
            let roster = nfs_content::events::intro::roster(
                index,
                &race.mode.data.game_mode.layout.blueprint,
                capability,
            )?;
            race.mode = race
                .mode
                .clone()
                .with_opponents(roster, vehicles.clone(), items)
                .map_err(|e| nfs_content::Error::Content(format!("intro opponents: {e:?}")))?;
        }
        Ok(self)
    }
    pub fn mode(
        &self,
        chapter: nfs_world::participants::intro::Chapter,
    ) -> Result<&driving::Mode, Error> {
        use nfs_world::participants::intro::Chapter;
        match chapter {
            Chapter::FirstDrive => self.driving.as_ref().map(|pair| &pair.0),
            Chapter::FirstRace => self.first_race.as_ref().map(|race| &race.mode),
            Chapter::SecondRace => self.second_race.as_ref().map(|race| &race.mode),
        }
        .ok_or(Error::Unsupported)
    }
    pub fn arrival(
        &self,
        chapter: nfs_world::participants::intro::Chapter,
    ) -> Result<&Authored, Error> {
        use nfs_world::participants::intro::Chapter;
        match chapter {
            Chapter::FirstDrive => Ok(&self.opening),
            Chapter::FirstRace => self
                .first_race
                .as_ref()
                .map(|race| &race.arrival)
                .ok_or(Error::Unsupported),
            Chapter::SecondRace => self
                .second_race
                .as_ref()
                .map(|race| &race.arrival)
                .ok_or(Error::Unsupported),
        }
    }
    #[cfg(test)]
    pub(crate) fn for_test(
        items: Arc<Collection>,
        parent: Asset,
        manager: nfs_world::sequences::Component,
    ) -> Self {
        use nfs_world::{replication::entity::Kind, scoring::event_sync::Binding};
        let sequence = sequences::Sequence {
            blueprint: "test/opening".into(),
            asset: Asset {
                bundle: 30,
                type_id: 9,
                local_index: 0,
            },
            catalog: vec![(8, 1), (9, 1)],
            manager,
            embedded_bus: 2,
            bus_count: 1,
            fields: vec![Kind::SequenceRoot, Kind::RpcReference],
            channels: std::collections::BTreeMap::from([(
                1,
                nfs_content::participant_logic::Channel::Participant { serializer: 1 },
            )]),
        };
        Self {
            start: Start {
                objective: 42,
                vehicle_item: items.items[&1].definition,
                opening_blueprint: sequence.blueprint.clone(),
                opening_guid: [0; 16],
                teleport: nfs_content::prologue::Pose {
                    position: [10., 2., 30.],
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                },
            },
            opening: Authored::new(
                sequence,
                parent,
                vec![Binding {
                    entity: 1,
                    event: 37,
                    node: 0,
                }],
            )
            .unwrap(),
            items,
            driving: Some(driving::tests::content(parent, manager)),
            previews: None,
            loading: None,
            selection: None,
            selection_program: None,
            first_race: None,
            second_race: None,
            manu_movie: None,
            diner_movie: None,
        }
    }
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        items: &InventoryCatalog,
    ) -> Result<Self, nfs_content::Error> {
        let start = nfs_content::prologue::extract(index)?;
        let sequence = sequences::extract(index, &start.opening_blueprint)?;
        let stops = sequences::direct_stop_signals(index, &start.opening_blueprint)?;
        if stops.is_empty() {
            return Err(nfs_content::Error::Content(
                "opening has no authored completion signal".into(),
            ));
        }
        let opening = Authored::new(sequence, parent, stops)
            .map_err(|e| nfs_content::Error::Content(format!("opening construction: {e:?}")))?;
        let name = "gameplay/progression/intro/introcarselectsequence";
        let selection = Authored::new(
            sequences::extract(index, name)?,
            parent,
            sequences::direct_stop_signals(index, name)?,
        )
        .map_err(|e| nfs_content::Error::Content(format!("starter selection: {e:?}")))?;
        let loading = loading::Loading::prepare(index)?;
        let selection_program = nfs_content::prologue::sequences::program::extract(index, name)?;
        let capability = "gameplay/progression/intro/introracespikeandamy";
        let first_race = Some(Race::prepare(index, parent, capability, false)?);
        let second_race = Some(Race::prepare(
            index,
            parent,
            "gameplay/progression/intro/intromeetandracemanu",
            true,
        )?);
        let manu_movie = Some(movies::Content::prepare(
            index,
            parent,
            "gameplay/progression/intro/intromeetandracemanu",
        )?);
        let driving = Some((
            driving::Mode::prepare(
                index,
                parent,
                "gameplay/progression/intro/introdrivetoclubgamemode",
            )?,
            driving::Mode::prepare(
                index,
                parent,
                "gameplay/progression/intro/evencancellergamemode",
            )?,
        ));
        let mut built = items
            .instantiate_vehicle(start.vehicle_item, 1)
            .map_err(|e| nfs_content::Error::Content(format!("intro vehicle: {e}")))?;
        // SubItems belong to the ephemeral vehicle; optional AdditionalItems
        // are not an account grant or equipped parts.
        built.collection.roots = built.collection.items.keys().copied().collect();
        let garage = nfs_content::garage::starter::extract(index)?;
        let mut previews = vec![];
        for slot in &garage.slots {
            let mut car = items
                .instantiate_vehicle(slot.definition, 1)
                .map_err(|e| nfs_content::Error::Content(format!("starter preview: {e}")))?;
            car.collection.roots = car.collection.items.keys().copied().collect();
            previews.push(Arc::new(car.collection));
        }
        Ok(Self {
            start,
            opening,
            driving,
            loading: Some(loading),
            selection: Some(selection),
            selection_program: Some(Arc::new(selection_program)),
            first_race,
            second_race,
            manu_movie,
            diner_movie: Some(movies::Content::prepare(
                index,
                parent,
                "gameplay/progression/intro/introreturntogarage",
            )?),
            items: Arc::new(built.collection),
            previews: Some(Arc::new(PreviewCars {
                garage,
                items: previews
                    .try_into()
                    .map_err(|_| nfs_content::Error::Content("starter count".into()))?,
            })),
        })
    }
    pub fn active(&self, restored: &crate::progression::Restored) -> bool {
        restored
            .branch(crate::progression::Branch::Intro)
            .objectives()
            .get(&self.start.objective)
            .is_some_and(|o| o.active && !o.completed)
    }
    pub fn temporary_vehicle(&self, content: &GarageContent) -> Result<TemporaryVehicle, Error> {
        let definition = content
            .vehicles
            .definition(&self.start.vehicle_item)
            .ok_or(Error::Unsupported)?;
        let (position, basis) = match content.layout.world_spawn() {
            Some(pose) => (pose.locator(), pose.basis()),
            None => {
                let slot = content.layout.slot(0).ok_or(Error::Shape)?;
                (
                    definition
                        .resting
                        .position_at(slot.locator())?
                        .map(|v| v.map_or(0., f32::from_bits)),
                    slot.basis(),
                )
            }
        };
        Ok(TemporaryVehicle {
            items: self.items.clone(),
            vehicle: 1,
            position,
            basis,
            blueprint_scene_key: content.layout.item_scene_key,
        })
    }
}
