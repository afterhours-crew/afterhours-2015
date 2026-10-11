// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local eligible-player snapshot and explicit empty history/recommendations.
//! The caller owns directory authorization and refreshes snapshots when state
//! changes. Nonempty history/recommendations remain unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    IneligibleRequest,
    Context,
    Reply,
}
use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::*;
use std::collections::BTreeSet;

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 4096,
        max_depth: 2,
        max_values: 256,
        max_collection: 64,
        max_byte_string: 128,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(4112, 0, 4096).expect("constant limits")
}
fn frame(wire: &[u8], category: u8) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::IneligibleRequest)?
        .ok_or(Error::IneligibleRequest)?;
    if d.consumed != wire.len()
        || d.frame.fields.routing_a != COMPONENT
        || d.frame.fields.category != category
        || d.frame.fields.slot != 0
        || d.frame.fields.reserved != [0, 0]
        || !d.frame.metadata.is_empty()
    {
        return Err(Error::IneligibleRequest);
    }
    Ok(d.frame)
}
/// One connection's explicitly selected eligible population. Caller supplies
/// authorization and sampling from its local directory; this layer only encodes.
/// Rebuild after population/history changes. No Debug exposes identities.
pub struct Current {
    persona: i64,
    recommendations_empty: bool,
    players: Vec<i64>,
    history_empty: bool,
}
impl Current {
    pub fn new(self_id: i64, eligible_players: &[i64], history_empty: bool) -> Result<Self, Error> {
        if self_id <= 0
            || eligible_players.len() > 50
            || eligible_players.iter().any(|x| *x <= 0 || *x == self_id)
            || eligible_players.iter().collect::<BTreeSet<_>>().len() != eligible_players.len()
        {
            return Err(Error::Context);
        }
        Ok(Self {
            persona: self_id,
            recommendations_empty: false,
            players: eligible_players.to_vec(),
            history_empty,
        })
    }
    pub fn reply(&self, wire: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let f = frame(wire, 0)?;
        let body = match f.fields.routing_b {
            GET_FRIENDS_RECOMMENDATIONS => {
                let m = FriendsRecommendationsRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::IneligibleRequest)?;
                if !self.recommendations_empty
                    || m.blaze_id != Some(self.persona)
                    || m.unknown_field_count() != 0
                    || m.encode(body_limits())
                        .map_err(|_| Error::IneligibleRequest)?
                        != f.body
                {
                    return Ok(None);
                }
                // The supported empty response omits BLIS. Only an explicit
                // empty recommendation state permits this response.
                Vec::new()
            }
            GET_RANDOM_PLAYERS => {
                let m = RandomPlayersRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::IneligibleRequest)?;
                if m.max_players != Some(50)
                    || m.unknown_field_count() != 0
                    || m.encode(body_limits())
                        .map_err(|_| Error::IneligibleRequest)?
                        != f.body
                {
                    return Ok(None);
                }
                RandomPlayersResponse {
                    blaze_ids: Some(PlayerIds(self.players.clone())),
                    ..Default::default()
                }
                .encode(body_limits())
                .map_err(|_| Error::Reply)?
            }
            GET_RECENT_PLAYERS => {
                let m = GetRecentPlayersRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::IneligibleRequest)?;
                if !self.history_empty
                    || m.include_first_party_friends != Some(false)
                    || m.max_players_to_return != Some(0)
                    || m.sort_order != Some(0)
                    || m.players.is_some()
                    || m.unknown_field_count() != 0
                    || m.encode(body_limits())
                        .map_err(|_| Error::IneligibleRequest)?
                        != f.body
                {
                    return Ok(None);
                }
                Vec::new()
            }
            _ => return Ok(None),
        };
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map(Some)
        .map_err(|_| Error::Reply)
    }

    /// Explicit account state, independent of the random-player population.
    pub fn with_empty_friend_recommendations(mut self) -> Self {
        self.recommendations_empty = true;
        self
    }
}

/// Routes handled by this service. Caller must not fall back to a template when
/// a selected local policy rejects an unsupported request.
pub fn owns(component: u16, command: u16) -> bool {
    component == COMPONENT
        && matches!(
            command,
            GET_FRIENDS_RECOMMENDATIONS | GET_RECENT_PLAYERS | GET_RANDOM_PLAYERS
        )
}
