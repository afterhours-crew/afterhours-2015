// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::prologue::movies::{Active, Output};

pub(in crate::net::players) type Movies = BTreeMap<u16, Active>;

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
        let Some(movie) = self.prologue.as_deref().and_then(|c| c.manu_movie.as_ref()) else {
            return Ok(());
        };
        if self.session_backlog >= MAX_PENDING
            || !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || !self.pending_rpcs.is_empty()
        {
            return Ok(());
        }
        let (Some(progression), Some(loaded)) = (&self.progression, &self.persistent) else {
            return Ok(());
        };
        let restored = progression
            .catalog
            .restore(loaded, progression.settings)
            .map_err(|_| replication::Error::Shape)?;
        if !restored
            .branch(crate::progression::Branch::Intro)
            .objectives()
            .get(&movie.data.objective)
            .is_some_and(|o| o.active && !o.completed)
        {
            return Ok(());
        }
        let participant = self.participants.intro_participants().find(|&p| {
            self.participants.intro_chapter(p) == Some(Chapter::FirstRace)
                && self.participants.stage(p) == Some(Stage::FreeRoam)
                && !self.intro_movies.contains_key(&p)
        });
        let Some(participant) = participant else {
            return Ok(());
        };
        if self.intro_movies.len() >= nfs_world::sequences::MAX_INSTANCES {
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
        let (active, output) = Active::start(movie, &mut players, owner)?;
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
    persona: u64,
    body: BitSpan<'_>,
) -> Result<Option<Output>, replication::Error> {
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
        let mut output = active.completed(players, owner, completed)?;
        if output.finished {
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
    let Some(content) = &content.manu_movie else {
        return Ok(None);
    };
    if route.method_index() != 3
        || players
            .objects()
            .scene_endpoint(content.data.state.0, content.data.state.1)
            != Some((ghost, route.selector()))
    {
        return Ok(None);
    }
    let &[_, participant] = envelope.references() else {
        return Err(replication::Error::Shape);
    };
    let active = movies
        .get_mut(&participant)
        .ok_or(replication::Error::UnknownObject)?;
    active.receive(
        content,
        players,
        Owner {
            connection: HOST_SELECTOR as u8,
            persona,
            participant,
        },
        body,
    )
}

#[cfg(test)]
mod tests;
