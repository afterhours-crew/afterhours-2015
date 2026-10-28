// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Licensed intro messages (`crate::messages::Licensed`). The client
//! reports a licence on its intro logic in every session (EventSync,
//! official E1026 34.664 s, E912 30.619 s on a finished profile). With the
//! objective the sender waits for completed, the message joins its manager's
//! table and its commit announces it (E1026 1247.925 s underground s107 m2,
//! `preorderclaimparts`); later sessions restore it (E912 32.336 s s107 m0).
//! A client without the licence never reports, so it never gets one.
use super::*;
use crate::{messages::Licensed, progression::Branch};
use nfs_world::replication::entity;

/// The intro logic's EventSync reports name entity 1 and the local player.
const REPORT_ENTITY: u32 = 1;

impl PlayerListener {
    /// A client licence report: remembered for the session, and the message
    /// is queued when its objective is already complete. `Ok(false)` when
    /// the message is no licence report; a report that does not name an
    /// owned intro logic is logged and dropped.
    pub(super) fn licence_report(
        &self,
        reported: &mut BTreeSet<u16>,
        players: &Players,
        story_pending: &mut VecDeque<crate::net::items::story::Request>,
        message: &nfs_world::logic::Message,
    ) -> Result<bool, replication::Error> {
        let nfs_world::logic::Message::Reached {
            event,
            target,
            player,
        } = message
        else {
            return Ok(false);
        };
        let Some(progression) = self.progression.clone() else {
            return Ok(false);
        };
        let Some(licensed) = progression
            .messages
            .as_ref()
            .and_then(|m| m.licensed(*event))
        else {
            return Ok(false);
        };
        let blueprint = progression
            .construction
            .blueprint(Branch::Intro)
            .ok_or(replication::Error::Unsupported)?;
        let participant = match players.objects().get(target.ghost).map(|o| &o.initial) {
            Some(replication::Initial::Entity { prefix, fields })
                if prefix.asset == blueprint.asset() =>
            {
                match fields.first() {
                    Some(entity::Initial::Root { reference, .. }) => Some(*reference),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(participant) = participant.filter(|&p| {
            target.entity == REPORT_ENTITY
                && *player == 0
                && players.owns_participant(HOST_SELECTOR as u8, self.persona, p)
        }) else {
            tracing::warn!(
                ghost = target.ghost,
                entity = target.entity,
                event,
                player,
                "licence report does not name an owned intro logic; dropped"
            );
            return Ok(true);
        };
        if !reported.insert(participant) {
            return Ok(true);
        }
        tracing::info!(participant, event, "client reported a licence");
        let loaded = self
            .persistent
            .as_deref()
            .ok_or(replication::Error::Unsupported)?;
        if Self::objective_completed(&progression, loaded, licensed.objective)? {
            let job =
                Self::licensed_job(players, self.persona, participant, &progression, licensed)?;
            if story_pending.len() >= MAX_PENDING {
                return Err(replication::Error::Bound);
            }
            story_pending.push_back(job);
        }
        Ok(true)
    }

    /// The licensed messages a committed completion releases for a
    /// participant whose client reported the licence.
    pub(super) fn licensed_after_completion(
        &self,
        request: &crate::net::items::story::Request,
        loaded: &crate::persistent::Loaded,
    ) -> Result<Vec<crate::net::items::story::Request>, replication::Error> {
        let crate::net::items::story::Action::Complete { objective } = request.action else {
            return Ok(vec![]);
        };
        if !self.licence_reported.contains(&request.participant) {
            return Ok(vec![]);
        }
        let Some(messages) = &request.progression.messages else {
            return Ok(vec![]);
        };
        let mut out = vec![];
        for licensed in messages
            .licensed
            .iter()
            .filter(|l| l.objective == objective)
        {
            if !Self::objective_completed(&request.progression, loaded, objective)? {
                return Err(replication::Error::Shape);
            }
            out.push(Self::licensed_job(
                &self.players,
                self.persona,
                request.participant,
                &request.progression,
                *licensed,
            )?);
        }
        Ok(out)
    }

    fn objective_completed(
        progression: &crate::progression::Content,
        loaded: &crate::persistent::Loaded,
        objective: u64,
    ) -> Result<bool, replication::Error> {
        let restored = progression
            .catalog
            .restore(loaded, progression.settings)
            .map_err(|_| replication::Error::Shape)?;
        Ok(restored
            .branch(Branch::Intro)
            .objectives()
            .get(&objective)
            .is_some_and(|s| s.completed))
    }

    fn licensed_job(
        players: &Players,
        persona: u64,
        participant: u16,
        progression: &std::sync::Arc<crate::progression::Content>,
        licensed: Licensed,
    ) -> Result<crate::net::items::story::Request, replication::Error> {
        let player_index = players
            .runtime_index(HOST_SELECTOR as u8, persona, participant)
            .ok_or(replication::Error::UnknownObject)?;
        Ok(crate::net::items::story::Request {
            participant,
            player_index: u32::from(player_index),
            progression: progression.clone(),
            action: crate::net::items::story::Action::Message {
                hash: licensed.hash,
                manager: licensed.manager,
            },
        })
    }
}
