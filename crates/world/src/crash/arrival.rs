// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The controlled arrival after relocation owns a temporary actor-control hold.
//! Its authored timer releases that hold and sends Stop; only the subsequent
//! owned client completion permits the caller to restore driving presentation.
use crate::{
    replication::{Error, Record, Update, entity::creation::Content, players::Players},
    sequences::{Completed, Definition, Layout, Owner, Stop},
};
use nfs_protocol::world::rpc::Serial;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
enum Phase {
    Running { stop_at: u64 },
    Stopping,
    Complete,
}
#[derive(Clone, Debug)]
struct Entry {
    owner: Owner,
    actor: u16,
    prior_control: bool,
    ghost: u16,
    number: u32,
    phase: Phase,
}
#[derive(Clone, Debug)]
pub struct Arrivals {
    definition: Definition,
    content: Content,
    duration_ms: u64,
    world_ms: u64,
    entries: BTreeMap<u16, Entry>,
}

/// Publish each actor update before its Stop, in the same accepted transaction.
#[derive(Debug)]
pub struct Stopped {
    pub actor: Option<Record>,
    pub stop: Stop,
}
impl Arrivals {
    pub fn new(definition: Definition, content: Content, duration_ms: u64) -> Result<Self, Error> {
        if definition.layout != Layout::ControlledArrival
            || definition.embedded_bus == 0
            || definition.presentation.is_some()
            || !(1..=super::MAX_DURATION_MS).contains(&duration_ms)
            || content.profile(definition.asset)? != &definition.layout.profile()
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
    pub fn active(&self, participant: u16) -> bool {
        self.entries
            .get(&participant)
            .is_some_and(|e| !matches!(e.phase, Phase::Complete))
    }
    pub fn claims(&self, ghost: u16, selector: u16) -> bool {
        selector == 0 && self.entries.values().any(|e| e.ghost == ghost)
    }
    pub fn completed(&self, ghost: u16) -> bool {
        self.entries
            .values()
            .any(|e| e.ghost == ghost && matches!(e.phase, Phase::Complete))
    }
    /// Caller admits only a completed relocation. Failed ownership, binding,
    /// allocation or clock checks consume neither actor state nor sequence IDs.
    pub fn start(
        &mut self,
        players: &mut Players,
        owner: Owner,
        now: u64,
    ) -> Result<Vec<Record>, Error> {
        if now < self.world_ms
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::Shape);
        }
        let old = self.entries.get(&owner.participant);
        if old.is_some_and(|e| e.owner != owner) {
            return Err(Error::UnknownObject);
        }
        if self.active(owner.participant) {
            if players.owned_actor(owner.connection, owner.persona, owner.participant)
                != old.map(|e| e.actor)
            {
                return Err(Error::UnknownObject);
            }
            return Ok(vec![]);
        }
        if old.is_none() && self.entries.len() >= super::MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let number = old
            .map_or(0, |e| e.number)
            .checked_add(1)
            .ok_or(Error::Bound)?;
        let stop_at = now.checked_add(self.duration_ms).ok_or(Error::Bound)?;
        let actor = players
            .owned_actor(owner.connection, owner.persona, owner.participant)
            .ok_or(Error::UnknownObject)?;
        let Some(Update::Actor(state)) = players.objects().get(actor).map(|o| &o.current) else {
            return Err(Error::TypeMismatch);
        };
        let prior_control = state.flag_a.ok_or(Error::Shape)?;
        let creation = self
            .definition
            .creation(players.objects(), owner.participant, number)?;
        let mut next = players.clone();
        let mut records = next.spawn_entities(vec![creation], &self.content)?;
        let ghost = records[0].id;
        records.extend(next.set_actor_control(
            owner.connection,
            owner.persona,
            owner.participant,
            actor,
            true,
        )?);
        *players = next;
        self.entries.insert(
            owner.participant,
            Entry {
                owner,
                actor,
                prior_control,
                ghost,
                number,
                phase: Phase::Running { stop_at },
            },
        );
        self.world_ms = now;
        Ok(records)
    }
    /// These are ordinary authored timer stops, never emergency fallbacks.
    /// Capacity counts complete actor/Stop pairs; pressure leaves both pending.
    pub fn poll(
        &mut self,
        players: &mut Players,
        now: u64,
        capacity: usize,
    ) -> Result<Vec<Stopped>, Error> {
        if now < self.world_ms {
            return Err(Error::Shape);
        }
        let due: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.phase, Phase::Running { stop_at } if now >= stop_at))
            .take(capacity)
            .map(|(&p, _)| p)
            .collect();
        if due.is_empty() {
            self.world_ms = now;
            return Ok(vec![]);
        }
        let mut next = players.clone();
        let mut stopped = Vec::new();
        for &participant in &due {
            let e = &self.entries[&participant];
            // An overlapping lifecycle must not silently have released this
            // hold. Reject the whole poll instead of overwriting its state.
            if !matches!(next.objects().get(e.actor).map(|o| &o.current), Some(Update::Actor(a)) if a.flag_a == Some(true))
            {
                return Err(Error::Shape);
            }
            let actor = next.set_actor_control(
                e.owner.connection,
                e.owner.persona,
                participant,
                e.actor,
                e.prior_control,
            )?;
            stopped.push(Stopped {
                actor,
                stop: Stop {
                    sequence: e.ghost,
                    serial: Serial::new(1).expect("fresh lifetime"),
                },
            });
        }
        *players = next;
        for participant in due {
            self.entries
                .get_mut(&participant)
                .expect("selected owner")
                .phase = Phase::Stopping;
        }
        self.world_ms = now;
        Ok(stopped)
    }
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
        let e = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if e.owner != owner || e.ghost != call.sequence {
            return Err(Error::UnknownObject);
        }
        match e.phase {
            Phase::Complete => Ok(None),
            Phase::Running { .. } => Err(Error::Shape),
            Phase::Stopping => {
                if players.owned_actor(owner.connection, owner.persona, owner.participant)
                    != Some(e.actor)
                {
                    return Err(Error::UnknownObject);
                }
                players.remove_entities(&[e.ghost])?;
                e.phase = Phase::Complete;
                Ok(Some(e.ghost))
            }
        }
    }
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
