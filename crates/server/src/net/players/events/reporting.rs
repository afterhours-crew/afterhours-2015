// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::items::story::{Action, Request};

impl PlayerListener {
    /// Validate the story worker's callback before any world state is adopted.
    /// A finish credit alone cannot release the authored result-report barrier.
    pub(in crate::net::players) fn event_story_reported(
        &self,
        request: &Request,
        loaded: &crate::persistent::Loaded,
    ) -> Result<bool, replication::Error> {
        let Action::Complete { objective } = request.action else {
            return Ok(false);
        };
        let Some(race) = self
            .event_state
            .entries
            .get(&request.participant)
            .and_then(|e| e.race.as_ref())
        else {
            return Ok(false);
        };
        let Some(story) = &race.story else {
            return Ok(false);
        };
        if story.objective != objective {
            return Ok(false);
        }
        if !race.credit_committed || !race.reporting_started || race.exit != Some(Exit::Completed) {
            return Err(replication::Error::Shape);
        }
        let restored = story
            .progression
            .catalog
            .restore(loaded, story.progression.settings)
            .map_err(|_| replication::Error::Shape)?;
        if !restored
            .branch(crate::progression::Branch::Main)
            .objectives()
            .get(&objective)
            .is_some_and(|s| s.completed)
        {
            return Err(replication::Error::Shape);
        }
        Ok(true)
    }

    /// Called after output capacity and all other adoption checks pass.
    pub(in crate::net::players) fn release_event_story(&mut self, participant: u16) {
        if let Some(race) = self
            .event_state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
        {
            race.story = None;
        }
    }
}
