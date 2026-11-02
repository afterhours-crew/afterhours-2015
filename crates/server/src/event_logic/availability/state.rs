// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Content, copies};
use nfs_content::events::discovery::{
    Endpoint,
    channels::Signal,
    membership::{Fact, Program},
};
use nfs_world::{participant_states::MAX_PARTICIPANTS, scoring::Error};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Participant {
    pub notifications: BTreeSet<Endpoint>,
    pub eligible: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub struct State {
    content: Arc<Content>,
    copies: copies::State,
}
impl State {
    pub fn new(content: Arc<Content>) -> Result<Self, Error> {
        let a = content.definitions();
        let copies = copies::State::new(
            &a.copies
                .iter()
                .map(|(id, c)| (id.clone(), copies::Definition::from(c)))
                .collect(),
            &a.channels,
        )?;
        Ok(Self { content, copies })
    }
    pub fn handles(&self, signal: &Signal) -> bool {
        self.copies.handles(signal)
    }
    /// Reconcile the complete human roster and current external facts before
    /// applying ordered authored control signals. Missing input or an invalid
    /// signal leaves every copied membership unchanged. The caller publishes
    /// only a successful result and tracks queue acceptance separately.
    pub fn update(
        &mut self,
        players: &BTreeSet<u64>,
        signals: &[Signal],
        mut external: impl FnMut(u64, &Fact) -> Option<bool>,
    ) -> Result<BTreeMap<u64, Participant>, Error> {
        if players.len() > MAX_PARTICIPANTS || players.contains(&0) || signals.len() > 64 {
            return Err(Error::Bound);
        }
        let mut next = self.copies.clone();
        let mut inputs = BTreeMap::new();
        for (id, program) in self.content.inputs() {
            let mut members = BTreeSet::new();
            for &player in players {
                if evaluate(program, player, &next, &mut external)? {
                    members.insert(player);
                }
            }
            inputs.insert(id.clone(), members);
        }
        next.sync(&inputs)?;
        for signal in signals {
            next.signal(signal)?;
        }
        let mut result = BTreeMap::new();
        let a = self.content.definitions();
        for &player in players {
            let mut membership = BTreeMap::new();
            for binding in &a.memberships {
                membership.insert(
                    binding.notification,
                    evaluate(&binding.program, player, &next, &mut external)?,
                );
            }
            let mut eligible = BTreeSet::new();
            for (id, rule) in &a.visibility {
                if rule
                    .evaluate(|site| membership.get(&site).copied())
                    .map_err(|_| Error::MissingInput)?
                {
                    eligible.insert(id.clone());
                }
            }
            result.insert(
                player,
                Participant {
                    notifications: membership
                        .into_iter()
                        .filter_map(|(site, member)| member.then_some(site))
                        .collect(),
                    eligible,
                },
            );
        }
        self.copies = next;
        Ok(result)
    }
}
fn evaluate(
    program: &Program,
    player: u64,
    copies: &copies::State,
    external: &mut impl FnMut(u64, &Fact) -> Option<bool>,
) -> Result<bool, Error> {
    program
        .evaluate(|fact| match fact {
            Fact::Copy(id) => copies.contains(id, player).ok(),
            Fact::Mutable(id) if program.empty_sets().contains(id) => Some(false),
            _ => external(player, fact),
        })
        .map_err(|_| Error::MissingInput)
}
