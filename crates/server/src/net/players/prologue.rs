// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::sequence_content::authored::Instances;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    participants::intro::Chapter,
    participants::{HostRpc, Lifecycle, Outcome, Stage},
    sequences::{Completed, Owner},
    teleport::Command,
};
type Ready = (Vec<replication::Record>, Vec<HostRpc>, Command);
type Finished = (Section, Vec<HostRpc>);
pub(super) type Modes = BTreeMap<
    u16,
    (
        crate::prologue::driving::Active,
        crate::prologue::driving::Active,
        Chapter,
    ),
>;
pub(super) mod finale;
pub(super) mod garage;
pub(super) mod movies;
pub(super) mod presentation;
pub(super) mod selection;
mod story;
#[cfg(test)]
mod tests;
pub(super) mod volumes;

pub(super) fn retirement(
    out: crate::prologue::driving::Output,
) -> Result<Vec<Finished>, replication::Error> {
    let mut entries = super::traffic::ai_departures(&out.departures);
    // The RawEscape setup costs ~100 bits more than the Packed one the
    // splitter budgets for.
    for mut section in split_scenes(
        out.records,
        nfs_world::application::OUTBOUND_FRAME_BITS - 96,
    )? {
        section.setup = Some(Setup::RawEscape([0; 3]));
        entries.push((section, vec![]));
    }
    entries.extend(super::traffic::ai_departures(&out.calls));
    let mut section = returning::empty_section();
    section.deleted = out.deleted;
    entries.push((section, vec![]));
    Ok(entries)
}

impl PlayerListener {
    /// Driving output as wire sections: records split to frame size, each
    /// with a setup; deletions ride with the first one, or alone.
    pub(super) fn driving_sections(
        records: Vec<replication::Record>,
        deleted: Vec<u16>,
    ) -> Result<Vec<Section>, replication::Error> {
        // Each deletion costs a flag and a 13-bit id in the first section.
        let budget = (nfs_world::application::OUTBOUND_FRAME_BITS - 96)
            .checked_sub(14 * deleted.len())
            .ok_or(replication::Error::Bound)?;
        let mut sections = split_scenes(records, budget)?;
        match sections.first_mut() {
            Some(first) => first.deleted = deleted,
            None if !deleted.is_empty() => {
                let mut section = returning::empty_section();
                section.deleted = deleted;
                sections.push(section);
            }
            None => {}
        }
        for section in &sections {
            if section.frame()?.len() > nfs_world::application::OUTBOUND_FRAME_BITS {
                return Err(replication::Error::Bound);
            }
        }
        Ok(sections)
    }
    /// The intro garage-entry completion as wire entries: records split to
    /// frame size with the deletions on the first section, and the calls
    /// distributed so every entry fits one outbound frame. E1186 packed the
    /// whole completion into a single entry; with installed content it
    /// exceeded `OUTBOUND_FRAME_BITS`, and the exit-entry check then refused
    /// the client's resent completion forever (black screen after the
    /// cinematic).
    pub(super) fn garage_entries(
        records: Vec<replication::Record>,
        deleted: Vec<u16>,
        calls: Vec<HostRpc>,
    ) -> Result<Vec<Finished>, replication::Error> {
        // The timed splitter budgets Packed setups with the session's
        // timestamp included; RawEscape setups cost 96 bits more and each
        // deletion a flag and a 13-bit id. Every fit below is measured as the
        // session admits it, so an untimed fallback section is counted with
        // the timestamp the session adds to it.
        let budget = (nfs_world::application::OUTBOUND_FRAME_BITS - 110)
            .checked_sub(14 * deleted.len())
            .ok_or(replication::Error::Bound)?;
        let mut sections = split_scenes(records, budget)?;
        for section in &mut sections {
            section.setup = Some(Setup::RawEscape([0; 3]));
        }
        if sections.is_empty() && (!deleted.is_empty() || !calls.is_empty()) {
            sections.push(returning::empty_section());
        }
        if let Some(first) = sections.first_mut() {
            first.deleted = deleted;
        }
        if sections.is_empty() {
            return Ok(Vec::new());
        }
        if !sections.iter().all(|section| entry_admitted(section, &[])) {
            return Err(replication::Error::Bound);
        }
        // Calls ride the sections in order, as many as fit (a reply group
        // carries at most fifteen); calls left over get call-only entries.
        // Every section is kept: returning once the calls ran out dropped the
        // record sections after the last call.
        let mut remaining = calls.as_slice();
        let mut entries: Vec<Finished> = Vec::with_capacity(sections.len());
        for section in sections {
            let chunk = fitting_calls(&section, &mut remaining);
            entries.push((section, chunk));
        }
        while !remaining.is_empty() {
            let section = returning::empty_section();
            let chunk = fitting_calls(&section, &mut remaining);
            if chunk.is_empty() {
                // Not even one call fits an empty section.
                return Err(replication::Error::Bound);
            }
            entries.push((section, chunk));
        }
        defer_deletions(&mut entries);
        Ok(entries)
    }
    pub(super) fn advance_prologue(&mut self) -> Result<(), replication::Error> {
        self.advance_story_movies()?;
        let Some(content) = self.prologue.as_deref() else {
            return Ok(());
        };
        let Some(vehicles) = self.vehicles.as_deref() else {
            return Err(replication::Error::Unsupported);
        };
        if self.session_backlog >= MAX_PENDING || !self.pending_entries.is_empty() {
            return Ok(());
        }
        let Some(seconds) = self.event_seconds(self.world_ms) else {
            return Ok(());
        };
        let mut players = self.players.clone();
        let mut population = self
            .population
            .clone()
            .ok_or(replication::Error::Unsupported)?;
        let mut opening = self.opening.clone();
        let mut selection = self.selection.clone();
        let mut selection_execution = self.selection_execution.clone();
        let mut modes = self.intro_modes.clone();
        let mut story_pending = self.story_pending.clone();
        let mut composites = self.intro_composites.clone();
        let mut ai_cursor = self.event_state.ai_cursor.clone();
        let mut participants = self.participants.clone();
        let mut world_volumes = self.world_volumes.clone();
        let mut world_activation = self.world_activation.clone();
        let mut cars = self.world_cars.clone();
        let mut records = Vec::new();
        let mut messages = Vec::new();
        let mut calls = Vec::new();
        let mut after_creation = Vec::new();
        let mut deleted = Vec::new();
        let mut teleports = Vec::new();
        let mut driving_presentation = BTreeSet::new();
        if let Some(composite) = &content.final_composite {
            for active in composites.values_mut() {
                if let Some(stop) = active.advance(composite, &players, seconds)? {
                    calls.push(HostRpc::SequenceStop(stop));
                }
            }
        }
        {
            for (active, _, chapter) in modes.values_mut() {
                let out = active.advance(content.mode(*chapter)?, &mut players, seconds)?;
                records.extend(out.records);
                calls.extend(out.calls);
                deleted.extend(out.deleted);
            }
        }
        let owners = players.player_participants().collect::<Vec<_>>();
        for participant in owners {
            let owner = Owner {
                connection: HOST_SELECTOR as u8,
                persona: self.persona,
                participant,
            };
            if participants.intro_participants().any(|p| p == participant)
                && let Some(loading) = &content.loading
                && let Some(record) = loading.spawn(&mut players, owner)?
            {
                records.push(record);
            }
            let chapter = participants.intro_chapter(participant);
            match self.participants.stage(participant) {
                Some(Stage::FreeRoam)
                    if chapter == Some(Chapter::SecondRace)
                        && content.return_drive.is_some()
                        && content.diner_movie.as_ref().is_some_and(|movie| {
                            self.intro_movies.get(&participant).is_some_and(|active| {
                                active.objective() == movie.data.objective
                                    && active.phase() == nfs_world::fmv::Phase::Finished
                            })
                        })
                        && modes
                            .get(&participant)
                            .is_some_and(|m| m.2 == Chapter::SecondRace && m.0.has_left())
                        && self.pending.is_empty()
                        && self.pending_rpcs.is_empty() =>
                {
                    let inventory = self
                        .inventory
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?;
                    let item =
                        inventory.slots.values()[0].ok_or(replication::Error::UnknownObject)?;
                    let definition = inventory
                        .items
                        .items
                        .get(&item)
                        .ok_or(replication::Error::UnknownObject)?
                        .definition;
                    let car = vehicles
                        .vehicles
                        .definition(&definition)
                        .ok_or(replication::Error::Unsupported)?;
                    let vehicle = players
                        .owned_vehicle(owner.connection, owner.persona, participant, item)
                        .ok_or(replication::Error::UnknownObject)?;
                    if cars.get(&vehicle) != Some(&(participant, true)) {
                        return Err(replication::Error::Shape);
                    }
                    opening.retire_completed(&players, owner)?;
                    let (drive, output) = crate::prologue::driving::Active::start_with_car(
                        content.mode(Chapter::ReturnDrive)?,
                        &mut players,
                        owner,
                        seconds,
                        crate::event_logic::car_id(&car.blueprint),
                    )?;
                    let (_, canceller, _) = modes
                        .remove(&participant)
                        .ok_or(replication::Error::UnknownObject)?;
                    let Outcome::Advanced(chain) =
                        participants.intro_return_drive_begin(participant)
                    else {
                        return Err(replication::Error::Shape);
                    };
                    records.extend(output.records);
                    deleted.extend(output.deleted);
                    calls.extend(output.calls);
                    calls.extend(chain.into_iter().map(HostRpc::Participant));
                    modes.insert(participant, (drive, canceller, Chapter::ReturnDrive));
                }
                Some(Stage::FreeRoam)
                    if chapter == Some(Chapter::FirstRace)
                        && content.second_race.is_some()
                        && self
                            .intro_movies
                            .get(&participant)
                            .is_some_and(|m| m.phase() == nfs_world::fmv::Phase::Finished)
                        && modes.get(&participant).is_some_and(|m| m.0.has_left())
                        && self.pending.is_empty()
                        && self.pending_rpcs.is_empty() =>
                {
                    opening.retire_completed(&players, owner)?;
                    let Outcome::Advanced(chain) =
                        participants.intro_second_race_begin(participant)
                    else {
                        return Err(replication::Error::Shape);
                    };
                    calls.extend(chain.into_iter().map(HostRpc::Participant));
                }
                Some(Stage::IntroRelocating)
                    if self.pending.is_empty() && self.pending_rpcs.is_empty() =>
                {
                    let item = self
                        .inventory
                        .as_ref()
                        .and_then(|i| i.slots.values()[0])
                        .ok_or(replication::Error::UnknownObject)?;
                    let vehicle = players
                        .owned_vehicle(owner.connection, owner.persona, participant, item)
                        .ok_or(replication::Error::UnknownObject)?;
                    if cars.get(&vehicle) != Some(&(participant, true)) {
                        return Err(replication::Error::Shape);
                    }
                    let pose = content
                        .second_race
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?
                        .destination
                        .teleport;
                    let command = Command {
                        endpoint: participants
                            .intro_bindings()
                            .ok_or(replication::Error::Shape)?
                            .teleport,
                        participant,
                        vehicle,
                        position: pose.position,
                        basis: pose.basis,
                        speed: 0.,
                    };
                    command.encode()?;
                    let Outcome::Advanced(chain) = participants.intro_relocation_ready(participant)
                    else {
                        return Err(replication::Error::Shape);
                    };
                    calls.extend(chain.into_iter().map(HostRpc::Participant));
                    teleports.push((self.world_ms, command));
                }
                Some(Stage::IntroSpawning) if !cars.values().any(|(p, _)| *p == participant) => {
                    if cars.len() >= MAX_WORLD_CARS {
                        return Err(replication::Error::Bound);
                    }
                    let vehicle_owner = crate::vehicle_content::population::Owner {
                        connection: owner.connection,
                        persona: owner.persona,
                        participant,
                    };
                    let spawned = if chapter == Some(Chapter::FirstRace) {
                        population.spawn_world(
                            &mut players,
                            vehicle_owner,
                            self.inventory
                                .as_ref()
                                .ok_or(replication::Error::Unsupported)?,
                            &vehicles.vehicles,
                            &vehicles.layout,
                        )?
                    } else {
                        population.spawn_temporary_world(
                            &mut players,
                            vehicle_owner,
                            content.temporary_vehicle(vehicles)?,
                            &vehicles.vehicles,
                        )?
                    };
                    // A newly registered vehicle bundle can precede its car in
                    // the creation batch. Resolve the owned car, not record 0.
                    let ghost = if chapter == Some(Chapter::FirstRace) {
                        let item = self
                            .inventory
                            .as_ref()
                            .and_then(|i| i.slots.values()[0])
                            .ok_or(replication::Error::UnknownObject)?;
                        players.owned_vehicle(owner.connection, owner.persona, participant, item)
                    } else {
                        players.temporary_vehicle(owner.connection, owner.persona, participant)
                    }
                    .ok_or(replication::Error::UnknownObject)?;
                    cars.insert(ghost, (participant, false));
                    records.extend(spawned.records);
                    messages.extend(spawned.messages);
                    deleted.extend(spawned.deleted);
                }
                Some(Stage::IntroOpening) => {
                    let chapter = chapter.ok_or(replication::Error::Shape)?;
                    let arrival = content.arrival(chapter)?;
                    let record = if chapter != Chapter::FirstDrive {
                        opening.start_stopped(arrival, &mut players, owner)?
                    } else {
                        opening.start(arrival, &mut players, owner)?
                    };
                    if let Some(record) = record {
                        records.push(record);
                        let bindings = participants
                            .intro_bindings()
                            .ok_or(replication::Error::Shape)?;
                        after_creation.push(HostRpc::Participant(
                            nfs_world::participants::Notification {
                                endpoint: bindings.opening,
                                participant,
                                call: nfs_world::participants::Call::Enter,
                            },
                        ));
                    }
                }
                Some(Stage::IntroStartingDrive)
                    if modes
                        .get(&participant)
                        .is_none_or(|pair| Some(pair.2) != chapter) =>
                {
                    if modes
                        .get(&participant)
                        .is_some_and(|pair| !pair.0.has_left())
                    {
                        continue;
                    }
                    let Some((_, canceller_content)) = content.driving.as_ref() else {
                        continue;
                    };
                    if !modes.contains_key(&participant)
                        && modes.len() >= nfs_world::sequences::MAX_INSTANCES
                    {
                        return Err(replication::Error::Bound);
                    };
                    let chapter = chapter.ok_or(replication::Error::Shape)?;
                    let (mut drive, a) = crate::prologue::driving::Active::start(
                        content.mode(chapter)?,
                        &mut players,
                        owner,
                        seconds,
                    )?;
                    let (canceller, b) = if let Some((_, canceller, _)) = modes.remove(&participant)
                    {
                        (canceller, crate::prologue::driving::Output::default())
                    } else {
                        crate::prologue::driving::Active::start(
                            canceller_content,
                            &mut players,
                            owner,
                            seconds,
                        )?
                    };
                    if !a.deleted.is_empty() || !b.deleted.is_empty() {
                        return Err(replication::Error::Shape);
                    };
                    records.extend(a.records);
                    records.extend(b.records);
                    // The objects exist immediately, but their launch callbacks
                    // follow the participant's ordered world subscriptions.
                    // Reserve all of it on clones: queue pressure cannot expose
                    // Alive or a mode callback without the preceding memberships.
                    calls.extend(world_volumes.before_alive(
                        &content.volumes,
                        &players,
                        participant,
                    )?);
                    let Outcome::Advanced(alive) = participants.intro_drive_ready(participant)
                    else {
                        return Err(replication::Error::Shape);
                    };
                    calls.extend(alive.into_iter().map(HostRpc::Participant));
                    if let Some(activation) = &mut world_activation {
                        calls.extend(
                            activation
                                .set(participant, true, nfs_world::session::MAX_RPC_QUEUE, |id| {
                                    players.owns_participant(owner.connection, owner.persona, id)
                                })?
                                .into_iter()
                                .map(HostRpc::WorldActivation),
                        );
                    }
                    calls.extend(world_volumes.after_alive(
                        &content.volumes,
                        &players,
                        participant,
                    )?);
                    calls.extend(a.calls);
                    calls.extend(world_volumes.after_drive(
                        &content.volumes,
                        &players,
                        participant,
                    )?);
                    calls.extend(b.calls);
                    if chapter == Chapter::FirstDrive {
                        driving_presentation.insert(participant);
                    }
                    if let Some(ai) = content.mode(chapter)?.opponents() {
                        let previous = population.registrations().clone();
                        let mut registrations = previous.clone();
                        let selected = ai_cursor.select(ai)?;
                        let mut created = ai.create(
                            &mut players,
                            &mut registrations,
                            owner.connection,
                            owner.persona,
                            &selected,
                        )?;
                        let admitted = drive.admit(
                            content.mode(chapter)?,
                            &mut players,
                            &mut created.racers,
                        )?;
                        population.update_registrations(&previous, registrations)?;
                        records.extend(created.records);
                        messages.extend(created.messages);
                        calls.extend(created.calls);
                        records.extend(admitted.records);
                        deleted.extend(admitted.deleted);
                        calls.extend(admitted.calls);
                    }
                    modes.insert(participant, (drive, canceller, chapter));
                }
                Some(Stage::IntroEnteringGarage) if chapter == Some(Chapter::ReturnDrive) => {
                    let output = population.prepare_return(
                        &mut players,
                        crate::vehicle_content::population::Owner {
                            connection: owner.connection,
                            persona: owner.persona,
                            participant,
                        },
                        self.inventory
                            .as_ref()
                            .ok_or(replication::Error::Unsupported)?
                            .clone(),
                        &vehicles.vehicles,
                        &vehicles.layout,
                    )?;
                    records.extend(output.records);
                    deleted.extend(output.deleted);
                    messages.extend(output.messages);
                    calls.extend(output.bindings.into_iter().map(HostRpc::Vehicle));
                }
                // The final visit waits for its queued garage composite. It
                // must never enter the initial starter-selection sequence.
                Some(Stage::IntroLoadingGarage) if chapter == Some(Chapter::ReturnDrive) => {
                    if let Some(waiting) = &self.sequences
                        && waiting.is_complete(participant)
                        && modes
                            .get(&participant)
                            .is_some_and(|pair| pair.0.has_left())
                        && let Some(composite) = &content.final_composite
                    {
                        if composites.len() >= nfs_world::sequences::MAX_INSTANCES
                            || composites.contains_key(&participant)
                        {
                            return Err(replication::Error::Bound);
                        }
                        let (key, serializer) = composite.data.sequence.state;
                        let (scene, selector) = players
                            .objects()
                            .scene_endpoint(key, serializer)
                            .ok_or(replication::Error::UnknownObject)?;
                        let serial = crate::event_logic::call(
                            players.objects(),
                            scene,
                            selector,
                            0,
                            Some(participant),
                            nfs_world::game_mode::Arguments::None,
                        )?
                        .serial;
                        let Outcome::Advanced(chain) = participants.intro_final_garage_ready(
                            participant,
                            nfs_world::participants::Endpoint {
                                scene,
                                selector,
                                serial,
                            },
                        ) else {
                            return Err(replication::Error::Shape);
                        };
                        let ghost = waiting
                            .ghost(participant)
                            .ok_or(replication::Error::UnknownObject)?;
                        players.remove_entities(&[ghost])?;
                        deleted.push(ghost);
                        let (active, record) = crate::prologue::composite::Active::start(
                            composite,
                            &mut players,
                            owner,
                            seconds,
                        )?;
                        composites.insert(participant, active);
                        records.push(record);
                        calls.extend(chain.into_iter().map(HostRpc::Participant));
                    }
                }
                Some(Stage::IntroEnteringGarage | Stage::IntroLoadingGarage) => {
                    if let Some(previews) = &content.previews {
                        let out = population.spawn_previews(
                            &mut players,
                            crate::vehicle_content::population::Owner {
                                connection: owner.connection,
                                persona: owner.persona,
                                participant,
                            },
                            previews.clone(),
                            &vehicles.vehicles,
                        )?;
                        records.extend(out.records);
                        messages.extend(out.messages);
                        calls.extend(out.bindings.into_iter().map(HostRpc::Vehicle));
                    }
                    if self.participants.stage(participant) == Some(Stage::IntroLoadingGarage)
                        && let Some(waiting) = &self.sequences
                        && waiting.is_complete(participant)
                        && let Some(authored) = &content.selection
                    {
                        let ghost = waiting
                            .ghost(participant)
                            .ok_or(replication::Error::UnknownObject)?;
                        players.remove_entities(&[ghost])?;
                        deleted.push(ghost);
                        let Outcome::Advanced(chain) = participants.intro_garage_ready(participant)
                        else {
                            return Err(replication::Error::Shape);
                        };
                        let mut out = crate::prologue::driving::Output::default();
                        garage::notify(content, &mut modes, &mut players, chain, &mut out)?;
                        records.extend(out.records);
                        deleted.extend(out.deleted);
                        calls.extend(out.calls);
                        if let Some(record) = selection.start(authored, &mut players, owner)? {
                            if let Some(program) = &content.selection_program {
                                if selection_execution.len() >= nfs_world::sequences::MAX_INSTANCES
                                {
                                    return Err(replication::Error::Bound);
                                }
                                let slot = players
                                    .local_slot(owner.connection, owner.persona, participant)
                                    .ok_or(replication::Error::UnknownObject)?;
                                let (execution, output) =
                                    crate::sequence_content::execution::Execution::start(
                                        program.clone(),
                                        record.id,
                                        participant,
                                        slot,
                                        seconds,
                                    )
                                    .map_err(|_| replication::Error::Shape)?;
                                if !output.commands.is_empty() {
                                    return Err(replication::Error::Unsupported);
                                }
                                for message in output.remote {
                                    let nfs_world::logic::Message::Fire {
                                        events,
                                        player,
                                        target,
                                    } = message
                                    else {
                                        return Err(replication::Error::Shape);
                                    };
                                    calls.extend(events.into_iter().map(|event| {
                                        HostRpc::Event(nfs_world::logic::Fire {
                                            event,
                                            player,
                                            target,
                                        })
                                    }));
                                }
                                if selection_execution
                                    .insert(participant, selection::State::new(execution))
                                    .is_some()
                                {
                                    return Err(replication::Error::DuplicateObject);
                                }
                            }
                            records.push(record);
                        }
                    }
                }
                _ => {}
            }
            // One owner per tick keeps a busy multi-player session within the
            // same bounded content/replication batch as a single-player start.
            if !records.is_empty() || !messages.is_empty() {
                break;
            }
        }
        if let Some(loading) = &content.loading {
            records.extend(loading.sync(
                &mut players,
                &participants,
                HOST_SELECTOR as u8,
                self.persona,
            )?);
        }
        let scene_membership = self.stage_scene_membership(&mut players, &mut participants)?;
        records.extend(scene_membership.records);
        deleted.extend(scene_membership.deleted);
        // A staged car is constructed with an absolute zero origin. Reserve
        // space for that origin rather than inheriting a prior movement frame.
        let mut sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS - 96)?;
        for section in &mut sections {
            section.setup = Some(Setup::RawEscape([0; 3]));
        }
        if !deleted.is_empty() {
            sections.push(Section {
                float_bits: None,
                flag: false,
                setup: None,
                records: vec![],
                deleted,
            });
        }
        let calls = presentation::present_with_driving_tail(
            &presentation::Context {
                logic: self.logic.as_deref(),
                ghosts: &self.logic_ghosts,
                players: &players,
                participants: &participants,
                content: Some(content),
            },
            calls,
            &driving_presentation,
        )?;
        // Keep the state entry in a later replication entry. Ordinary RPCs
        // can be moved ahead of queued creations when their explicit object
        // references already exist; this dependency is on the sequence too.
        let mut entries = Vec::new();
        if !after_creation.is_empty() {
            entries.extend(
                std::mem::take(&mut sections)
                    .into_iter()
                    .map(|s| (s, vec![])),
            );
            entries.extend(admitted_entries(
                returning::empty_section(),
                after_creation,
            )?);
        }
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE {
            // This transaction can never fit, even after pressure clears.
            return Err(replication::Error::Bound);
        }
        if sections.len() > MAX_PENDING.saturating_sub(self.pending.len())
            || entries.len() > MAX_PENDING.saturating_sub(self.pending_entries.len())
            || teleports.len() > MAX_PENDING.saturating_sub(self.pending_teleports.len())
            || messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
            || calls.len()
                > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Ok(());
        }
        match self.stage_story_results(content, &players, &mut modes, &mut story_pending) {
            Err(replication::Error::Bound) => return Ok(()),
            value => value?,
        }
        match self.stage_composite_results(&players, &mut composites, &mut story_pending) {
            Err(replication::Error::Bound) => return Ok(()),
            value => value?,
        }
        if let (Some(binding), Some(progression), Some(loaded)) = (
            &content
                .driving
                .as_ref()
                .ok_or(replication::Error::Unsupported)?
                .1
                .data
                .shutdown,
            &self.progression,
            &self.persistent,
        ) {
            let restored = progression
                .catalog
                .restore(loaded, progression.settings)
                .map_err(|_| replication::Error::Shape)?;
            if restored
                .branch(crate::progression::Branch::Intro)
                .objectives()
                .get(&binding.objective)
                .is_some_and(|s| s.completed)
            {
                for (&participant, (drive, canceller, chapter)) in &modes {
                    if *chapter == Chapter::ReturnDrive
                        && drive.has_left()
                        && canceller.has_left()
                        && composites.get(&participant).is_some_and(|c| c.finished())
                        && participants.finish_intro(participant) == Outcome::Unsupported
                    {
                        return Err(replication::Error::Shape);
                    }
                }
            }
        }
        self.story_pending = story_pending;
        self.intro_composites = composites;
        self.players = players;
        self.population = Some(population);
        self.opening = opening;
        self.selection = selection;
        self.selection_execution = selection_execution;
        self.intro_modes = modes;
        self.event_state.ai_cursor = ai_cursor;
        self.participants = participants;
        self.world_volumes = world_volumes;
        self.world_activation = world_activation;
        self.scene_membership = scene_membership.session;
        self.world_cars = cars;
        self.pending.extend(sections);
        self.queue_entries(entries);
        self.pending_content.extend(messages);
        self.pending_rpcs.extend(calls);
        self.pending_teleports.extend(teleports);
        Ok(())
    }
    pub(super) fn driving_signal(
        content: &crate::prologue::Content,
        modes: &mut Modes,
        players: &Players,
        message: &nfs_world::logic::Message,
        responses: &mut Vec<HostRpc>,
    ) -> Result<bool, replication::Error> {
        for (active, _, chapter) in modes.values_mut() {
            if let Some(calls) = active.signal(content.mode(*chapter)?, players, message)? {
                responses.extend(calls);
                return Ok(true);
            };
        }
        Ok(false)
    }
    pub(super) fn driving_input(
        content: &crate::prologue::Content,
        vehicles: Option<&crate::vehicle_content::GarageContent>,
        inventory: Option<&Inventory>,
        modes: &mut Modes,
        players: &mut Players,
        seconds: Option<f64>,
        body: BitSpan<'_>,
    ) -> Result<Option<crate::prologue::driving::Output>, replication::Error> {
        if modes.is_empty() {
            return Ok(None);
        };
        let seconds = seconds.ok_or(replication::Error::Shape)?;
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
        if route.selector() == 0
            && route.method_index() == 0
            && !modes.values().any(|(a, _, _)| a.ghost == ghost)
            && let Some((_, canceller, _)) = modes.values_mut().find(|(_, c, _)| c.ghost == ghost)
        {
            let leave = nfs_world::game_mode::Leave::decode(body)
                .map_err(|_| replication::Error::Shape)?
                .ok_or(replication::Error::Shape)?;
            return Ok(Some(
                canceller.leave(
                    &content
                        .driving
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?
                        .1,
                    players,
                    leave.participant,
                )?,
            ));
        }
        if route.selector() == 0
            && route.method_index() == 0
            && let Some((active, _, chapter)) =
                modes.values_mut().find(|(a, _, _)| a.ghost == ghost)
        {
            let leave = nfs_world::game_mode::Leave::decode(body)
                .map_err(|_| replication::Error::Shape)?
                .ok_or(replication::Error::Shape)?;
            // Leave's seven remaining bits align the method to a byte. The
            // shared decoder validates their length; native writers need not
            // zero them.
            return Ok(Some(active.leave(
                content.mode(*chapter)?,
                players,
                leave.participant,
            )?));
        }
        if matches!(route.method_index(), 2 | 3)
            && modes.values().any(|(_, _, chapter)| {
                content.mode(*chapter).is_ok_and(|drive| {
                    drive.data.game_mode.checkpoints.iter().any(|&(k, s)| {
                        players.objects().scene_endpoint(k, s) == Some((ghost, route.selector()))
                    })
                })
            })
        {
            let crossing = nfs_world::game_mode::Crossing::decode(body)
                .map_err(|_| replication::Error::Shape)?
                .ok_or(replication::Error::Shape)?;
            let Some((active, _, chapter)) = modes.values_mut().find_map(|pair| {
                pair.0
                    .owns_vehicle(players, crossing.vehicle)
                    .then_some(pair)
            }) else {
                return Ok(Some(crate::prologue::driving::Output::default()));
            };
            return Ok(Some(active.crossing(
                content.mode(*chapter)?,
                players,
                crossing,
                seconds,
            )?));
        }
        let Some((active, _, chapter)) = modes
            .values_mut()
            .find(|(a, _, _)| a.sequence_owner(ghost, route.selector()))
        else {
            return Ok(None);
        };
        if route.method_index() != 1 {
            return Ok(None);
        };
        let call = Completed::decode(body)?;
        let definition = match chapter {
            Chapter::FirstDrive => content.start.vehicle_item,
            Chapter::FirstRace | Chapter::SecondRace | Chapter::ReturnDrive => {
                let inventory = inventory.ok_or(replication::Error::Unsupported)?;
                let item = inventory.slots.values()[0].ok_or(replication::Error::UnknownObject)?;
                inventory
                    .items
                    .items
                    .get(&item)
                    .ok_or(replication::Error::UnknownObject)?
                    .definition
            }
        };
        let car = vehicles
            .and_then(|v| v.vehicles.definition(&definition))
            .ok_or(replication::Error::Unsupported)?;
        active.clock(seconds)?;
        Ok(Some(active.completed(
            content.mode(*chapter)?,
            players,
            call,
            crate::event_logic::car_id(&car.blueprint),
        )?))
    }
    pub(super) fn intro_car_ready(
        content: Option<&crate::prologue::Content>,
        players: &mut Players,
        participants: &mut Lifecycle,
        persona: u64,
        participant: u16,
        vehicle: u16,
    ) -> Result<Option<Ready>, replication::Error> {
        if participants.stage(participant) != Some(Stage::IntroSpawning) {
            return Ok(None);
        }
        let content = content.ok_or(replication::Error::Unsupported)?;
        if !players.owns_participant_vehicle(HOST_SELECTOR as u8, persona, participant, vehicle) {
            return Err(replication::Error::UnknownObject);
        }
        let pose = match participants
            .intro_chapter(participant)
            .ok_or(replication::Error::Shape)?
        {
            Chapter::ReturnDrive => return Err(replication::Error::Shape),
            Chapter::FirstDrive => {
                if players.temporary_vehicle(HOST_SELECTOR as u8, persona, participant)
                    != Some(vehicle)
                {
                    return Err(replication::Error::UnknownObject);
                }
                content.start.teleport
            }
            Chapter::FirstRace => {
                content
                    .first_race
                    .as_ref()
                    .ok_or(replication::Error::Unsupported)?
                    .destination
                    .teleport
            }
            Chapter::SecondRace => {
                content
                    .second_race
                    .as_ref()
                    .ok_or(replication::Error::Unsupported)?
                    .destination
                    .teleport
            }
        };
        let mut records = players.bind_player_vehicle(
            HOST_SELECTOR as u8,
            persona,
            participant,
            Some(vehicle),
        )?;
        records.extend(players.actor_vehicle_ready(HOST_SELECTOR as u8, persona, participant)?);
        let command = Command {
            endpoint: participants
                .intro_bindings()
                .ok_or(replication::Error::UnknownObject)?
                .teleport,
            participant,
            vehicle,
            position: pose.position,
            basis: pose.basis,
            speed: 0.,
        };
        command.encode()?;
        let Outcome::Advanced(calls) = participants.intro_vehicle_ready(participant) else {
            return Err(replication::Error::Shape);
        };
        Ok(Some((
            records,
            calls.into_iter().map(HostRpc::Participant).collect(),
            command,
        )))
    }
    pub(super) fn opening_signal(
        content: &crate::prologue::Content,
        opening: &mut Instances,
        players: &Players,
        persona: u64,
        message: &nfs_world::logic::Message,
        responses: &mut Vec<HostRpc>,
    ) -> Result<bool, replication::Error> {
        let nfs_world::logic::Message::Reached { target, .. } = message else {
            return Ok(false);
        };
        let Some(owner) = opening.owner(target.ghost) else {
            return Ok(false);
        };
        if owner.connection != HOST_SELECTOR as u8 || owner.persona != persona {
            return Err(replication::Error::UnknownObject);
        }
        if let Some(stop) = opening.signal(&content.opening, players, owner, message)? {
            responses.push(HostRpc::SequenceStop(stop));
        }
        Ok(true)
    }
    pub(super) fn opening_completed(
        opening: &mut Instances,
        players: &mut Players,
        participants: &mut Lifecycle,
        logic: Option<&crate::garage_logic::GarageLogic>,
        logic_ghosts: &mut BTreeMap<u16, BTreeMap<u16, u16>>,
        persona: u64,
        body: BitSpan<'_>,
    ) -> Result<Option<Option<Finished>>, replication::Error> {
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
        if !opening.has_endpoint(ghost, route.selector()) || route.method_index() != 1 {
            return Ok(None);
        }
        let call = Completed::decode(body)?;
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant: call.participant,
        };
        let Some(deleted) = opening.complete(players, owner, call)? else {
            return Ok(Some(None));
        };
        let Outcome::Advanced(calls) = participants.intro_opening_complete(owner.participant)
        else {
            return Err(replication::Error::Shape);
        };
        // The first drive's presentation logic (return request, HUD) is
        // created with the drive, as at the official garage arrival (career
        // start 914 and 918 precede the first-drive mode 920; E1183).
        let records = match logic {
            Some(logic) => {
                logic
                    .produce_present(
                        crate::garage_logic::Which::Arrived,
                        players,
                        owner.participant,
                        logic_ghosts.entry(owner.participant).or_default(),
                        crate::garage_logic::Part::Creations,
                    )?
                    .records
            }
            None => Vec::new(),
        };
        Ok(Some(Some((
            Section {
                float_bits: None,
                flag: false,
                deleted: vec![deleted],
                setup: (!records.is_empty()).then_some(Setup::RawEscape([0; 3])),
                records,
            },
            calls.into_iter().map(HostRpc::Participant).collect(),
        ))))
    }
}
