// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
pub(in crate::net::players) type Composites = BTreeMap<u16, crate::prologue::composite::Active>;
type Output = (Vec<u16>, Vec<HostRpc>);

pub(in crate::net::players) fn signal(
    content: &crate::prologue::Content,
    composites: &mut Composites,
    players: &Players,
    persona: u64,
    message: &nfs_world::logic::Message,
    calls: &mut Vec<HostRpc>,
) -> Result<bool, replication::Error> {
    let nfs_world::logic::Message::Reached { target, .. } = message else {
        return Ok(false);
    };
    for active in composites.values_mut() {
        if active.owner().persona != persona || active.owner().connection != HOST_SELECTOR as u8 {
            return Err(replication::Error::UnknownObject);
        }
        if target.ghost == active.ghost() {
            if let Some(stop) = active.signal(
                content
                    .final_composite
                    .as_ref()
                    .ok_or(replication::Error::Unsupported)?,
                players,
                message,
            )? {
                calls.push(HostRpc::SequenceStop(stop));
            }
            return Ok(true);
        }
        if active.observe_completion(players, message)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(in crate::net::players) fn input(
    composites: &mut Composites,
    players: &mut Players,
    participants: &mut Lifecycle,
    persona: u64,
    body: BitSpan<'_>,
) -> Result<Option<Output>, replication::Error> {
    if composites.is_empty() {
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
    let Some(active) = composites.values_mut().find(|a| a.ghost() == ghost) else {
        return Ok(None);
    };
    if route.selector() != 0 || route.method_index() != 1 {
        return Ok(None);
    }
    if active.owner().persona != persona || active.owner().connection != HOST_SELECTOR as u8 {
        return Err(replication::Error::UnknownObject);
    }
    let call = Completed::decode(body)?;
    let Some(deleted) = active.completed(players, call)? else {
        return Ok(Some((vec![], vec![])));
    };
    let Outcome::Advanced(chain) =
        participants.intro_final_composite_complete(active.owner().participant)
    else {
        return Err(replication::Error::Shape);
    };
    Ok(Some((
        vec![deleted],
        chain.into_iter().map(HostRpc::Participant).collect(),
    )))
}

impl PlayerListener {
    pub(in crate::net::players) fn stage_composite_results(
        &self,
        players: &Players,
        composites: &mut Composites,
        pending: &mut VecDeque<crate::net::items::story::Request>,
    ) -> Result<(), replication::Error> {
        for active in composites.values_mut() {
            let Some(objective) = active.pending_completion() else {
                continue;
            };
            let progression = self
                .progression
                .as_ref()
                .ok_or(replication::Error::Unsupported)?;
            let restored = progression
                .catalog
                .restore(
                    self.persistent
                        .as_deref()
                        .ok_or(replication::Error::Unsupported)?,
                    progression.settings,
                )
                .map_err(|_| replication::Error::Shape)?;
            if !restored
                .branch(crate::progression::Branch::Intro)
                .objectives()
                .get(&objective)
                .is_some_and(|s| s.active || s.completed)
            {
                return Err(replication::Error::Shape);
            }
            let owner = active.owner();
            if pending.len() >= MAX_PENDING {
                return Err(replication::Error::Bound);
            }
            let player_index = players
                .runtime_index(owner.connection, owner.persona, owner.participant)
                .ok_or(replication::Error::UnknownObject)?;
            pending.push_back(crate::net::items::story::Request {
                participant: owner.participant,
                player_index: player_index.into(),
                progression: progression.clone(),
                action: crate::net::items::story::Action::Complete { objective },
            });
        }
        Ok(())
    }
}
