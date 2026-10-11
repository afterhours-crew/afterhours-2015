// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;

use nfs_fire2::Frame;

use crate::limits::{lookup_body_limits as body_limits, lookup_frame_limits as frame_limits};
use nfs_protocol::gamemanager::*;

pub use nfs_services::matchmaking::{Generated, SEED_BYTES};
pub const MAX_REQUESTS: usize = 8;
pub const MAX_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_IMPORTED_SCOPES: usize = 2;
#[derive(Clone)]
pub struct Context {
    owned: Option<nfs_services::group::Context>,
    group: u64,
    player_session_id: u64,
    persona: i64,

    connection: u64,
    group_setup: Vec<u8>,
}
impl Context {
    pub(crate) fn from_group(g: &nfs_services::group::Context) -> Self {
        Self {
            owned: Some(g.clone()),
            group: g.group(),
            player_session_id: g.player_session(),
            persona: g.persona(),

            connection: g.connection(),
            group_setup: g.setup().to_vec(),
        }
    }
}

pub struct Profile {
    admission: Option<nfs_services::matchmaking::Config>,

    scopes: Vec<Vec<u8>>,
    status: Option<crate::matchmaking_status::Profile>,
    worlds: Vec<WorldProfile>,
}
struct WorldProfile {
    scope: Vec<u8>,
    setup: crate::world_setup::Profile,
    generated: crate::world_setup::Generated,

    connection: Option<crate::world_connection::Config>,
    readiness: Option<crate::world_readiness::Config>,
    continuation: Option<crate::world_post_readiness::Config>,
}
fn frame(wire: &[u8], category: u8) -> Result<Frame<'_>, Failure> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Failure::IneligibleRequest)?
        .ok_or(Failure::IneligibleRequest)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || f.fields.routing_a != 4
        || f.fields.routing_b != START_MATCHMAKING
        || f.fields.category != category
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Failure::IneligibleRequest);
    }
    Ok(f)
}

impl Profile {
    pub(crate) fn owned(
        admission: nfs_services::matchmaking::Config,
        status: crate::matchmaking_status::Config,
        world: nfs_services::world_setup::Config,
        generated: crate::world_setup::Generated,
    ) -> Result<Self, Failure> {
        admission.validate().map_err(|_| Failure::ProfileConfig)?;
        let mut key = b"local-world-allocation-policy-v1".to_vec();
        key.extend_from_slice(&generated.uuid);
        let policy = openssl::sha::sha256(&key);
        Ok(Self {
            admission: Some(admission),

            scopes: Vec::new(),
            status: Some(crate::matchmaking_status::Profile::owned(status)),
            worlds: vec![WorldProfile {
                scope: Vec::new(),
                setup: crate::world_setup::Profile::owned(world),
                generated,

                connection: Some(crate::world_connection::Config::owned()),
                readiness: Some(
                    crate::world_readiness::Config::owned(policy)
                        .map_err(|_| Failure::ProfileConfig)?,
                ),
                continuation: Some(
                    crate::world_post_readiness::Config::owned(policy)
                        .map_err(|_| Failure::ProfileConfig)?,
                ),
            }],
        })
    }

    pub fn scope_count(&self) -> usize {
        self.admission
            .as_ref()
            .map_or(self.scopes.len(), |p| p.starting_decay_ages.len())
    }

    pub fn status_notification_enabled(&self) -> bool {
        self.status.is_some()
    }

    pub fn world_setup_enabled(&self) -> bool {
        !self.worlds.is_empty()
    }

    pub fn world_connection_enabled(&self) -> bool {
        self.worlds.first().is_some_and(|w| w.connection.is_some())
    }
}

pub struct Session<'a> {
    profile: &'a Profile,
    context: Context,
    generated: Generated,
    accepted: Option<Vec<u8>>,
    acknowledgement: Option<nfs_services::matchmaking::Session>,
    requests: usize,
    bytes: usize,
    stopped: bool,
    status: Option<crate::matchmaking_status::Session>,
    world: Option<crate::world_setup::Session>,
    selected_world: Option<&'a WorldProfile>,
    connection: Option<crate::world_connection::Session>,
    world_scope_eligible: Option<bool>,
    world_self_mesh_observed: bool,
    world_transport_taken: bool,
    world_readiness_taken: bool,
}
impl<'a> Session<'a> {
    pub(crate) fn commit_after_write(&mut self) -> Result<(), Failure> {
        if self.stopped {
            self.abort_connection_write();
            return Err(Failure::Reply);
        }
        if let Some(s) = self.acknowledgement.as_mut() {
            s.commit_after_write().map_err(|_| Failure::Reply)?;
        }
        if let Some(s) = self.status.as_mut() {
            s.commit_after_write().map_err(|_| Failure::Reply)?;
        }
        if let Some(world) = self.world.as_mut() {
            world.commit_after_write()?;
        }
        Ok(())
    }

    pub fn new(
        profile: &'a Profile,
        context: Context,
        generated: Generated,
    ) -> Result<Self, Failure> {
        generated
            .validate(context.group)
            .map_err(|_| Failure::ProfileConfig)?;
        Ok(Self {
            profile,
            context,
            generated,
            accepted: None,
            acknowledgement: None,
            requests: 0,
            bytes: 0,
            stopped: false,
            status: None,
            world: None,
            selected_world: None,
            connection: None,
            world_scope_eligible: None,
            world_self_mesh_observed: false,
            world_transport_taken: false,
            world_readiness_taken: false,
        })
    }
    pub fn response(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        if self.stopped
            || self.requests == MAX_REQUESTS
            || wire.len() > MAX_INPUT_BYTES.saturating_sub(self.bytes)
        {
            self.abort_connection_write();
            return Ok(None);
        }
        self.requests += 1;
        self.bytes += wire.len();
        let result = self.next(wire);
        if !matches!(result, Ok(Some(_))) {
            self.abort_connection_write();
        }
        result
    }
    fn next(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        let f = frame(wire, 0)?;
        if self.accepted.as_ref().is_some_and(|body| body != f.body) {
            return Err(Failure::IneligibleRequest);
        }
        let q = StartMatchmakingRequest::decode(f.body, body_limits())
            .map_err(|_| Failure::IneligibleRequest)?;
        if q.unknown_field_count() != 0
            || q.encode(body_limits())
                .map_err(|_| Failure::IneligibleRequest)?
                != f.body
        {
            return Err(Failure::IneligibleRequest);
        }
        self.profile
            .admission
            .as_ref()
            .ok_or(Failure::ProfileConfig)?
            .admit(
                self.context
                    .owned
                    .as_ref()
                    .ok_or(Failure::IneligibleRequest)?,
                f.body,
            )
            .map_err(|_| Failure::IneligibleRequest)?;
        let current_scope: Vec<u8> = Vec::new();
        let accepted = nfs_services::matchmaking::Accepted {
            group: self.context.group,
            persona: self.context.persona,
            user_session: self.context.player_session_id,
            connection: self.context.connection,
            scenario: q
                .common_game_data
                .as_ref()
                .and_then(|c| c.originating_scenario_id)
                .ok_or(Failure::IneligibleRequest)?,
        };
        if self.acknowledgement.is_none() {
            self.acknowledgement = Some(
                nfs_services::matchmaking::Session::new(accepted, self.generated.clone())
                    .map_err(|_| Failure::ProfileConfig)?,
            );
        }
        let output = self
            .acknowledgement
            .as_mut()
            .ok_or(Failure::ProfileConfig)?
            .reply(accepted, f.fields.correlation)
            .map_err(|_| Failure::Reply)?;
        let mut replies = vec![output];
        if let Some(profile) = &self.profile.status {
            let accepted = crate::matchmaking_status::Accepted {
                group_id: self.context.group,
                session_id: self.generated.session_id,
                user_session_id: self.context.player_session_id,
                scenario_id: q
                    .common_game_data
                    .as_ref()
                    .and_then(|c| c.originating_scenario_id)
                    .ok_or(Failure::IneligibleRequest)?,
            };
            if self.status.is_none() {
                self.status = Some(
                    crate::matchmaking_status::Session::new(profile, accepted)
                        .map_err(|_| Failure::IneligibleRequest)?,
                );
            }
            if let Some(notification) = self
                .status
                .as_mut()
                .ok_or(Failure::ProfileConfig)?
                .emit_after_m1(accepted)
                .map_err(|_| Failure::IneligibleRequest)?
            {
                replies.push(notification);
            }
        }
        if !self.profile.worlds.is_empty() {
            let selected = self
                .profile
                .worlds
                .iter()
                .find(|w| w.scope == current_scope);
            self.world_scope_eligible = Some(selected.is_some());
            let Some(selected) = selected else {
                self.accepted = Some(f.body.to_vec());
                return Ok(Some(replies));
            };
            if self.world.is_none() {
                let current =
                    crate::world_setup::Current::from_accepted(crate::world_setup::Accepted {
                        g1_setup: &self.context.group_setup,
                        request: wire,
                        reply: &replies[0],
                        group_id: self.context.group,
                        session_id: self.generated.session_id,
                        persona_id: self.context.persona,
                        user_session_id: self.context.player_session_id,
                        connection_group_id: self.context.connection,
                        scenario_id: q
                            .common_game_data
                            .as_ref()
                            .and_then(|c| c.originating_scenario_id)
                            .ok_or(Failure::IneligibleRequest)?,
                    })?;
                self.world = Some(crate::world_setup::Session::new(
                    &selected.setup,
                    current,
                    selected.generated.clone(),
                )?);
                self.selected_world = Some(selected);
            }
            if let Some(setup) = self
                .world
                .as_mut()
                .ok_or(Failure::ProfileConfig)?
                .emit_after_accepted_m1()?
            {
                replies.push(setup);
            }
        }
        self.accepted = Some(f.body.to_vec());
        Ok(Some(replies))
    }
    pub(crate) fn observe_world_self_mesh(&mut self, wire: &[u8]) -> Result<(), Failure> {
        self.world
            .as_mut()
            .ok_or(Failure::IneligibleRequest)?
            .observe_self_mesh(wire)
            .map_err(|_| Failure::IneligibleRequest)?;
        self.world_self_mesh_observed = true;
        Ok(())
    }
    pub(crate) fn reply_world_self_mesh(&mut self, wire: &[u8]) -> Result<Vec<Vec<u8>>, Failure> {
        if self.stopped || self.world_scope_eligible != Some(true) {
            return Err(Failure::IneligibleRequest);
        }
        if self.connection.is_none() {
            let config = self
                .selected_world
                .ok_or(Failure::ProfileConfig)?
                .connection
                .as_ref()
                .ok_or(Failure::ProfileConfig)?;
            let binding = self
                .world
                .as_ref()
                .ok_or(Failure::IneligibleRequest)?
                .connection_binding()?;
            self.connection = Some(
                crate::world_connection::Session::new(config, binding)
                    .map_err(crate::world_connection::failure)?,
            );
        }
        let result = self
            .connection
            .as_mut()
            .ok_or(Failure::ProfileConfig)?
            .reply(wire)
            .map_err(crate::world_connection::failure);
        if result.is_ok() {
            self.world_self_mesh_observed = true;
        } else {
            self.stopped = true;
        }
        result
    }
    pub fn world_scope_eligible(&self) -> Option<bool> {
        self.world_scope_eligible
    }
    pub(crate) fn take_world_transport_binding(
        &mut self,
    ) -> Result<Option<crate::world_handshake::Binding>, Failure> {
        if self.stopped
            || self.world_transport_taken
            || !self.profile.world_connection_enabled()
            || self.world_scope_eligible != Some(true)
        {
            return Ok(None);
        }
        let world = self.world.as_ref().ok_or(Failure::ProfileConfig)?;
        let mut binding = world.transport_binding()?;
        if self.selected_world.is_some_and(|w| w.readiness.is_some()) {
            binding = binding.with_readiness(world.readiness_binding()?)?;
        }
        self.world_transport_taken = true;
        Ok(Some(binding))
    }
    pub(crate) fn world_connection_progress(&self) -> (bool, usize) {
        self.connection
            .as_ref()
            .map(|c| (c.validation_emitted(), c.self_request_count()))
            .unwrap_or_default()
    }
    pub(crate) fn take_world_readiness_after_write(
        &mut self,
    ) -> Result<Option<crate::world_readiness::Session>, Failure> {
        if let Some(connection) = self.connection.as_mut() {
            connection
                .commit_after_write()
                .map_err(crate::world_connection::failure)?;
        }
        if self.stopped
            || self.world_readiness_taken
            || self.world_scope_eligible != Some(true)
            || !self
                .connection
                .as_ref()
                .is_some_and(|c| c.validation_emitted())
        {
            return Ok(None);
        }
        let Some(config) = self.selected_world.and_then(|w| w.readiness.as_ref()) else {
            return Ok(None);
        };
        let binding = self
            .world
            .as_ref()
            .ok_or(Failure::ProfileConfig)?
            .readiness_binding()?;
        let session = crate::world_readiness::Session::new(config, binding)
            .map_err(|_| Failure::ProfileConfig)?;
        self.world_readiness_taken = true;
        Ok(Some(session))
    }
    pub(crate) fn abort_connection_write(&mut self) {
        if let Some(s) = self.acknowledgement.as_mut() {
            s.abort_write();
        }
        if let Some(world) = self.world.as_mut() {
            world.abort_write();
        }
        if let Some(s) = self.status.as_mut() {
            s.abort_write();
        }
        if let Some(connection) = self.connection.as_mut() {
            connection.abort_write();
        }
        self.stopped = true;
    }
    pub fn world_self_mesh_observed(&self) -> bool {
        self.world_self_mesh_observed
    }
    pub(crate) fn world_continuation_config(
        &self,
    ) -> Result<Option<&'a crate::world_post_readiness::Config>, Failure> {
        if self.stopped || !self.world_readiness_taken || self.world_scope_eligible != Some(true) {
            return Err(Failure::IneligibleRequest);
        }
        Ok(self.selected_world.and_then(|w| w.continuation.as_ref()))
    }
    pub(crate) fn world_readiness_binding(
        &self,
    ) -> Result<crate::world_readiness::Binding, Failure> {
        if self.stopped || !self.world_readiness_taken || self.world_scope_eligible != Some(true) {
            return Err(Failure::IneligibleRequest);
        }
        self.world
            .as_ref()
            .ok_or(Failure::ProfileConfig)?
            .readiness_binding()
    }
}
