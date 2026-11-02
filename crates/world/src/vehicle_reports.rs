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
/// Fresh authoritative inputs and their independently throttled replication.
#[derive(Clone, Debug, Default)]
pub struct Received {
    /// Owned vehicles whose newer revisions were successfully retained.
    pub accepted: Vec<u16>,
    /// Records due for replication; these may be empty despite accepted input.
    pub echoes: Vec<Record>,
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
        self.receive_with_accepted(players, connection, persona, movement, cars, now_ms)
            .map(|received| received.echoes)
    }

    /// Return every fresh accepted vehicle separately from outgoing echoes so
    /// owned input consumers are not limited by the replication interval.
    pub fn receive_with_accepted(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        movement: &frame::Movement<'_>,
        cars: &BTreeSet<u16>,
        now_ms: u64,
    ) -> Result<Received, Error> {
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
        self.receive_ai_with_accepted(players, connection, persona, movement, cars, now_ms)
            .map(|received| received.echoes)
    }

    /// Fresh AI input is independent of the outgoing echo cadence, just like
    /// player input. Validate every delegated key before retaining any sample.
    pub fn receive_ai_with_accepted(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        movement: &frame::Movement<'_>,
        cars: &BTreeMap<u16, u64>,
        now_ms: u64,
    ) -> Result<Received, Error> {
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
    ) -> Result<Received, Error> {
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
        let mut output = Received::default();
        for (id, target, sample) in reports {
            let stamp = sample.physics.identity_words;
            let physics = sample.physics;
            let echo = if let Some(entry) = next.entries.get_mut(&id) {
                if now_ms < entry.sent_ms {
                    return Err(Error::Shape);
                }
                if stamp[2] <= entry.stamp[2] {
                    continue;
                }
                entry.stamp = stamp;
                if now_ms - entry.sent_ms < ECHO_INTERVAL_MS {
                    false
                } else {
                    entry.sent_ms = now_ms;
                    true
                }
            } else {
                next.entries.insert(
                    id,
                    Entry {
                        target,
                        stamp,
                        sent_ms: now_ms,
                    },
                );
                true
            };
            // Throttle outgoing replication, never the accepted authoritative
            // pose or controls. Distance, crash and other owned state readers
            // must see inputs received between two echoes as well.
            match target {
                Target::Player => {
                    let records = world.set_vehicle_physics(connection, persona, id, physics)?;
                    if echo {
                        output.echoes.extend(records);
                    }
                }
                Target::Ai(key) => {
                    let records =
                        world.set_ai_vehicle_physics(connection, persona, key, physics)?;
                    // The first accepted report proves the client drives the car;
                    // only then is its actor's control suppression cleared.
                    if echo {
                        output.echoes.extend(records);
                        output
                            .echoes
                            .extend(world.release_ai_control(connection, persona, key)?);
                    }
                }
            }
            output.accepted.push(id);
        }
        *self = next;
        *players = world;
        Ok(output)
    }
}
