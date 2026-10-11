// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use crate::limits::{control_body_limits as body_limits, control_frame_limits as frame_limits};
use nfs_fire2::Frame;
use nfs_protocol::{gamemanager::*, users::*};
use nfs_services::world_setup as owned;

pub use nfs_services::world_setup::{Generated, SEED_BYTES};
fn frame(wire: &[u8], command: u16, category: u8) -> Result<Frame<'_>, Failure> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Failure::ProfileConfig)?
        .ok_or(Failure::ProfileConfig)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || f.fields.routing_a != 4
        || f.fields.routing_b != command
        || f.fields.category != category
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(f)
}
fn net_bytes(n: &NetworkAddress<'_>) -> Result<Vec<u8>, Failure> {
    let NetworkAddress::IpPair(p) = n else {
        return Err(Failure::ProfileConfig);
    };
    if p.unknown_field_count() != 0
        || p.machine_id.is_none()
        || p.external_address.as_ref().is_none_or(|p| {
            p.unknown_field_count() != 0
                || p.ip.is_none()
                || p.port.is_none()
                || p.machine_id.is_none()
        })
        || p.internal_address.as_ref().is_none_or(|p| {
            p.unknown_field_count() != 0
                || p.ip.is_none()
                || p.port.is_none()
                || p.machine_id.is_none()
        })
    {
        return Err(Failure::ProfileConfig);
    }
    p.encode(body_limits()).map_err(|_| Failure::ProfileConfig)
}
pub struct Profile {
    config: owned::Config,
    metrics: owned::MatchMetrics,
}
impl Profile {
    pub fn owned(config: owned::Config) -> Self {
        let metrics = owned::MatchMetrics {
            fit: config.max_fit(),
            elapsed_micros: 0,
        };
        Self { config, metrics }
    }
    pub fn setup(&self, current: &Current, generated: &Generated) -> Result<Vec<u8>, Failure> {
        self.config
            .setup(&current.owned(), &generated.allocation(), self.metrics)
            .map_err(|_| Failure::ProfileConfig)
    }
}
pub struct Session {
    publication: owned::Publication,
    current: Current,
    generated: Generated,
    observed: bool,
}
impl Session {
    pub fn new(profile: &Profile, current: Current, generated: Generated) -> Result<Self, Failure> {
        let publication = owned::Publication::new(
            &profile.config,
            &current.owned(),
            &generated.allocation(),
            profile.metrics,
        )
        .map_err(|_| Failure::ProfileConfig)?;
        Ok(Self {
            publication,
            current,
            generated,
            observed: false,
        })
    }
    pub fn emit_after_accepted_m1(&mut self) -> Result<Option<Vec<u8>>, Failure> {
        self.publication.emit().map_err(|_| Failure::Reply)
    }
    pub fn commit_after_write(&mut self) -> Result<(), Failure> {
        self.publication
            .commit_after_write()
            .map_err(|_| Failure::Reply)
    }
    pub fn abort_write(&mut self) {
        self.publication.abort_write();
    }
    pub fn observe_self_mesh(&mut self, wire: &[u8]) -> Result<(), Failure> {
        if !self.publication.published() || self.observed {
            return Err(Failure::IneligibleRequest);
        }
        validate_self_mesh(wire, &self.current, &self.generated)?;
        self.observed = true;
        Ok(())
    }
    pub(crate) fn connection_binding(&self) -> Result<crate::world_connection::Current, Failure> {
        if !self.publication.published() {
            return Err(Failure::IneligibleRequest);
        }
        Ok(crate::world_connection::Current {
            group_id: self.current.g1,
            session_id: self.current.m1,
            game_id: self.generated.game_id,
            persona_id: self.current.persona,
            user_session_id: self.current.uid,
            local_connection_group_id: self.current.connection,
            host_connection_group_id: self.generated.host_connection,
            scenario_id: self.current.scenario,
        })
    }
    pub(crate) fn transport_binding(&self) -> Result<crate::world_handshake::Binding, Failure> {
        if !self.publication.published() {
            return Err(Failure::IneligibleRequest);
        }
        let p = ReplicatedGamePlayer::decode(&self.current.player, body_limits())
            .map_err(|_| Failure::ProfileConfig)?;
        crate::world_handshake::Binding::from_current(
            &self.generated.uuid,
            p.uuid.ok_or(Failure::ProfileConfig)?,
            self.current.connection,
            self.generated.host_connection,
        )
    }
    pub(crate) fn readiness_binding(&self) -> Result<crate::world_readiness::Binding, Failure> {
        if !self.publication.published() {
            return Err(Failure::IneligibleRequest);
        }
        Ok(crate::world_readiness::Binding {
            group: self.current.g1,
            matchmaking: self.current.m1,
            game: self.generated.game_id,
            persona: self.current.persona,
            user_session: self.current.uid,
            local_connection: self.current.connection,
            host_connection: self.generated.host_connection,
            baseline_join_micros: self.generated.clock,
        })
    }
}
pub struct Current {
    g1: u64,
    m1: u64,
    persona: i64,
    uid: u64,
    connection: u64,
    scenario: u64,
    player: Vec<u8>,
    game_uuid: Vec<u8>,
}
pub(crate) struct Accepted<'a> {
    pub(crate) g1_setup: &'a [u8],
    pub(crate) request: &'a [u8],
    pub(crate) reply: &'a [u8],
    pub(crate) group_id: u64,
    pub(crate) session_id: u64,
    pub(crate) persona_id: i64,
    pub(crate) user_session_id: u64,
    pub(crate) connection_group_id: u64,
    pub(crate) scenario_id: u64,
}
impl Current {
    fn owned(&self) -> owned::Current<'_> {
        owned::Current {
            group: self.g1,
            matchmaking: self.m1,
            persona: self.persona,
            user_session: self.uid,
            connection: self.connection,
            scenario: self.scenario,
            player: &self.player,
            group_uuid: &self.game_uuid,
        }
    }
    pub(crate) fn from_accepted(a: Accepted<'_>) -> Result<Self, Failure> {
        let c = Self::source_association(a.g1_setup, a.request, a.reply)?;
        if (c.g1, c.m1, c.persona, c.uid, c.connection, c.scenario)
            != (
                a.group_id,
                a.session_id,
                a.persona_id,
                a.user_session_id,
                a.connection_group_id,
                a.scenario_id,
            )
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(c)
    }
    pub(crate) fn source_association(
        g1: &[u8],
        query: &[u8],
        reply: &[u8],
    ) -> Result<Self, Failure> {
        let gf = frame(g1, 20, 2)?;
        let qf = frame(query, 13, 0)?;
        let rf = frame(reply, 13, 1)?;
        if gf.fields.correlation != 0 || qf.fields.correlation != rf.fields.correlation {
            return Err(Failure::ProfileConfig);
        }
        let g =
            NotifyGameSetup::decode(gf.body, body_limits()).map_err(|_| Failure::ProfileConfig)?;
        let q = StartMatchmakingRequest::decode(qf.body, body_limits())
            .map_err(|_| Failure::ProfileConfig)?;
        let r = StartMatchmakingResponse::decode(rf.body, body_limits())
            .map_err(|_| Failure::ProfileConfig)?;
        if g.unknown_field_count() != 0
            || q.unknown_field_count() != 0
            || r.unknown_field_count() != 0
            || g.encode(body_limits())
                .map_err(|_| Failure::ProfileConfig)?
                != gf.body
            || q.encode(body_limits())
                .map_err(|_| Failure::ProfileConfig)?
                != qf.body
            || r.encode(body_limits())
                .map_err(|_| Failure::ProfileConfig)?
                != rf.body
        {
            return Err(Failure::ProfileConfig);
        }
        let gd = g.game_data.as_ref().ok_or(Failure::ProfileConfig)?;
        let gid = gd
            .game_id
            .filter(|x| *x > 0 && *x <= i64::MAX as u64)
            .ok_or(Failure::ProfileConfig)?;
        let join = q.player_join_data.as_ref().ok_or(Failure::ProfileConfig)?;
        let players = join
            .player_data_list
            .as_ref()
            .ok_or(Failure::ProfileConfig)?;
        if gd.game_type != Some(1)
            || players.0.len() != 1
            || join.group_id != Some(ObjectId(4, 2, gid as i64))
        {
            return Err(Failure::ProfileConfig);
        }
        let persona = players.0[0]
            .user
            .as_ref()
            .and_then(|u| u.blaze_id)
            .filter(|v| *v > 0)
            .ok_or(Failure::ProfileConfig)?;
        let roster = g.game_roster.as_ref().ok_or(Failure::ProfileConfig)?;
        let ps: Vec<_> = roster
            .0
            .iter()
            .filter(|p| p.player_id == Some(persona))
            .collect();
        if ps.len() != 1 {
            return Err(Failure::ProfileConfig);
        }
        let p = ps[0];
        let uid = p
            .player_session_id
            .filter(|x| *x > 0)
            .ok_or(Failure::ProfileConfig)?;
        let connection = p
            .connection_group_id
            .filter(|x| *x > 0 && *x <= i64::MAX as u64)
            .ok_or(Failure::ProfileConfig)?;
        let m1 = r
            .session_id
            .filter(|x| *x > 0 && *x <= i64::MAX as u64 && *x != gid)
            .ok_or(Failure::ProfileConfig)?;
        let common = q.common_game_data.as_ref().ok_or(Failure::ProfileConfig)?;
        let scenario = common
            .originating_scenario_id
            .ok_or(Failure::ProfileConfig)?;
        if p.unknown_field_count() != 0
            || gd.unknown_field_count() != 0
            || p.uuid.is_none_or(|v| v.len() != 36)
            || gd.uuid.is_none_or(|v| v.len() != 36)
            || p.game_id != Some(gid)
            || gd.game_protocol_version_string != common.game_protocol_version_string
            || net_bytes(p.network_address.as_ref().ok_or(Failure::ProfileConfig)?)?
                != net_bytes(
                    common
                        .player_network_address
                        .as_ref()
                        .ok_or(Failure::ProfileConfig)?,
                )?
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self {
            g1: gid,
            m1,
            persona,
            uid,
            connection,
            scenario,
            player: p
                .encode(body_limits())
                .map_err(|_| Failure::ProfileConfig)?,
            game_uuid: gd.uuid.ok_or(Failure::ProfileConfig)?.to_vec(),
        })
    }
}
pub fn validate_self_mesh(
    wire: &[u8],
    current: &Current,
    generated: &Generated,
) -> Result<(), Failure> {
    generated
        .validate(&current.owned())
        .map_err(|_| Failure::ProfileConfig)?;
    let f = frame(wire, 29, 0)?;
    let q = UpdateMeshConnectionRequest::decode(f.body, body_limits())
        .map_err(|_| Failure::ProfileConfig)?;
    if q.unknown_field_count() != 0
        || q.encode(body_limits())
            .map_err(|_| Failure::ProfileConfig)?
            != f.body
        || q.game_id != Some(generated.game_id)
        || q.source_group_id != Some(ObjectId(30722, 2, current.connection as i64))
        || q.target_group_id != q.source_group_id
        || q.player_net_connection_status != Some(2)
        || q.player_net_connection_flags != Some(0)
        || q.qos_info.as_ref().is_none_or(|v| {
            v.latency_ms != Some(0)
                || v.packet_loss != Some(PacketLossBits(0))
                || v.unknown_field_count() != 0
        })
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
