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
    target: Target,
    stamp: [u32; 3],
    sent_ms: u64,
}
#[derive(Clone, Debug, Default)]
pub struct Reports {
    entries: BTreeMap<u16, Entry>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Player,
    Ai(u64),
}
impl Reports {
    /// Retain and echo the current client-authoritative chassis, including the
    /// first report, which may already reflect a teleport or driving transition.
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
        let targets = cars.iter().map(|&id| (id, Target::Player)).collect();
        self.receive_targets(players, connection, persona, movement, &targets, now_ms)
    }

    /// Accept reports only for the host-selected AI spawn keys in this session.
    /// The ID/key association is checked against retained world state, so a
    /// caller cannot delegate another session's car or turn AI into inventory.
    pub fn receive_ai(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        movement: &frame::Movement<'_>,
        cars: &BTreeMap<u16, u64>,
        now_ms: u64,
    ) -> Result<Vec<Record>, Error> {
        if cars.len() > frame::MAX_MOVEMENT_RECORDS {
            return Err(Error::Bound);
        }
        for (&id, &key) in cars {
            if players
                .ai_ids(connection, persona, key)
                .is_none_or(|ids| ids.vehicle != id)
            {
                return Err(Error::UnknownObject);
            }
        }
        let targets = cars
            .iter()
            .map(|(&id, &key)| (id, Target::Ai(key)))
            .collect();
        self.receive_targets(players, connection, persona, movement, &targets, now_ms)
    }

    fn receive_targets(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        movement: &frame::Movement<'_>,
        cars: &BTreeMap<u16, Target>,
        now_ms: u64,
    ) -> Result<Vec<Record>, Error> {
        let header = movement.header.ok_or(Error::Shape)?;
        if cars.len() > frame::MAX_MOVEMENT_RECORDS {
            return Err(Error::Bound);
        }
        let mut reports = Vec::new();
        let mut seen = BTreeSet::new();
        for record in &movement.records {
            let Some(&target) = cars.get(&record.id) else {
                continue;
            };
            if record.component != 0 || !seen.insert(record.id) {
                return Err(Error::Shape);
            }
            let sample = Sample::decode(header, record.data)?;
            // Validate ownership and the chassis shape before staging any
            // reports. The stored pose is not a substitute for client state.
            match target {
                Target::Player => players.vehicle_physics(connection, persona, record.id)?,
                Target::Ai(key) => players.ai_vehicle_physics(connection, persona, key)?,
            };
            reports.push((record.id, target, sample));
        }
        let mut next = self.clone();
        let mut world = players.clone();
        next.entries
            .retain(|id, entry| cars.get(id) == Some(&entry.target));
        let mut output = Vec::new();
        for (id, target, sample) in reports {
            let stamp = sample.physics.identity_words;
            let physics = sample.physics;
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
                next.entries.insert(
                    id,
                    Entry {
                        target,
                        stamp,
                        sent_ms: now_ms,
                    },
                );
            }
            match target {
                Target::Player => {
                    output.extend(world.set_vehicle_physics(connection, persona, id, physics)?)
                }
                Target::Ai(key) => {
                    output.extend(world.set_ai_vehicle_physics(connection, persona, key, physics)?);
                    // The first accepted report proves the client drives the car;
                    // only then is its actor's control suppression cleared.
                    output.extend(world.release_ai_control(connection, persona, key)?);
                }
            }
        }
        *self = next;
        *players = world;
        Ok(output)
    }
}
