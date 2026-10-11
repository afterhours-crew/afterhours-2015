// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned group, matchmaking and current-world orchestration after user setup.
//! Services build typed replies from accepted identities and fresh allocation;
//! no source-frame/template branch is available in this runtime.
use crate::Failure;
use crate::limits::{lookup_body_limits as body_limits, lookup_frame_limits as frame_limits};
use nfs_protocol::users::NotifyUserAddedInitial;
pub const MAX_REQUESTS: usize = 24;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub struct SoloProfile {
    persona: i64,
    matchmaking: Option<(
        crate::matchmaking_session::Profile,
        crate::matchmaking_session::Generated,
    )>,
    group: Option<(nfs_services::group::Config, nfs_services::group::Generated)>,
}
impl SoloProfile {
    pub fn owned(persona: i64) -> Result<Self, Failure> {
        if persona <= 0 {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self {
            persona,
            matchmaking: None,
            group: None,
        })
    }
    pub fn persona_id(&self) -> i64 {
        self.persona
    }
    pub fn with_group(
        mut self,
        policy: nfs_services::group::Config,
        generated: nfs_services::group::Generated,
    ) -> Result<Self, Failure> {
        policy.validate().map_err(crate::group_failure)?;
        self.group = Some((policy, generated));
        Ok(self)
    }
    pub fn group_time(&self) -> Option<i64> {
        self.group.as_ref().map(|(_, g)| g.create_time)
    }
    pub fn with_owned_matchmaking(
        mut self,
        admission: nfs_services::matchmaking::Config,
        status: crate::matchmaking_status::Config,
        world: nfs_services::world_setup::Config,
        matchmaking: crate::matchmaking_session::Generated,
        generated: crate::world_setup::Generated,
    ) -> Result<Self, Failure> {
        if self.group.is_none() || self.matchmaking.is_some() {
            return Err(Failure::ProfileConfig);
        }
        self.matchmaking = Some((
            crate::matchmaking_session::Profile::owned(admission, status, world, generated)?,
            matchmaking,
        ));
        Ok(self)
    }
    pub fn bounded_matchmaking_enabled(&self) -> bool {
        self.matchmaking.is_some()
    }
    pub fn matchmaking_status_enabled(&self) -> bool {
        self.matchmaking
            .as_ref()
            .is_some_and(|(profile, _)| profile.status_notification_enabled())
    }
    pub fn world_setup_enabled(&self) -> bool {
        self.matchmaking
            .as_ref()
            .is_some_and(|(profile, _)| profile.world_setup_enabled())
    }
    pub fn world_connection_enabled(&self) -> bool {
        self.matchmaking
            .as_ref()
            .is_some_and(|(profile, _)| profile.world_connection_enabled())
    }
}
pub struct Session<'a> {
    profile: &'a SoloProfile,

    requests: usize,
    input_bytes: usize,
    previous: Option<(u32, Vec<u8>)>,
    stopped: bool,
    serving: bool,

    continuation: Option<crate::world_post_readiness::Session>,

    group: Option<nfs_services::group::Session>,
    matchmaking: Option<crate::matchmaking_session::Session<'a>>,
}
impl<'a> Session<'a> {
    pub fn new(initial: &[u8], latest: &[u8], profile: &'a SoloProfile) -> Result<Self, Failure> {
        let f = nfs_fire2::decode(initial, frame_limits())
            .map_err(|_| Failure::ProfileShape)?
            .ok_or(Failure::ProfileShape)?
            .frame;
        let initial = NotifyUserAddedInitial::decode(f.body, body_limits())
            .map_err(|_| Failure::ProfileShape)?;
        if initial.user_info.as_ref().and_then(|user| user.blaze_id) != Some(profile.persona) {
            return Err(Failure::ProfileConfig);
        }
        let group = match &profile.group {
            Some((policy, generated)) => {
                let data = initial
                    .extended_data
                    .as_ref()
                    .ok_or(Failure::ProfileConfig)?;
                let objects = data
                    .blaze_object_id_list
                    .as_ref()
                    .ok_or(Failure::ProfileConfig)?;
                if objects.0.len() != 1 {
                    return Err(Failure::ProfileConfig);
                }
                let current = nfs_fire2::decode(latest, frame_limits())
                    .map_err(|_| Failure::ProfileShape)?
                    .ok_or(Failure::ProfileShape)?
                    .frame;
                let metrics = nfs_protocol::users::UserSessionExtendedDataUpdate::decode(
                    current.body,
                    body_limits(),
                )
                .map_err(|_| Failure::ProfileShape)?;
                let qos = metrics
                    .extended_data
                    .as_ref()
                    .and_then(|d| d.qos_data.as_ref())
                    .ok_or(Failure::ProfileConfig)?;
                let identity = nfs_services::group::Identity::new(
                    initial.user_info.as_ref().ok_or(Failure::ProfileConfig)?,
                    objects.0[0],
                    qos,
                )
                .map_err(crate::group_failure)?;
                Some(
                    nfs_services::group::Session::new(policy.clone(), identity, generated.clone())
                        .map_err(crate::group_failure)?,
                )
            }
            None => None,
        };
        Ok(Self {
            profile,

            requests: 0,
            input_bytes: 0,
            previous: None,
            stopped: false,
            serving: false,

            continuation: None,

            group,
            matchmaking: None,
        })
    }
    pub fn requests(&self) -> usize {
        self.requests
    }
    pub fn world_scope_eligible(&self) -> Option<bool> {
        self.matchmaking
            .as_ref()
            .and_then(|m| m.world_scope_eligible())
    }
    pub fn world_self_mesh_observed(&self) -> bool {
        self.matchmaking
            .as_ref()
            .is_some_and(|m| m.world_self_mesh_observed())
    }
    pub fn take_world_transport_binding(
        &mut self,
    ) -> Result<Option<crate::world_handshake::Binding>, Failure> {
        self.matchmaking
            .as_mut()
            .map(|m| m.take_world_transport_binding())
            .transpose()
            .map(Option::flatten)
    }
    pub fn world_connection_progress(&self) -> (bool, usize) {
        self.matchmaking
            .as_ref()
            .map(|m| m.world_connection_progress())
            .unwrap_or_default()
    }
    pub fn take_world_readiness_after_write(
        &mut self,
    ) -> Result<Option<crate::world_readiness::Session>, Failure> {
        self.matchmaking
            .as_mut()
            .map(|m| m.take_world_readiness_after_write())
            .transpose()
            .map(Option::flatten)
    }

    pub fn enable_world_continuation(
        &mut self,
        permit: crate::world_readiness::ContinuationPermit,
    ) -> Result<(), Failure> {
        if self.continuation.is_some() {
            self.abort_write();
            return Err(Failure::IneligibleRequest);
        }
        let matchmaking = self
            .matchmaking
            .as_ref()
            .ok_or(Failure::IneligibleRequest)?;
        let Some(config) = matchmaking.world_continuation_config()? else {
            return Ok(());
        };
        if matchmaking.world_readiness_binding()? != permit.binding() {
            self.abort_write();
            return Err(Failure::IneligibleRequest);
        }
        let mut session = crate::world_post_readiness::Session::new(config);
        if session.enable(permit).is_err() {
            self.abort_write();
            return Err(Failure::ProfileConfig);
        }
        self.continuation = Some(session);
        Ok(())
    }
    pub fn commit_after_write(&mut self) -> Result<(), Failure> {
        if let Some(m) = self.matchmaking.as_mut() {
            m.commit_after_write()?;
        }
        if let Some(group) = self.group.as_mut() {
            group.commit_after_write();
        }
        if let Some(session) = self.continuation.as_mut() {
            session.commit_after_write().map_err(|_| Failure::Reply)?;
        }
        Ok(())
    }
    pub fn abort_write(&mut self) {
        if let Some(matchmaking) = self.matchmaking.as_mut() {
            matchmaking.abort_connection_write();
        }
        if let Some(group) = self.group.as_mut() {
            group.abort_write();
        }
        if let Some(session) = self.continuation.as_mut() {
            session.abort_write();
        }
        self.stopped = true;
    }
    pub fn serving(mut self) -> Self {
        self.serving = true;
        self
    }

    pub fn response(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        if self
            .continuation
            .as_ref()
            .is_some_and(|s| s.has_pending_write())
        {
            self.abort_write();
            return Err(Failure::Reply);
        }
        let limit = MAX_REQUESTS;
        if self.stopped || (!self.serving && self.requests == limit) {
            self.stopped = true;
            return Ok(None);
        }
        self.requests = self.requests.saturating_add(1);
        let result = self.next(wire);
        if !matches!(result, Ok(Some(_))) {
            if !self.serving {
                self.stopped = true;
            }
            self.previous = None;
        }
        result
    }
    fn next(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        let d = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Failure::IneligibleRequest)?
            .ok_or(Failure::IneligibleRequest)?;
        let f = d.frame;
        if d.consumed != wire.len()
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Failure::IneligibleRequest);
        }
        if let Some((correlation, previous)) = &self.previous
            && *correlation == f.fields.correlation
            && previous != wire
        {
            return Err(Failure::IneligibleRequest);
        }
        self.input_bytes = self
            .input_bytes
            .checked_add(wire.len())
            .ok_or(Failure::BodyLimit)?;
        if !self.serving && self.input_bytes > MAX_INPUT_BYTES {
            return Err(Failure::BodyLimit);
        }
        if (f.fields.routing_a, f.fields.routing_b) == (4, 13)
            && self.profile.bounded_matchmaking_enabled()
        {
            if self.matchmaking.is_none() {
                let context = self
                    .group
                    .as_ref()
                    .and_then(|g| g.matchmaking_context())
                    .ok_or(Failure::IneligibleRequest)?
                    .clone();
                let (policy, generated) = self
                    .profile
                    .matchmaking
                    .as_ref()
                    .ok_or(Failure::ProfileConfig)?;
                self.matchmaking = Some(crate::matchmaking_session::Session::new(
                    policy,
                    crate::matchmaking_session::Context::from_group(&context),
                    generated.clone(),
                )?);
            }
            let replies = self
                .matchmaking
                .as_mut()
                .ok_or(Failure::ProfileConfig)?
                .response(wire)?;
            if replies.is_some() {
                self.previous = Some((f.fields.correlation, wire.to_vec()));
            }
            return Ok(replies);
        }
        if (f.fields.routing_a, f.fields.routing_b) == (4, 7)
            && let Some(policy) = self.continuation.as_mut()
            && policy.is_enabled()
        {
            let replies = policy
                .response(wire)
                .map_err(|_| Failure::IneligibleRequest)?;
            if replies.is_some() {
                self.previous = Some((f.fields.correlation, wire.to_vec()));
            }
            return Ok(replies);
        }
        if f.fields.routing_a == 4 {
            if f.fields.routing_b == 29
                && self.profile.world_setup_enabled()
                && self.matchmaking.is_some()
            {
                if self.profile.world_connection_enabled() {
                    let replies = self
                        .matchmaking
                        .as_mut()
                        .ok_or(Failure::IneligibleRequest)?
                        .reply_world_self_mesh(wire)?;
                    self.previous = Some((f.fields.correlation, wire.to_vec()));
                    return Ok(Some(replies));
                }
                self.matchmaking
                    .as_mut()
                    .ok_or(Failure::IneligibleRequest)?
                    .observe_world_self_mesh(wire)?;
                return Ok(None);
            }
            let replies = match &mut self.group {
                Some(group) => group.response(wire).map_err(crate::group_failure)?,
                None => None,
            };
            if replies.is_some() {
                self.previous = Some((f.fields.correlation, wire.to_vec()));
            }
            return Ok(replies);
        }
        Ok(None)
    }
}
