// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The arrive sequence after garage exit.
//!
//! When the client reports the world ready, official hosts enter the startup
//! arrive state (field 79, wire selector 78) and create a participant state
//! sequence with the same layout as the garage waiting sequence (manager
//! Gameplay selector 2, embedded bus 2; E101 and E742 used a Garage-bundle
//! sequence asset). The client acknowledges it (selector 2, method 0), plays
//! it and fires [`ARRIVED`] on its entity 1 when done; the host stops it, the
//! client confirms (selector 3, method 0) and completes (selector 0, method
//! 1), and the host deletes it and leaves the state (E101 109,092-115,292 ms,
//! E742 134,112-142,111 ms).

use crate::{
    logic::Message,
    replication::{Error, Record, entity::creation::Content, players::Players},
    sequences::{Completed, Definition, Owner, Stop, profile},
};
use nfs_protocol::world::rpc::Serial;
use std::collections::BTreeMap;

/// Client event on the sequence's entity 1 when it has played.
pub const ARRIVED: u32 = 13_572_341;
/// Stop without the client's event after this long (local policy).
pub const FALLBACK_MS: u64 = 30_000;
pub const MAX_INSTANCES: usize = 128;
/// Client acknowledgements (method 0): started, then done before completing.
const ACKNOWLEDGED: [u16; 2] = [2, 3];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Running { fallback_at: u64 },
    Stopping,
    Complete,
}

#[derive(Clone, Copy, Debug)]
struct Instance {
    owner: Owner,
    ghost: u16,
    phase: Phase,
}

#[derive(Clone, Debug)]
pub struct Arrivals {
    definition: Definition,
    content: Content,
    entries: BTreeMap<u16, Instance>,
    world_ms: u64,
}

impl Arrivals {
    pub fn new(definition: Definition, content: Content) -> Result<Self, Error> {
        if definition.embedded_bus == 0 || content.profile(definition.asset)? != &profile() {
            return Err(Error::Shape);
        }
        Ok(Self {
            definition,
            content,
            entries: BTreeMap::new(),
            world_ms: 0,
        })
    }

    /// Create the participant's arrive sequence. Its number follows the
    /// garage waiting sequence (0); E101 numbered it 1.
    pub fn start(&mut self, players: &mut Players, owner: Owner) -> Result<Option<Record>, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        if let Some(old) = self.entries.get(&owner.participant) {
            return if old.owner == owner {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.entries.len() >= MAX_INSTANCES {
            return Err(Error::Bound);
        }
        let fallback_at = self.world_ms.checked_add(FALLBACK_MS).ok_or(Error::Bound)?;
        let creation = self
            .definition
            .creation(players.objects(), owner.participant, 1)?;
        let record = players
            .spawn_entities(vec![creation], &self.content)?
            .remove(0);
        self.entries.insert(
            owner.participant,
            Instance {
                owner,
                ghost: record.id,
                phase: Phase::Running { fallback_at },
            },
        );
        Ok(Some(record))
    }

    /// The client's [`ARRIVED`] event: the sequence has played; stop it.
    /// `Ok(None)` for other messages and repeats.
    pub fn arrived(&mut self, message: &Message) -> Result<Option<Stop>, Error> {
        let Message::Reached { event, target, .. } = message else {
            return Ok(None);
        };
        if *event != ARRIVED || target.entity != 1 {
            return Ok(None);
        }
        let Some(instance) = self.entries.values_mut().find(|i| i.ghost == target.ghost) else {
            return Ok(None);
        };
        if !matches!(instance.phase, Phase::Running { .. }) {
            return Ok(None);
        }
        instance.phase = Phase::Stopping;
        Ok(Some(stop(instance.ghost)))
    }

    /// Stops for sequences that never reported [`ARRIVED`].
    pub fn poll(&mut self, world_ms: u64) -> Result<Vec<Stop>, Error> {
        if world_ms < self.world_ms {
            return Err(Error::Shape);
        }
        self.world_ms = world_ms;
        let mut stops = Vec::new();
        for instance in self.entries.values_mut() {
            if matches!(instance.phase, Phase::Running { fallback_at } if world_ms >= fallback_at) {
                instance.phase = Phase::Stopping;
                stops.push(stop(instance.ghost));
            }
        }
        Ok(stops)
    }

    /// Whether `(ghost, selector, method)` is a client acknowledgement of a
    /// live arrive sequence (no reply).
    pub fn acknowledges(&self, ghost: u16, selector: u16, method: u32) -> bool {
        method == 0
            && ACKNOWLEDGED.contains(&selector)
            && self.entries.values().any(|i| i.ghost == ghost)
    }

    pub fn has_completion(&self, ghost: u16, selector: u16) -> bool {
        selector == 0 && self.entries.values().any(|i| i.ghost == ghost)
    }

    /// The client's completion after the stop. Returns the sequence to delete
    /// the first time; `None` for repeats.
    pub fn complete(
        &mut self,
        players: &Players,
        owner: Owner,
        call: Completed,
    ) -> Result<Option<u16>, Error> {
        call.encode()?;
        if owner.participant != call.participant
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        let instance = self
            .entries
            .get_mut(&call.participant)
            .ok_or(Error::UnknownObject)?;
        if instance.owner != owner || instance.ghost != call.sequence {
            return Err(Error::UnknownObject);
        }
        match instance.phase {
            Phase::Running { .. } => Err(Error::Shape),
            Phase::Stopping => {
                instance.phase = Phase::Complete;
                Ok(Some(instance.ghost))
            }
            Phase::Complete => Ok(None),
        }
    }

    pub fn ghost(&self, participant: u16) -> Option<u16> {
        self.entries.get(&participant).map(|i| i.ghost)
    }
}

fn stop(sequence: u16) -> Stop {
    Stop {
        sequence,
        serial: Serial::new(1).expect("fresh lifetime"),
    }
}

#[cfg(test)]
mod tests;
