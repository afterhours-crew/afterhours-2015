// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-owned selection for the next garage departure. No progression
//! updates or event start are implied by selecting an authored location.
use super::*;
use nfs_content::events::{
    discovery::{Endpoint as Site, teleport::Definitions},
    restart::Destination,
};
use nfs_world::{
    participants::{Lifecycle, Stage},
    teleport::selection::Selection,
};

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    selected: BTreeMap<u16, (Site, Destination)>,
    cursors: BTreeMap<Site, usize>,
}
impl State {
    pub(super) fn retain(&mut self, participants: &Lifecycle) {
        self.selected.retain(|&id, _| {
            matches!(
                participants.stage(id),
                Some(Stage::Customization | Stage::ExitingGarage)
            )
        });
    }
    pub(super) fn position(&self, participant: u16) -> Option<Destination> {
        self.selected.get(&participant).map(|(_, point)| *point)
    }
    fn select(
        &mut self,
        participant: u16,
        site: Site,
        stage: Option<Stage>,
        points: &[Destination],
    ) -> Result<(), replication::Error> {
        if !matches!(stage, Some(Stage::Customization | Stage::ExitingGarage)) || points.is_empty()
        {
            return Err(replication::Error::Shape);
        }
        if self
            .selected
            .get(&participant)
            .is_some_and(|(old, _)| *old == site)
        {
            return Ok(());
        }
        if stage != Some(Stage::Customization) {
            return Err(replication::Error::Shape);
        }
        if self.selected.len() >= nfs_world::participant_states::MAX_PARTICIPANTS
            && !self.selected.contains_key(&participant)
            || self.cursors.len() >= nfs_content::events::discovery::MAX_EVENTS
                && !self.cursors.contains_key(&site)
        {
            return Err(replication::Error::Bound);
        }
        let cursor = self.cursors.get(&site).copied().unwrap_or(0) % points.len();
        self.selected.insert(participant, (site, points[cursor]));
        self.cursors.insert(site, (cursor + 1) % points.len());
        Ok(())
    }
    pub(super) fn receive(
        &mut self,
        definitions: Option<&Definitions>,
        players: &Players,
        participants: &Lifecycle,
        persona: u64,
        body: nfs_protocol::world::BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
        let Some(definitions) = definitions else {
            return Ok(false);
        };
        let Some(manager) = definitions.manager else {
            return Ok(false);
        };
        let world = players.objects();
        let Some(manager) = world.scene_endpoint(manager.scene_key, manager.serializer) else {
            return Ok(false);
        };
        let Some(request) = Selection::decode(body, manager)? else {
            return Ok(false);
        };
        if !players.owns_participant(HOST_SELECTOR as u8, persona, request.participant) {
            return Err(replication::Error::UnknownObject);
        }
        let mut matches = definitions.locations.iter().filter(|(site, _)| {
            world.scene_endpoint(site.scene_key, site.serializer)
                == Some((request.scene, request.selector))
        });
        let Some((&site, points)) = matches.next() else {
            return Err(replication::Error::UnknownObject);
        };
        if matches.next().is_some() {
            return Err(replication::Error::Shape);
        }
        self.select(
            request.participant,
            site,
            participants.stage(request.participant),
            points,
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
