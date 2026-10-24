// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant membership in a client-observed collection of volumes. One
//! instance owns one filter's responsibility tokens and output set. Transport
//! identities, weak-owner resolution and source-set subscriptions stay outside.
use crate::{replication::Error, world_activation::Call};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PARTICIPANTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    Send {
        owner: u64,
        participant: u64,
        call: Call,
    },
    Membership {
        participant: u64,
        inside: bool,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Row {
    owner: Option<u64>,
    generation: u32,
    // Kept separately from output membership: controllable destruction removes
    // membership but preserves this value for a later assignment.
    inside: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter {
    participants: BTreeMap<u64, Row>,
    controllables: BTreeMap<u64, u64>,
    members: BTreeSet<u64>,
}

impl Filter {
    pub fn members(&self) -> &BTreeSet<u64> {
        &self.members
    }
    pub fn generation(&self, participant: u64) -> Option<u32> {
        self.participants.get(&participant).map(|r| r.generation)
    }
    pub fn owner(&self, participant: u64) -> Option<u64> {
        self.participants.get(&participant).and_then(|r| r.owner)
    }
    fn assignment(participant: u64, row: &Row) -> Vec<Effect> {
        row.owner
            .into_iter()
            .map(|owner| Effect::Send {
                owner,
                participant,
                call: Call::Subscribe {
                    token: row.generation,
                    inside: row.inside,
                },
            })
            .collect()
    }
    fn membership(&mut self, participant: u64, inside: bool) -> Option<Effect> {
        let changed = if inside {
            self.members.insert(participant)
        } else {
            self.members.remove(&participant)
        };
        changed.then_some(Effect::Membership {
            participant,
            inside,
        })
    }
    /// Source-set entry starts at generation zero, outside. A participant may
    /// exist before its controllable or its reporting owner becomes available.
    pub fn add(
        &mut self,
        participant: u64,
        controllable: Option<(u64, Option<u64>)>,
    ) -> Result<Vec<Effect>, Error> {
        if participant == 0 || controllable.is_some_and(|(car, owner)| car == 0 || owner == Some(0))
        {
            return Err(Error::Bound);
        }
        if self.participants.contains_key(&participant) {
            return Ok(vec![]);
        }
        if self.participants.len() >= MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        if controllable.is_some_and(|(car, _)| self.controllables.contains_key(&car)) {
            return Err(Error::DuplicateObject);
        }
        let row = Row {
            owner: controllable.and_then(|(_, owner)| owner),
            ..Row::default()
        };
        let effects = Self::assignment(participant, &row);
        if let Some((car, _)) = controllable {
            self.controllables.insert(car, participant);
        }
        self.participants.insert(participant, row);
        Ok(effects)
    }
    /// Owner-change callback. Weak-owner invalidation must be delivered first;
    /// `connected` answers whether the old owner's connection still exists.
    /// Losing an owner preserves both membership and the last reported flag.
    pub fn owner_changed(
        &mut self,
        controllable: u64,
        owner: Option<u64>,
        connected: impl Fn(u64) -> bool,
    ) -> Result<Vec<Effect>, Error> {
        if owner == Some(0) {
            return Err(Error::Bound);
        }
        let Some(&participant) = self.controllables.get(&controllable) else {
            return Ok(vec![]);
        };
        let row = self
            .participants
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        if row.owner == owner {
            return Ok(vec![]);
        }
        let mut effects = Vec::new();
        if let Some(old) = row.owner.filter(|old| connected(*old)) {
            effects.push(Effect::Send {
                owner: old,
                participant,
                call: Call::Unsubscribe,
            });
        }
        row.owner = owner;
        if owner.is_some() {
            row.generation = row.generation.wrapping_add(1);
            effects.extend(Self::assignment(participant, row));
        }
        Ok(effects)
    }
    /// Participant's controllable-change callback. Replacing the controllable
    /// subscribes its ownership and sends the existing generation/inside flag;
    /// an ownership callback on the same controllable instead advances the token.
    pub fn controllable_changed(
        &mut self,
        participant: u64,
        controllable: Option<(u64, Option<u64>)>,
        connected: impl Fn(u64) -> bool,
    ) -> Result<Vec<Effect>, Error> {
        if controllable.is_some_and(|(car, owner)| car == 0 || owner == Some(0)) {
            return Err(Error::Bound);
        }
        if !self.participants.contains_key(&participant) {
            return Ok(vec![]);
        }
        if let Some((car, owner)) = controllable
            && let Some(&existing) = self.controllables.get(&car)
        {
            if existing != participant {
                return Err(Error::DuplicateObject);
            }
            return self.owner_changed(car, owner, connected);
        }
        self.controllables.retain(|_, p| *p != participant);
        let row = self
            .participants
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        let Some((car, owner)) = controllable else {
            // Native callback clears the weak owner without revoking or
            // changing membership. Destruction has its own callback below.
            row.owner = None;
            return Ok(vec![]);
        };
        self.controllables.insert(car, participant);
        if owner.is_some() {
            row.owner = owner;
            Ok(Self::assignment(participant, row))
        } else {
            Ok(vec![])
        }
    }
    /// Weak player destruction invalidates references silently. It does not
    /// invent a controllable-owner callback or a volume crossing.
    pub fn owner_destroyed(&mut self, owner: u64) {
        for row in self.participants.values_mut() {
            if row.owner == Some(owner) {
                row.owner = None;
            }
        }
    }
    pub fn controllable_destroyed(&mut self, controllable: u64) -> Vec<Effect> {
        let Some(participant) = self.controllables.remove(&controllable) else {
            return vec![];
        };
        let Some(owner) = self
            .participants
            .get_mut(&participant)
            .and_then(|row| row.owner.take())
        else {
            return vec![];
        };
        let mut effects = vec![Effect::Send {
            owner,
            participant,
            call: Call::Unsubscribe,
        }];
        effects.extend(self.membership(participant, false));
        effects
    }
    /// The native RPC callback ignores the crossing clock. It compares the
    /// generation, stores the reported flag, then updates the deduplicated set.
    /// The network owner validates the sender before calling this method.
    pub fn report(&mut self, participant: u64, generation: u32, inside: bool) -> Option<Effect> {
        let row = self.participants.get_mut(&participant)?;
        if row.generation != generation {
            return None;
        }
        row.inside = inside;
        self.membership(participant, inside)
    }
    /// Connection boundary for a decoded report already routed to this filter.
    /// Unknown/retired participants and obsolete tokens are inert. A current
    /// token from anyone except its assigned owner is not authorized.
    pub fn report_from(
        &mut self,
        owner: u64,
        participant: u64,
        generation: u32,
        inside: bool,
    ) -> Result<Option<Effect>, Error> {
        let Some(row) = self.participants.get(&participant) else {
            return Ok(None);
        };
        if row.generation != generation {
            return Ok(None);
        }
        if row.owner != Some(owner) {
            return Err(Error::UnknownObject);
        }
        Ok(self.report(participant, generation, inside))
    }
    /// Source-set removal uses the participant's current controllable owner,
    /// not the cached assignment owner. Supply it only when its connection ID
    /// is nonzero, as required by the native removal path.
    pub fn remove(&mut self, participant: u64, current_owner: Option<u64>) -> Vec<Effect> {
        if self.participants.remove(&participant).is_none() {
            return vec![];
        }
        self.controllables.retain(|_, p| *p != participant);
        let mut effects: Vec<_> = current_owner
            .into_iter()
            .map(|owner| Effect::Send {
                owner,
                participant,
                call: Call::Unsubscribe,
            })
            .collect();
        effects.extend(self.membership(participant, false));
        effects
    }
    /// Retire in the source set's enumeration order. Return no partial cleanup
    /// if the supplied source snapshot is incomplete, duplicated or oversized.
    pub fn retire(&mut self, source: &[(u64, Option<u64>)]) -> Result<Vec<Effect>, Error> {
        if source.len() > MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let supplied: BTreeSet<_> = source.iter().map(|(p, _)| *p).collect();
        if supplied.len() != source.len() || supplied != self.participants.keys().copied().collect()
        {
            return Err(Error::Shape);
        }
        let mut effects = Vec::new();
        for &(participant, owner) in source {
            effects.extend(self.remove(participant, owner));
        }
        Ok(effects)
    }
}

#[cfg(test)]
mod tests;
