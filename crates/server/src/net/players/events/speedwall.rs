// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

impl PlayerListener {
    pub(super) fn event_speedwall(
        &self,
        content: &event_logic::Content,
        players: &Players,
        car: u16,
        results: event_logic::Results<'_>,
    ) -> Result<Option<nfs_world::speedwall::Progress>, replication::Error> {
        // Reference-only event harnesses can run without inventory/vehicle
        // content. A configured runtime must resolve the actual raced car.
        let Some(inventory) = self.inventory.as_ref() else {
            return Ok(None);
        };
        let vehicle = players
            .vehicle_item_of(HOST_SELECTOR as u8, self.persona, car)
            .and_then(|id| inventory.items.items.get(&id))
            .ok_or(replication::Error::UnknownObject)?;
        let info = self
            .vehicles
            .as_ref()
            .and_then(|c| c.vehicles.definition(&vehicle.definition))
            .and_then(|d| d.info.as_ref())
            .ok_or(replication::Error::Unsupported)?;
        let total = content.result_rep(results)?;
        if results.finish.race_ms < 0 || total < 0 {
            return Err(replication::Error::Bound);
        }
        let progress = nfs_world::speedwall::Progress {
            score_bits: 0f32.to_bits(),
            time_ms_bits: (results.finish.race_ms as f32).to_bits(),
            rep_bits: (total as f32).to_bits(),
            position: results
                .finish
                .position
                .try_into()
                .map_err(|_| replication::Error::Bound)?,
            event_id: content.record_id(),
            vehicle: info.short_name.as_str().as_bytes().to_vec(),
        };
        progress.encode()?;
        Ok(Some(progress))
    }

    pub(in crate::net::players) fn event_credit_committed(&mut self, participant: u16, batch: u64) {
        if let Some(race) = self
            .event_state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            && race.credit_batch == Some(batch)
        {
            race.credit_committed = true;
        }
    }
}
