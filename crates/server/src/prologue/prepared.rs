// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Complete static story compiler product. Recreate inventories, codecs,
//! lifecycle plans and reporter state through their ordinary constructors.
use super::*;
use crate::{prepared_content::Cache, vehicle_content};
use nfs_content::{Error as ContentError, events::ai::Roster, garage::starter::Garage};
use nfs_services::item_content::ItemContent;
use std::collections::BTreeSet;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    start: Start,
    opening: Box<Authored>,
    driving: (Box<driving::Mode>, Box<driving::Mode>),
    loading: Box<loading::Loading>,
    membership: Box<membership::Content>,
    selection: Box<Authored>,
    selection_program: Box<nfs_content::prologue::sequences::program::Definition>,
    first_race: Box<Race>,
    second_race: Box<Race>,
    manu_movie: Box<movies::Content>,
    diner_movie: Box<movies::Content>,
    return_drive: Box<driving::Mode>,
    final_composite: Box<composite::Content>,
    volumes: volumes::WorldVolumes,
    return_events: Vec<(u32, u32)>,
    mode_notifications: Vec<(u32, nfs_content::events::intro::ModeNotification)>,
    garage: Garage,
    rosters: [Roster; 2],
}
fn required<T>(value: Option<T>) -> Result<T, ContentError> {
    value.ok_or(ContentError::Cache("incomplete prepared prologue"))
}
impl Snapshot {
    fn build(
        index: &mut AssetIndex,
        parent: Asset,
        items: &InventoryCatalog,
    ) -> Result<Self, ContentError> {
        tracing::info!("compiling complete prologue cache");
        let v = Content::prepare(index, parent, items)?;
        let first_race = required(v.first_race)?;
        let second_race = required(v.second_race)?;
        let rosters = [
            nfs_content::events::intro::roster(
                index,
                &first_race.mode.data.game_mode.layout.blueprint,
                "gameplay/progression/intro/introracespikeandamy",
            )?,
            nfs_content::events::intro::roster(
                index,
                &second_race.mode.data.game_mode.layout.blueprint,
                "gameplay/progression/intro/intromeetandracemanu",
            )?,
        ];
        Ok(Self {
            start: v.start,
            opening: Box::new(v.opening),
            driving: {
                let (a, b) = required(v.driving)?;
                (Box::new(a), Box::new(b))
            },
            loading: Box::new(required(v.loading)?),
            membership: Box::new(required(v.membership)?),
            selection: Box::new(required(v.selection)?),
            selection_program: Box::new(required(v.selection_program)?.as_ref().clone()),
            first_race: Box::new(first_race),
            second_race: Box::new(second_race),
            manu_movie: Box::new(required(v.manu_movie)?),
            diner_movie: Box::new(required(v.diner_movie)?),
            return_drive: Box::new(required(v.return_drive)?),
            final_composite: Box::new(required(v.final_composite)?),
            volumes: v.volumes,
            return_events: v.return_events,
            mode_notifications: v.mode_notifications,
            garage: required(v.previews)?.garage.clone(),
            rosters,
        })
    }

    fn finish(
        self,
        items: &InventoryCatalog,
        vehicles: &vehicle_content::Content,
        ai_items: &ItemContent,
    ) -> Result<Content, ContentError> {
        self.validate()?;
        let instantiate = |definition| -> Result<Arc<Collection>, ContentError> {
            let mut built = items
                .instantiate_vehicle(definition, 1)
                .map_err(|e| ContentError::Content(format!("prepared intro vehicle: {e}")))?;
            built.collection.roots = built.collection.items.keys().copied().collect();
            Ok(Arc::new(built.collection))
        };
        let temporary = instantiate(self.start.vehicle_item)?;
        let previews = self
            .garage
            .slots
            .iter()
            .map(|slot| instantiate(slot.definition))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| ContentError::Cache("prepared starter count"))?;
        let [first, second] = self.rosters;
        let mut first_race = *self.first_race;
        let mut second_race = *self.second_race;
        first_race.mode = first_race
            .mode
            .with_opponents(first, vehicles.clone(), ai_items)
            .map_err(|e| ContentError::Content(format!("prepared first race: {e:?}")))?;
        second_race.mode = second_race
            .mode
            .with_opponents(second, vehicles.clone(), ai_items)
            .map_err(|e| ContentError::Content(format!("prepared second race: {e:?}")))?;
        Ok(Content {
            start: self.start,
            opening: *self.opening,
            driving: Some((*self.driving.0, *self.driving.1)),
            loading: Some(*self.loading),
            membership: Some(*self.membership),
            selection: Some(*self.selection),
            selection_program: Some(Arc::new(*self.selection_program)),
            first_race: Some(first_race),
            second_race: Some(second_race),
            manu_movie: Some(*self.manu_movie),
            diner_movie: Some(*self.diner_movie),
            return_drive: Some(*self.return_drive),
            final_composite: Some(*self.final_composite),
            volumes: self.volumes,
            return_events: self.return_events,
            mode_notifications: self.mode_notifications,
            items: temporary,
            previews: Some(Arc::new(PreviewCars {
                garage: self.garage,
                items: previews,
            })),
        })
    }

    fn validate(&self) -> Result<(), ContentError> {
        let bad = || ContentError::Cache("prepared prologue invariants");
        for pose in [
            &self.start.teleport,
            &self.first_race.destination.teleport,
            &self.second_race.destination.teleport,
        ] {
            if pose
                .position
                .iter()
                .chain(pose.basis.iter().flatten())
                .any(|v| !v.is_finite())
            {
                return Err(bad());
            }
        }
        if self.start.objective == 0
            || self.start.vehicle_item == [0; 16]
            || self
                .garage
                .slots
                .iter()
                .map(|s| s.definition)
                .collect::<BTreeSet<_>>()
                .len()
                != 3
            || self.garage.slots.iter().any(|s| {
                s.definition == [0; 16]
                    || s.transform.iter().any(|v| !f32::from_bits(*v).is_finite())
            })
            || self.return_events.len() > 4096
            || self.mode_notifications.len() > 4096
        {
            return Err(bad());
        }
        let v = &self.volumes;
        // A degraded empty-volume fallback must never become a successful cache.
        if v.events.is_empty()
            || v.activities.is_empty()
            || [
                &v.events,
                &v.activities,
                &v.spots,
                &v.notifications,
                &v.cop_patrols,
            ]
            .iter()
            .any(|set| set.len() > 1024)
            || v.event_order.len() > 1024
            || v.driving_order.len() > 2048
            || v.event_order
                .iter()
                .map(|e| e.volume)
                .collect::<BTreeSet<_>>()
                != v.events
            || v.event_order
                .iter()
                .any(|e| e.notifications.iter().any(|n| !v.notifications.contains(n)))
            || v.driving_order.iter().copied().collect::<BTreeSet<_>>()
                != v.activities.union(&v.spots).copied().collect()
        {
            return Err(bad());
        }
        let p = &self.selection_program;
        p.program.validate().map_err(|_| bad())?;
        p.composition.dispatcher()?;
        nfs_world::scoring::event_sync::Router::new(1, &p.signals).map_err(|_| bad())?;
        if !matches!(
            p.services.get(&p.root),
            Some(nfs_content::prologue::sequences::program::Service::Root)
        ) || p
            .services
            .keys()
            .chain(p.links.keys())
            .any(|id| usize::from(*id) >= p.composition.nodes.len())
            || p.signals
                .iter()
                .any(|s| usize::from(s.node) >= p.composition.nodes.len())
        {
            return Err(bad());
        }
        Ok(())
    }
}

impl Content {
    pub fn prepare_cached(
        index: &mut AssetIndex,
        parent: Asset,
        items: &InventoryCatalog,
        vehicles: &vehicle_content::Content,
        ai_items: &ItemContent,
        cache: Option<&Cache>,
    ) -> Result<Self, ContentError> {
        let Some(cache) = cache else {
            return Self::prepare(index, parent, items)?.with_opponents(
                index,
                vehicles.clone(),
                ai_items,
            );
        };
        cache.prepare(
            "complete-prologue",
            &serde_json::to_vec(&parent)
                .map_err(|_| ContentError::Cache("prologue parent identity"))?,
            |bytes| {
                tracing::info!(bytes = bytes.len(), "validating complete prologue cache");
                let snapshot: Snapshot = serde_json::from_slice(bytes)
                    .map_err(|e| ContentError::Content(format!("prepared prologue decode: {e}")))?;
                snapshot.finish(items, vehicles, ai_items)
            },
            || {
                serde_json::to_vec(&Snapshot::build(index, parent, items)?)
                    .map_err(|e| ContentError::Content(format!("prologue serialization: {e}")))
            },
        )
    }
}
