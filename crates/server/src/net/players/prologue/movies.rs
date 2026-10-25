// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::prologue::movies::{Active, Output};

pub(in crate::net::players) type Movies = BTreeMap<u16, Active>;

fn content_for(
    content: &crate::prologue::Content,
    objective: u64,
) -> Result<&crate::prologue::movies::Content, replication::Error> {
    let mut matches = [&content.manu_movie, &content.diner_movie]
        .into_iter()
        .flatten()
        .filter(|c| c.data.objective == objective);
    let found = matches.next().ok_or(replication::Error::Shape)?;
    if matches.next().is_some() {
        return Err(replication::Error::Shape);
    }
    Ok(found)
}

pub(in crate::net::players) fn sections(
    records: Vec<replication::Record>,
    deleted: Vec<u16>,
) -> Result<Vec<Section>, replication::Error> {
    let mut out = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS - 96)?;
    for section in &mut out {
        section.setup = Some(Setup::RawEscape([0; 3]));
    }
    if !deleted.is_empty() {
        out.push(Section {
            float_bits: None,
            flag: false,
            setup: None,
            records: vec![],
            deleted,
        });
    }
    Ok(out)
}

impl PlayerListener {
    /// Only committed, restored story state can start the next movie. Keep the
    /// finished owner until its chapter retires so repeated ticks cannot replay it.
    pub(in crate::net::players) fn advance_story_movies(
        &mut self,
    ) -> Result<(), replication::Error> {
        let Some(content) = self.prologue.as_deref() else {
            return Ok(());
        };
        if self.session_backlog >= MAX_PENDING
            || !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || !self.pending_rpcs.is_empty()
        {
            return Ok(());
        }
        // Timer changes share the same backpressure/commit boundary as input.
        // Advance existing owners even when their durable objective has changed.
        let seconds = self.world_ms as f64 / 1000.;
        let mut movies = self.intro_movies.clone();
        let mut calls = Vec::new();
        for active in movies.values_mut() {
            calls.extend(
                active
                    .advance(
                        content_for(content, active.objective())?,
                        &self.players,
                        seconds,
                    )?
                    .calls,
            );
        }
        let calls = super::presentation::present(
            &super::presentation::Context {
                logic: self.logic.as_deref(),
                ghosts: &self.logic_ghosts,
                players: &self.players,
                participants: &self.participants,
                content: Some(content),
            },
            calls,
        )?;
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len()) {
            return Ok(());
        }
        self.intro_movies = movies;
        if !calls.is_empty() {
            self.pending_rpcs.extend(calls);
            return Ok(());
        }
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        let restored = progression
            .catalog
            .restore(loaded, progression.settings)
            .map_err(|_| replication::Error::Shape)?;
        let selected = self.participants.intro_participants().find_map(|p| {
            let movie = match self.participants.intro_chapter(p)? {
                Chapter::FirstRace => content.manu_movie.as_ref()?,
                Chapter::SecondRace => content.diner_movie.as_ref()?,
                Chapter::FirstDrive | Chapter::ReturnDrive => return None,
            };
            let available = self.intro_movies.get(&p).is_none_or(|old| {
                old.phase() == nfs_world::fmv::Phase::Finished
                    && old.objective() != movie.data.objective
            });
            (self.participants.stage(p) == Some(Stage::FreeRoam)
                && available
                && restored
                    .branch(crate::progression::Branch::Intro)
                    .objectives()
                    .get(&movie.data.objective)
                    .is_some_and(|o| o.active && !o.completed))
            .then_some((p, movie))
        });
        let Some((participant, movie)) = selected else {
            return Ok(());
        };
        if !self.intro_movies.contains_key(&participant)
            && self.intro_movies.len() >= nfs_world::sequences::MAX_INSTANCES
        {
            return Err(replication::Error::Bound);
        }
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona: self.persona,
            participant,
        };
        let mut players = self.players.clone();
        let mut participants = self.participants.clone();
        let Outcome::Advanced(chain) = participants.intro_movie_begin(participant) else {
            return Err(replication::Error::Shape);
        };
        let (active, output) = Active::start_at(movie, &mut players, owner, seconds)?;
        let mut calls = chain
            .into_iter()
            .map(HostRpc::Participant)
            .collect::<Vec<_>>();
        calls.extend(output.calls);
        let sections = sections(output.records, output.deleted)?;
        if sections.len() > MAX_PENDING.saturating_sub(self.pending.len())
            || calls.len()
                > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Ok(());
        }
        self.players = players;
        self.participants = participants;
        self.intro_movies.insert(participant, active);
        self.pending.extend(sections);
        self.pending_rpcs.extend(calls);
        Ok(())
    }
}

/// Called on the frame's staged owners. Allocation, sequence deletion, lifecycle
/// changes and responses commit only after the complete frame fits its queues.
pub(in crate::net::players) fn input(
    content: &crate::prologue::Content,
    movies: &mut Movies,
    players: &mut Players,
    participants: &mut Lifecycle,
    context: Input,
    body: BitSpan<'_>,
) -> Result<Option<Output>, replication::Error> {
    let Input {
        persona,
        gameplay,
        seconds,
    } = context;
    if movies.is_empty() {
        return Ok(None);
    }
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
        && route.method_index() == 1
        && let Some(active) = movies.values_mut().find(|a| a.sequence() == Some(ghost))
    {
        let completed = Completed::decode(body)?;
        let owner = Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant: completed.participant,
        };
        let before = active.phase();
        let mut output = active.completed(players, owner, completed)?;
        if before == nfs_world::fmv::Phase::Before
            && active.phase() == nfs_world::fmv::Phase::Playing
        {
            output.deleted.push(
                players
                    .retire_actor(
                        gameplay.ok_or(replication::Error::Shape)?,
                        owner.connection,
                        owner.persona,
                        owner.participant,
                    )?
                    .ok_or(replication::Error::Shape)?,
            );
        }
        if output.finished {
            if let Some(binding) = players.bind_actor(
                gameplay.ok_or(replication::Error::Shape)?,
                owner.connection,
                owner.persona,
                owner.participant,
            )? {
                output.calls.insert(0, HostRpc::Actor(binding));
            }
            let Outcome::Advanced(chain) = participants.intro_movie_complete(owner.participant)
            else {
                return Err(replication::Error::Shape);
            };
            output
                .calls
                .extend(chain.into_iter().map(HostRpc::Participant));
        }
        return Ok(Some(output));
    }
    if route.method_index() != 3 {
        return Ok(None);
    }
    let known_endpoint = [&content.manu_movie, &content.diner_movie]
        .into_iter()
        .flatten()
        .any(|c| {
            players
                .objects()
                .scene_endpoint(c.data.state.0, c.data.state.1)
                == Some((ghost, route.selector()))
        });
    if !known_endpoint {
        return Ok(None);
    }
    let &[_, participant] = envelope.references() else {
        return Err(replication::Error::Shape);
    };
    let active = movies
        .get_mut(&participant)
        .ok_or(replication::Error::UnknownObject)?;
    let content = content_for(content, active.objective())?;
    if players
        .objects()
        .scene_endpoint(content.data.state.0, content.data.state.1)
        != Some((ghost, route.selector()))
    {
        return Err(replication::Error::Shape);
    }
    let owner = Owner {
        connection: HOST_SELECTOR as u8,
        persona,
        participant,
    };
    active.clock(seconds)?;
    let mut output = active.receive_with(content, players, owner, body, |staged| {
        Ok(vec![
            staged
                .create_actor(owner.connection, owner.persona, owner.participant)?
                .ok_or(replication::Error::Shape)?,
        ])
    })?;
    // Timed narrative outros need their replacement actor while the sequence
    // is running. The immediate outro binds at its completion boundary.
    if let Some(out) = output.as_mut()
        && content.timed_outro()
        && !out.records.is_empty()
    {
        let binding = players
            .bind_actor(
                gameplay.ok_or(replication::Error::Shape)?,
                owner.connection,
                owner.persona,
                owner.participant,
            )?
            .ok_or(replication::Error::Shape)?;
        out.calls.push(HostRpc::Actor(binding));
    }
    Ok(output)
}

pub(in crate::net::players) struct Input {
    pub persona: u64,
    pub gameplay: Option<u32>,
    pub seconds: f64,
}

#[cfg(test)]
mod tests;
