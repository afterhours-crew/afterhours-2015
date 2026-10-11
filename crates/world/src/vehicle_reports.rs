// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded replication of movement from a car's authoritative client.
use crate::{
    frame,
    replication::{Error, Record, players::Players, vehicle::movement::Sample},
};
use std::collections::{BTreeMap, BTreeSet};
pub const ECHO_INTERVAL_MS: u64 = 100;
#[derive(Clone, Debug)]
struct Entry {
    stamp: [u32; 3],
    sent_ms: u64,
}
#[derive(Clone, Debug, Default)]
pub struct Reports {
    entries: BTreeMap<u16, Entry>,
}
impl Reports {
    /// Publish the stored initial chassis once the newly created world car is
    /// reporting, then retain and echo its current client-authoritative state.
    /// Clock/revision words always come from the accepted report's header.
    /// The caller commits this clone together with its frame and output queue.
    pub fn receive(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        movement: &frame::Movement<'_>,
        cars: &BTreeSet<u16>,
        now_ms: u64,
    ) -> Result<Vec<Record>, Error> {
        let header = movement.header.ok_or(Error::Shape)?;
        if cars.len() > frame::MAX_MOVEMENT_RECORDS {
            return Err(Error::Bound);
        }
        let mut reports = Vec::new();
        let mut seen = BTreeSet::new();
        for record in &movement.records {
            if !cars.contains(&record.id) {
                continue;
            }
            if record.component != 0 || !seen.insert(record.id) {
                return Err(Error::Shape);
            }
            let sample = Sample::decode(header, record.data)?;
            let current = players.vehicle_physics(connection, persona, record.id)?;
            reports.push((record.id, sample, current));
        }
        let mut next = self.clone();
        let mut world = players.clone();
        next.entries.retain(|id, _| cars.contains(id));
        let mut output = Vec::new();
        for (id, sample, initial) in reports {
            let stamp = sample.physics.identity_words;
            let mut physics = sample.physics;
            if let Some(entry) = next.entries.get_mut(&id) {
                if now_ms < entry.sent_ms {
                    return Err(Error::Shape);
                }
                if stamp[2] <= entry.stamp[2] {
                    continue;
                }
                entry.stamp = stamp;
                if now_ms - entry.sent_ms < ECHO_INTERVAL_MS {
                    continue;
                }
                entry.sent_ms = now_ms;
            } else {
                // A zero-time initializer is followed by the first timestamped
                // owned state after authority starts; do not replace its intended
                // starting mode with the client's constructor-default report.
                physics = initial;
                physics.identity_words = stamp;
                next.entries.insert(
                    id,
                    Entry {
                        stamp,
                        sent_ms: now_ms,
                    },
                );
            }
            output.extend(world.set_vehicle_physics(connection, persona, id, physics)?);
        }
        *self = next;
        *players = world;
        Ok(output)
    }
}
