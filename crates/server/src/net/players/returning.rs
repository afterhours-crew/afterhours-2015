// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The garage return, staged as the official host stages it (E921, both
//! official returns of E847): the confirming request is answered with the
//! return-requested presentation event alone; after its client confirmation the
//! free-roam presentation is deleted and the entry sequence starts with its
//! path; about 1.4 s after the request the garage car is staged (its content
//! ahead), the returning presentation is created and the arriving actor
//! announced. Sending everything in the request's reply made the client stall
//! for about 2.5 s before its presentation reacted (five owned runs, E919 to
//! E921).

use super::*;
use crate::garage_logic::{Part, Which};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    arrival::Arrivals,
    participants::{HostRpc, Outcome},
    sequences::{Completed, Owner},
};

type Entry = (Section, Vec<HostRpc>);
#[cfg(test)]
mod tests;

/// World ms from the confirming request to the teardown (official: the host
/// frame after the one carrying the return-requested event).
pub(in crate::net) const RETURN_TEARDOWN_DELAY_MS: u64 = 100;
/// World ms from the confirming request to the garage car staging (official
/// content registrations at 1.37 s, returning events at 1.44 to 1.47 s).
pub(in crate::net) const RETURN_STAGING_DELAY_MS: u64 = 1_350;
/// How long client calls are logged after a return begins (diagnostics).
pub(in crate::net) const RETURN_TRACE_MS: u64 = 12_000;

/// A return in progress: teardown waits for its owned confirmation and world
/// time; staging follows teardown and its own minimum delay.
#[derive(Clone, Debug)]
pub(in crate::net) struct PendingReturn {
    pub participant: u16,
    /// Intro owns its car and staging; only the confirmation gate is shared.
    pub intro: bool,
    /// World ms of the confirming request.
    pub requested_ms: u64,
    pub torn_down: bool,
    /// Current owned callback; retained through staging for in-flight repeats.
    pub confirmation: Option<nfs_world::logic::Fire>,
    pub confirmed: bool,
    /// Participant replies to the confirming request; sent with the teardown.
    pub chain: Vec<HostRpc>,
    /// Garage slot refreshes; sent after the staged car.
    pub slots: Vec<HostRpc>,
}

impl PendingReturn {
    pub(super) fn confirm(
        &mut self,
        players: &Players,
        persona: u64,
        message: &nfs_world::logic::Message,
    ) -> bool {
        let (
            Some(expected),
            nfs_world::logic::Message::Reached {
                target,
                event,
                player,
            },
        ) = (&self.confirmation, message)
        else {
            return false;
        };
        if expected.target != *target
            || expected.event != *event
            || players.local_slot(HOST_SELECTOR as u8, persona, self.participant) != Some(*player)
        {
            return false;
        }
        self.confirmed = true;
        true
    }
}

pub(super) fn empty_section() -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted: Vec::new(),
        setup: None,
        records: Vec::new(),
    }
}
fn return_section(records: Vec<replication::Record>, deleted: Vec<u16>) -> Section {
    let mut section = empty_section();
    section.setup = Some(Setup::RawEscape([0; 3]));
    section.records = records;
    section.deleted = deleted;
    section
}
impl PlayerListener {
    /// Stage 1, inside the confirming request's transaction: the old driving
    /// context's return-requested event, nothing else.
    pub(super) fn begin_return(
        players: &mut Players,
        returns: &mut Arrivals,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
    ) -> Result<Vec<Entry>, replication::Error> {
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        };
        returns.reset(players, owner)?;
        let requested = Self::presentation_events(
            logic,
            Which::ReturnRequested,
            players,
            ghosts,
            persona,
            participant,
        )?;
        Ok(if requested.is_empty() {
            Vec::new()
        } else {
            vec![(empty_section(), requested)]
        })
    }
    /// Stage 2: delete the free-roam presentation, start the entry sequence
    /// and its path, and deliver the request's participant replies.
    pub(super) fn teardown_return(
        players: &mut Players,
        returns: &mut Arrivals,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
        chain: Vec<HostRpc>,
    ) -> Result<Vec<Entry>, replication::Error> {
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        };
        let produced = Self::arrive_logic_part(
            logic,
            Which::Returning,
            players,
            participant,
            ghosts,
            Part::Deletions,
        )?;
        // The path assignment precedes the sequence on the wire.
        let records = returns.start_rollout(players, owner)?;
        Ok(vec![(return_section(records, produced.deleted), chain)])
    }
    /// Stage 3: stage the garage car (its content ahead, its slot announced
    /// after the creation), create the returning presentation and the
    /// arriving actor, then refresh the garage slots.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stage_return(
        players: &mut Players,
        population: &mut Population,
        inventory: &Inventory,
        content: &GarageContent,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
        slots: Vec<HostRpc>,
    ) -> Result<(Vec<Entry>, Vec<nfs_world::content::Message>), replication::Error> {
        let staged = population.prepare_return(
            players,
            crate::vehicle_content::population::Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            },
            inventory.clone(),
            &content.vehicles,
            &content.layout,
        )?;
        let messages = staged.messages;
        let produced = Self::arrive_logic_part(
            logic,
            Which::Returning,
            players,
            participant,
            ghosts,
            Part::Creations,
        )?;
        let mut records = produced.records;
        records.extend(staged.records);
        records.extend(players.actor_arriving(HOST_SELECTOR as u8, persona, participant)?);
        let mut out = vec![(
            return_section(records, produced.deleted),
            produced
                .events
                .into_iter()
                .map(HostRpc::Event)
                .collect::<Vec<_>>(),
        )];
        // A frame's messages precede its replication records. Announce the
        // replacement slot only after its car creation has been published;
        // the transport deliberately rejects references to unknown vehicles.
        if !staged.bindings.is_empty() {
            out.push((
                empty_section(),
                staged.bindings.into_iter().map(HostRpc::Vehicle).collect(),
            ));
        }
        if !slots.is_empty() {
            out.push((empty_section(), slots));
        }
        Ok((out, messages))
    }
    /// Run the due stages of the pending returns (world ms). A stage without
    /// queue room waits for the next tick; a failed stage is dropped with a
    /// warning (a started sequence still ends on its own timer).
    pub(in crate::net) fn advance_returns(&mut self, world_ms: u64) {
        let mut index = 0;
        while index < self.pending_returns.len() {
            let pending = self.pending_returns[index].clone();
            let teardown_due = pending.confirmed
                && !pending.torn_down
                && world_ms
                    >= pending
                        .requested_ms
                        .saturating_add(RETURN_TEARDOWN_DELAY_MS);
            let staging_due = !pending.intro
                && pending.torn_down
                && world_ms >= pending.requested_ms.saturating_add(RETURN_STAGING_DELAY_MS);
            if !(teardown_due || staging_due) {
                index += 1;
                continue;
            }
            if self.pending_entries.len() + 3 > MAX_PENDING {
                return;
            }
            let result = if teardown_due && pending.intro {
                self.run_intro_return(&pending)
            } else if teardown_due {
                self.run_return_teardown(&pending)
            } else {
                self.run_return_staging(&pending)
            };
            match result {
                // No room in the content queue: retry on the next tick.
                Ok(false) => return,
                Ok(true) if teardown_due => {
                    let entry = &mut self.pending_returns[index];
                    entry.torn_down = true;
                    entry.chain.clear();
                    index += 1;
                }
                Ok(true) => {
                    self.pending_returns.remove(index);
                }
                Err(error) => {
                    tracing::warn!(
                        participant = pending.participant,
                        ?error,
                        teardown = teardown_due,
                        "owned garage return stage failed; the return is dropped"
                    );
                    self.pending_returns.remove(index);
                }
            }
        }
    }
    fn run_intro_return(&mut self, pending: &PendingReturn) -> Result<bool, replication::Error> {
        let mut players = self.players.clone();
        let mut returns = self
            .returns
            .clone()
            .ok_or(replication::Error::Unsupported)?;
        let mut modes = self.intro_modes.clone();
        let mut ghosts = self.logic_ghosts.clone();
        let chain = pending
            .chain
            .iter()
            .map(|call| match call {
                HostRpc::Participant(notification) => Ok(*notification),
                _ => Err(replication::Error::Shape),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Self::confirmed_intro_return(
            self.prologue
                .as_deref()
                .ok_or(replication::Error::Unsupported)?,
            &mut modes,
            &mut players,
            &mut returns,
            self.logic.as_deref(),
            &mut ghosts,
            self.persona,
            pending.participant,
            chain,
        )?;
        let mut story_pending = self.story_pending.clone();
        match self.stage_first_drive_return(
            &players,
            &self.participants,
            out.calls.iter(),
            &mut story_pending,
        ) {
            Ok(()) => {}
            Err(replication::Error::Bound) => return Ok(false),
            Err(error) => return Err(error),
        }
        let mut deferred = Vec::new();
        out.calls = prologue::presentation::present(
            &prologue::presentation::Context {
                logic: self.logic.as_deref(),
                ghosts: &ghosts,
                players: &players,
                participants: &self.participants,
                content: self.prologue.as_deref(),
            },
            out.calls,
            &mut deferred,
        )?;
        let entries = Self::garage_entries(out.records, out.deleted, out.calls)?;
        if entries.len() > MAX_PENDING.saturating_sub(self.pending_entries.len())
            || deferred.len() > MAX_PENDING.saturating_sub(self.pending_intro_presentation.len())
        {
            return Ok(false);
        }
        self.players = players;
        self.returns = Some(returns);
        self.intro_modes = modes;
        self.logic_ghosts = ghosts;
        self.story_pending = story_pending;
        self.pending_intro_presentation.extend(deferred);
        self.queue_entries(entries);
        tracing::info!(
            participant = pending.participant,
            "confirmed intro garage return queued"
        );
        Ok(true)
    }
    fn run_return_teardown(&mut self, pending: &PendingReturn) -> Result<bool, replication::Error> {
        let mut players = self.players.clone();
        let mut returns = self
            .returns
            .clone()
            .ok_or(replication::Error::Unsupported)?;
        let mut ghosts = self.logic_ghosts.clone();
        let entries = Self::teardown_return(
            &mut players,
            &mut returns,
            self.logic.as_deref(),
            &mut ghosts,
            self.persona,
            pending.participant,
            pending.chain.clone(),
        )?;
        self.players = players;
        self.returns = Some(returns);
        self.logic_ghosts = ghosts;
        self.queue_entries(entries);
        tracing::info!(
            participant = pending.participant,
            "owned garage return teardown queued; the entry sequence starts"
        );
        Ok(true)
    }
    fn run_return_staging(&mut self, pending: &PendingReturn) -> Result<bool, replication::Error> {
        let (Some(population), Some(inventory), Some(content)) = (
            self.population.as_ref(),
            self.inventory.as_ref(),
            self.vehicles.as_deref(),
        ) else {
            return Err(replication::Error::Unsupported);
        };
        let mut players = self.players.clone();
        let mut population = population.clone();
        let mut ghosts = self.logic_ghosts.clone();
        let (entries, messages) = Self::stage_return(
            &mut players,
            &mut population,
            inventory,
            content,
            self.logic.as_deref(),
            &mut ghosts,
            self.persona,
            pending.participant,
            pending.slots.clone(),
        )?;
        let content_messages = messages.len();
        if content_messages > MAX_PENDING.saturating_sub(self.pending_content.len()) {
            return Ok(false);
        }
        self.players = players;
        self.population = Some(population);
        self.logic_ghosts = ghosts;
        self.pending_content.extend(messages);
        self.queue_entries(entries);
        tracing::info!(
            participant = pending.participant,
            content_messages,
            "owned garage car staged; the returning presentation queued"
        );
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_return_call(
        body: BitSpan<'_>,
        returns: &mut Arrivals,
        players: &mut Players,
        participants: &mut nfs_world::participants::Lifecycle,
        population: &mut Population,
        inventory: &Inventory,
        content: &GarageContent,
        sequences: &mut nfs_world::sequences::Sequences,
        arrivals: Option<&mut Arrivals>,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        presence: &mut nfs_world::garage::presence::Presence,
        roles: Option<&crate::scene_roles::SceneRoles>,
        persona: u64,
    ) -> Result<Option<Option<(Entry, u16)>>, replication::Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| replication::Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| replication::Error::Shape)?;
        let Some(&ghost) = envelope.references().first() else {
            return Ok(None);
        };
        if returns.acknowledges(ghost, route.selector(), route.method_index()) {
            let &[_, participant] = envelope.references() else {
                return Err(replication::Error::Shape);
            };
            if envelope.words() != [0, 0]
                || !envelope.remaining().is_empty()
                || route.arguments().len() != 7
                || returns.ghost(participant) != Some(ghost)
                || !players.owns_participant(HOST_SELECTOR as u8, persona, participant)
            {
                return Err(replication::Error::Shape);
            }
            return Ok(Some(None));
        }
        if !returns.has_completion(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(None);
        }
        let call = Completed::decode(body)?;
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant: call.participant,
        };
        if returns.complete(players, owner, call)?.is_none() {
            return Ok(Some(None));
        }
        let (mut records, mut deleted) = returns.finish_rollout(players, owner)?;
        records.extend(players.bind_player_vehicle(
            owner.connection,
            persona,
            owner.participant,
            None,
        )?);
        let removed = population.finish_return(
            players,
            crate::vehicle_content::population::Owner {
                connection: owner.connection,
                persona,
                participant: owner.participant,
            },
            inventory,
            &content.layout,
        )?;
        deleted.push(removed);
        sequences.reset(players, owner)?;
        if let Some(arrivals) = arrivals {
            arrivals.reset(players, owner)?;
        }
        let produced = Self::arrive_logic(
            logic,
            crate::garage_logic::Which::Returned,
            players,
            owner.participant,
            ghosts,
        )?;
        records.extend(produced.records);
        deleted.extend(produced.deleted);
        let Outcome::Advanced(notifications) = participants.returned_to_garage(owner.participant)
        else {
            return Err(replication::Error::Shape);
        };
        let mut chain: Vec<_> = notifications
            .into_iter()
            .map(HostRpc::Participant)
            .collect();
        let endpoint = Self::presence_endpoint(roles, players)?;
        if let Some(present) = presence.set(endpoint, owner.participant, true, |p| {
            players.owns_participant(owner.connection, persona, p)
        })? {
            // Presence enters between BeginGarage enter/leave.
            let at = chain.len().saturating_sub(2);
            chain.insert(at, HostRpc::GaragePresence(present));
        }
        chain.extend(produced.events.into_iter().map(HostRpc::Event));
        let mut section = empty_section();
        section.setup = Some(Setup::RawEscape([0; 3]));
        section.records = records;
        section.deleted = deleted;
        Ok(Some(Some(((section, chain), removed))))
    }
}
