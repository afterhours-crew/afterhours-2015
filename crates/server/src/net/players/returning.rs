// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
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
fn empty_section() -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted: Vec::new(),
        setup: None,
        records: Vec::new(),
    }
}
impl PlayerListener {
    /// The Enter Garage prompt request (startup field 100): only the
    /// presentation's old-context stop goes out now. The official host keeps
    /// the participant in free roam until the car enters the garage volume.
    pub(super) fn request_return(
        logic: Option<&crate::garage_logic::GarageLogic>,
        players: &Players,
        ghosts: &BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
    ) -> Result<Vec<Entry>, replication::Error> {
        let requested = Self::presentation_events(
            logic,
            crate::garage_logic::Which::ReturnRequested,
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
    /// Leave free roam for the prompted participant and begin the return;
    /// `Unsupported` when the deployment lacks return content.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn garage_entered(
        players: &mut Players,
        participants: &mut nfs_world::participants::Lifecycle,
        returns: Option<&mut Arrivals>,
        population: Option<&mut Population>,
        inventory: Option<&Inventory>,
        content: Option<&GarageContent>,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
    ) -> Result<Vec<Entry>, replication::Error> {
        let (Some(returns), Some(population), Some(inventory), Some(content)) =
            (returns, population, inventory, content)
        else {
            return Err(replication::Error::Unsupported);
        };
        let Outcome::Advanced(chain) = participants.enter_garage(participant) else {
            return Err(replication::Error::Shape);
        };
        tracing::info!(
            participant,
            "car entered the garage volume after the prompt; garage return begins"
        );
        Self::begin_return(
            players,
            returns,
            population,
            inventory,
            content,
            logic,
            ghosts,
            persona,
            participant,
            chain.into_iter().map(HostRpc::Participant).collect(),
        )
    }
    /// The car entered the garage volume after the prompt: stage the
    /// replacement garage car, start the entry sequence and leave free roam.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_return(
        players: &mut Players,
        returns: &mut Arrivals,
        population: &mut Population,
        inventory: &Inventory,
        content: &GarageContent,
        logic: Option<&crate::garage_logic::GarageLogic>,
        ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        participant: u16,
        mut chain: Vec<HostRpc>,
    ) -> Result<Vec<Entry>, replication::Error> {
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        };
        returns.reset(players, owner)?;
        let mut out = Vec::new();
        let staged = population.prepare_return(
            players,
            crate::vehicle_content::population::Owner {
                connection: owner.connection,
                persona,
                participant,
            },
            inventory.clone(),
            &content.vehicles,
            &content.layout,
        )?;
        if !staged.messages.is_empty() {
            return Err(replication::Error::Unsupported);
        }
        let produced = Self::arrive_logic(
            logic,
            crate::garage_logic::Which::Returning,
            players,
            participant,
            ghosts,
        )?;
        let mut records = produced.records;
        records.extend(staged.records);
        records.extend(players.actor_arriving(owner.connection, persona, participant)?);
        records.extend(returns.start_rollout(players, owner)?);
        chain.extend(produced.events.into_iter().map(HostRpc::Event));
        let mut section = empty_section();
        section.setup = Some(Setup::RawEscape([0; 3]));
        section.records = records;
        section.deleted = produced.deleted;
        out.push((section, chain));
        // A frame's messages precede its replication records. Announce the
        // replacement slot only after its car creation has been published;
        // the transport deliberately rejects references to unknown vehicles.
        if !staged.bindings.is_empty() {
            out.push((
                empty_section(),
                staged.bindings.into_iter().map(HostRpc::Vehicle).collect(),
            ));
        }
        Ok(out)
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
                || route.arguments().read_u32(0, 7) != Ok(0)
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
