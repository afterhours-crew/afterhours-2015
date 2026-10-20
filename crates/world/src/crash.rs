// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant crash sequences. The signal is latched independently of the
//! movement echo rate; clearing it does not cancel a running cinematic.
use crate::{
    replication::{
        Error, Record, entity,
        entity::creation::{Asset, Content, Creation, Prefix},
        players::Players,
    },
    sequences::{Completed, Component, Owner, Stop},
};
use nfs_protocol::world::rpc::Serial;
use std::collections::BTreeMap;

pub const MAX_PARTICIPANTS: usize = 128;
pub const MAX_DURATION_MS: u64 = 30_000;

pub fn profile() -> entity::Profile {
    entity::Profile::new(&[
        entity::Kind::SequenceRoot,
        entity::Kind::RpcOptionalReference,
    ])
    .expect("fixed crash sequence profile")
}

/// Installed sequence identity. Unlike the garage sequences, this sequence
/// has no linked-channel serializer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub asset: Asset,
    pub manager: Component,
    pub embedded_bus: u32,
}
impl Definition {
    pub fn creation(self, players: &Players, owner: Owner, number: u32) -> Result<Creation, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let (manager, manager_selector) = players
            .objects()
            .scene_endpoint(self.manager.scene_key, self.manager.index)
            .ok_or(Error::UnknownObject)?;
        let rpc = |selector| entity::Rpc {
            selector,
            serial: Serial::new(1).expect("fresh lifetime"),
        };
        Ok(Creation {
            prefix: Prefix {
                parent: None,
                blueprint: manager,
                sub_id: self.embedded_bus,
                owner: None,
                asset: self.asset,
            },
            body: entity::Body {
                initial: Some(vec![
                    entity::Initial::SequenceRoot {
                        value: 1,
                        rpc: rpc(0),
                        manager,
                        manager_selector,
                        sequence: number,
                        participants: vec![owner.participant],
                        stopping: false,
                    },
                    entity::Initial::RpcOptionalReference {
                        rpc: rpc(1),
                        reference: Some(owner.participant),
                    },
                ]),
                updates: vec![Some(entity::Update::Noop); 2],
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Pending,
    Running { stop_at: u64 },
    Stopping,
    Complete,
}
#[derive(Clone, Debug)]
struct Entry {
    owner: Owner,
    stamp: u32,
    signal: bool,
    phase: Phase,
    ghost: Option<u16>,
    number: u32,
}
#[derive(Clone, Debug)]
pub struct Crashes {
    definition: Definition,
    content: Content,
    duration_ms: u64,
    world_ms: u64,
    entries: BTreeMap<u16, Entry>,
}
impl Crashes {
    pub fn new(definition: Definition, content: Content, duration_ms: u64) -> Result<Self, Error> {
        if definition.embedded_bus == 0
            || !(1..=MAX_DURATION_MS).contains(&duration_ms)
            || content.profile(definition.asset)? != &profile()
        {
            return Err(Error::Shape);
        }
        Ok(Self {
            definition,
            content,
            duration_ms,
            world_ms: 0,
            entries: BTreeMap::new(),
        })
    }

    /// Feed only a decoded report from this participant's current owned car.
    /// The caller commits this state with the accepted movement frame. Duplicate
    /// and out-of-order revisions cannot rearm or cancel a pending recovery.
    pub fn observe(
        &mut self,
        players: &Players,
        owner: Owner,
        revision: u32,
        signal: bool,
        eligible: bool,
    ) -> Result<(), Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        if let Some(entry) = self.entries.get_mut(&owner.participant) {
            if entry.owner != owner {
                return Err(Error::UnknownObject);
            }
            if revision <= entry.stamp {
                return Ok(());
            }
            entry.stamp = revision;
            if eligible
                && signal
                && !entry.signal
                && matches!(entry.phase, Phase::Idle | Phase::Complete)
            {
                entry.phase = Phase::Pending;
            }
            entry.signal = signal;
        } else {
            if self.entries.len() >= MAX_PARTICIPANTS {
                return Err(Error::Bound);
            }
            self.entries.insert(
                owner.participant,
                Entry {
                    owner,
                    stamp: revision,
                    signal,
                    phase: if signal && eligible {
                        Phase::Pending
                    } else {
                        Phase::Idle
                    },
                    ghost: None,
                    number: 0,
                },
            );
        }
        Ok(())
    }
    pub fn pending(&self) -> Vec<Owner> {
        self.entries
            .values()
            .filter(|e| e.phase == Phase::Pending)
            .map(|e| e.owner)
            .collect()
    }
    pub fn active(&self, participant: u16) -> bool {
        self.entries.get(&participant).is_some_and(|e| {
            matches!(
                e.phase,
                Phase::Pending | Phase::Running { .. } | Phase::Stopping
            )
        })
    }
    /// A conflicting lifecycle transition won before the queued crash began.
    pub fn discard_pending(&mut self, participant: u16) {
        if let Some(entry) = self.entries.get_mut(&participant)
            && entry.phase == Phase::Pending
        {
            entry.phase = Phase::Idle;
        }
    }
    /// Queue room must be reserved before starting. A pending pulse survives
    /// until the caller can commit its complete output.
    pub fn start(&mut self, players: &mut Players, owner: Owner) -> Result<Option<Record>, Error> {
        let entry = self
            .entries
            .get(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if entry.owner != owner {
            return Err(Error::UnknownObject);
        }
        if entry.phase != Phase::Pending {
            return Ok(None);
        }
        let number = entry.number.checked_add(1).ok_or(Error::Bound)?;
        let stop_at = self
            .world_ms
            .checked_add(self.duration_ms)
            .ok_or(Error::Bound)?;
        let creation = self.definition.creation(players, owner, number)?;
        let record = players
            .spawn_entities(vec![creation], &self.content)?
            .remove(0);
        let entry = self
            .entries
            .get_mut(&owner.participant)
            .expect("validated owner");
        entry.phase = Phase::Running { stop_at };
        entry.ghost = Some(record.id);
        entry.number = number;
        Ok(Some(record))
    }
    /// The installed server timer is a fallback. The client can complete the
    /// cinematic before it expires, without first receiving a Stop.
    pub fn poll(&mut self, world_ms: u64, capacity: usize) -> Result<Vec<Stop>, Error> {
        if world_ms < self.world_ms {
            return Err(Error::Shape);
        }
        self.world_ms = world_ms;
        let mut stops = Vec::new();
        for entry in self.entries.values_mut() {
            if stops.len() < capacity
                && matches!(entry.phase, Phase::Running { stop_at } if world_ms >= stop_at)
            {
                stops.push(Stop {
                    sequence: entry.ghost.ok_or(Error::UnknownObject)?,
                    serial: Serial::new(1).expect("fresh lifetime"),
                });
                entry.phase = Phase::Stopping;
            }
        }
        Ok(stops)
    }
    pub fn has_completion(&self, ghost: u16, selector: u16) -> bool {
        selector == 0 && self.entries.values().any(|e| e.ghost == Some(ghost))
    }
    /// A queued fallback is obsolete once this sequence has completed.
    pub fn completed(&self, ghost: u16) -> bool {
        self.entries
            .values()
            .any(|e| e.ghost == Some(ghost) && e.phase == Phase::Complete)
    }
    /// Remove only this owner's sequence, once. Retain its last ID so reliable
    /// duplicate completion calls are harmless until the next lifetime.
    pub fn complete(
        &mut self,
        players: &mut Players,
        owner: Owner,
        call: Completed,
    ) -> Result<Option<u16>, Error> {
        call.encode()?;
        if call.participant != owner.participant
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        let entry = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if entry.owner != owner || entry.ghost != Some(call.sequence) {
            return Err(Error::UnknownObject);
        }
        match entry.phase {
            Phase::Complete => Ok(None),
            Phase::Running { .. } | Phase::Stopping => {
                players.remove_entities(&[call.sequence])?;
                entry.phase = Phase::Complete;
                Ok(Some(call.sequence))
            }
            Phase::Idle | Phase::Pending => Err(Error::Shape),
        }
    }
    /// Retire a car/participant lifetime on garage return or disconnect.
    /// Do not silently abandon a running sequence.
    pub fn retire(&mut self, participant: u16) -> Result<(), Error> {
        if self.active(participant) {
            return Err(Error::Shape);
        }
        self.entries.remove(&participant);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
