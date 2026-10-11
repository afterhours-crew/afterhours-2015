// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use crate::{
    identity::Identities,
    menu_news,
    recommendations::{self, Recommendations},
    shared_wraps,
    telemetry::{self, Telemetry},
    user_lookup, user_settings,
};
use crate::{
    startup_branch, startup_profile as startup_followup, world_handshake,
    world_readiness::{self, Decision, InitialReliableSyncPrerequisite},
};
use nfs_services::user_session::users_metrics;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestRoute {
    pub component: u16,
    pub command: u16,
    pub category: u8,
    pub correlation: u32,
}

impl RequestRoute {
    pub fn of(wire: &[u8]) -> Option<Self> {
        let decoded = nfs_fire2::decode(wire, crate::frame_limits()).ok()??;
        if decoded.consumed != wire.len() {
            return None;
        }
        let f = decoded.frame.fields;
        Some(Self {
            component: f.routing_a,
            command: f.routing_b,
            category: f.category,
            correlation: f.correlation,
        })
    }
}
#[derive(Debug, Eq, PartialEq)]
pub enum Answer {
    Reply(Vec<Vec<u8>>),
    AwaitWorld,
    Unsupported,
}

pub struct Startup<'p> {
    profile: &'p startup_followup::Profile,
    users: Option<users_metrics::Followup>,
    initial: Vec<u8>,
    latest: Vec<u8>,
    branch: Option<startup_branch::Session<'p>>,
    previous: Option<(Vec<u8>, Vec<u8>)>,
    readiness: Option<world_readiness::Session>,
    pending_readiness: Option<world_readiness::PreparedBatch>,
    departure: Option<nfs_services::departure::Departure>,
    world_start: Option<world_handshake::Binding>,
    world_started: bool,
    world_gate: Option<world_readiness::Binding>,
    telemetry: Telemetry,
    recommendations: Recommendations,
    shared_wraps: shared_wraps::Service,
    identities: Identities,
    lookup: Option<user_lookup::Profile>,
    local_social: Option<nfs_services::local_social::Current>,
    settings: Option<user_settings::Session>,
    catalogs: Option<&'p nfs_services::control_catalogs::Catalog>,
    entitlements: Option<(
        &'p nfs_services::entitlements::State,
        nfs_storage::AccountId,
    )>,
    stats: Option<&'p nfs_services::stats::Catalog>,
    current_stats: Option<nfs_services::stats::Current>,
    kickback: Option<(&'p nfs_services::kickback::State, nfs_storage::AccountId)>,
    speedwall: Option<(&'p nfs_services::speedwall::State, nfs_storage::AccountId)>,
    item_licenses: Option<&'p nfs_services::item_licenses::Content>,
    owned_local_social: bool,
    owned_menu_awards: bool,
    current_awards: Option<nfs_services::awards::Current>,
    challenges: Option<&'p nfs_services::challenges::Catalog>,
    current_challenges: Option<nfs_services::challenges::Current>,
}
fn accepted<T>(result: Result<T, Failure>) -> Result<Option<T>, Failure> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Failure::IneligibleRequest) => Ok(None),
        Err(error) => Err(error),
    }
}

impl<'p> Startup<'p> {
    pub fn new(
        initial_user_added: &[u8],
        profile: &'p startup_followup::Profile,
    ) -> Result<Self, Failure> {
        let initial = nfs_protocol::users::NotifyUserAddedInitial::decode(
            nfs_fire2::decode(
                initial_user_added,
                nfs_services::user_session::frame_limits(),
            )
            .map_err(|_| Failure::ProfileShape)?
            .ok_or(Failure::ProfileShape)?
            .frame
            .body,
            nfs_services::user_session::body_limits(),
        )
        .map_err(|_| Failure::ProfileShape)?;
        let user = initial.user_info.as_ref().ok_or(Failure::ProfileConfig)?;
        if user.blaze_id != Some(profile.persona_id()) {
            return Err(Failure::ProfileConfig);
        }
        let identities = Identities {
            persona: profile.persona_id(),
            account: user
                .external_id
                .map(|id| id as i64)
                .ok_or(Failure::ProfileConfig)?,
            name: user.name.ok_or(Failure::ProfileConfig)?.to_vec(),
        };
        Ok(Self {
            profile,
            users: Some(
                users_metrics::Followup::new(initial_user_added)
                    .map_err(crate::user_session_failure)?,
            ),
            initial: initial_user_added.to_vec(),
            latest: Vec::new(),
            branch: None,
            previous: None,
            readiness: None,
            pending_readiness: None,
            departure: None,
            world_start: None,
            world_started: false,
            world_gate: None,
            telemetry: Telemetry::new(),
            recommendations: Recommendations::new(),
            shared_wraps: shared_wraps::Service::new(),
            identities,
            lookup: None,
            local_social: None,
            settings: None,
            catalogs: None,
            entitlements: None,
            kickback: None,
            speedwall: None,
            item_licenses: None,
            stats: None,
            current_stats: None,
            owned_local_social: false,
            owned_menu_awards: false,
            challenges: None,
            current_challenges: None,
            current_awards: None,
        })
    }
    pub fn with_user_settings(
        mut self,
        settings: user_settings::Settings,
    ) -> Result<Self, Failure> {
        self.settings = Some(
            user_settings::Session::new(self.identities.persona, settings)
                .map_err(user_settings::failure)?,
        );
        Ok(self)
    }

    pub fn needs_settings_view(&self, wire: &[u8]) -> bool {
        self.settings.is_some()
            && nfs_fire2::decode(wire, crate::frame_limits())
                .ok()
                .flatten()
                .is_some_and(|d| {
                    user_settings::owns(d.frame.fields.routing_a, d.frame.fields.routing_b)
                })
    }
    pub fn settings_view(&mut self, current: user_settings::Settings) -> Result<(), Failure> {
        self.settings
            .as_mut()
            .ok_or(Failure::ProfileConfig)?
            .refresh(current)
            .map_err(user_settings::failure)
    }
    pub fn pending_settings_change(&self) -> Option<user_settings::Change> {
        self.settings.as_ref().and_then(|s| s.pending())
    }

    pub fn with_control_catalogs(
        mut self,
        catalog: &'p nfs_services::control_catalogs::Catalog,
    ) -> Self {
        self.catalogs = Some(catalog);
        self
    }
    pub fn with_owned_local_social(mut self) -> Self {
        self.owned_local_social = true;
        self
    }

    pub fn with_entitlements(
        mut self,
        state: &'p nfs_services::entitlements::State,
        account: nfs_storage::AccountId,
    ) -> Self {
        self.entitlements = Some((state, account));
        self
    }

    pub fn with_stats(mut self, catalog: &'p nfs_services::stats::Catalog) -> Self {
        self.stats = Some(catalog);
        self
    }

    pub fn with_kickback(
        mut self,
        state: &'p nfs_services::kickback::State,
        account: nfs_storage::AccountId,
    ) -> Self {
        self.kickback = Some((state, account));
        self
    }

    pub fn with_speedwall(
        mut self,
        state: &'p nfs_services::speedwall::State,
        account: nfs_storage::AccountId,
    ) -> Self {
        self.speedwall = Some((state, account));
        self
    }

    pub fn with_item_licenses(mut self, content: &'p nfs_services::item_licenses::Content) -> Self {
        self.item_licenses = Some(content);
        self
    }

    pub fn needs_account_view(&self, request: &[u8]) -> bool {
        self.needs_awards_view(request)
            || (self.users.is_none()
                && self.stats.is_some()
                && RequestRoute::of(request).is_some_and(|r| {
                    r.category == 0 && nfs_services::stats::owns(r.component, r.command)
                }))
    }

    pub fn with_owned_menu_awards(mut self) -> Self {
        self.owned_menu_awards = true;
        self
    }
    pub fn needs_awards_view(&self, request: &[u8]) -> bool {
        self.users.is_none()
            && self.owned_menu_awards
            && RequestRoute::of(request).is_some_and(|r| {
                r.category == 0 && nfs_services::awards::owns(r.component, r.command)
            })
    }
    pub fn awards_view(&mut self, current: nfs_services::awards::Current) {
        self.current_awards = Some(current);
    }

    pub fn with_challenges(mut self, catalog: &'p nfs_services::challenges::Catalog) -> Self {
        self.challenges = Some(catalog);
        self
    }
    pub fn needs_challenges_view(&self, request: &[u8]) -> bool {
        self.users.is_none()
            && self.challenges.is_some()
            && RequestRoute::of(request).is_some_and(|r| {
                r.category == 0 && nfs_services::challenges::owns(r.component, r.command)
            })
    }
    pub fn challenges_view(&mut self, current: nfs_services::challenges::Current) {
        self.current_challenges = Some(current);
    }

    pub fn account_view(&mut self, current: nfs_services::stats::Current) {
        self.current_stats = Some(current);
    }
    pub fn identities(&self) -> &Identities {
        &self.identities
    }
    pub fn shared_wraps(&self) -> &shared_wraps::Service {
        &self.shared_wraps
    }
    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }
    pub fn recommendations(&self) -> &Recommendations {
        &self.recommendations
    }
    pub fn services_open(&self) -> bool {
        self.branch.is_some()
    }

    fn open_services(&mut self) -> Result<(), Failure> {
        self.lookup = Some(
            user_lookup::Profile::from_current(&self.initial, &self.latest)
                .map_err(user_lookup::failure)?,
        );
        if self.owned_local_social {
            self.local_social = Some(
                nfs_services::local_social::Current::new(self.identities.persona, &[], true)
                    .map_err(|_| Failure::ProfileConfig)?
                    .with_empty_friend_recommendations(),
            );
        }
        if let Some(branch) = self.profile.branch() {
            let session = startup_branch::Session::new(&self.initial, &self.latest, branch)?;
            self.branch = Some(session.serving());
        }
        Ok(())
    }
    pub fn handle(&mut self, request: &[u8], unix_micros: i64) -> Result<Answer, Failure> {
        let Some(route) = RequestRoute::of(request) else {
            return Ok(Answer::Unsupported);
        };
        if route.category != 0 {
            return Ok(Answer::Unsupported);
        }
        if let Some((previous, first)) = &self.previous
            && previous == request
            && !self.needs_account_view(request)
            && !self.needs_challenges_view(request)
            && !self.needs_settings_view(request)
        {
            return Ok(Answer::Reply(vec![first.clone()]));
        }
        if let Some(answer) = self.depart(request)? {
            let first = answer.first().ok_or(Failure::Reply)?.clone();
            self.previous = Some((request.to_vec(), first));
            return Ok(Answer::Reply(answer));
        }
        if (route.component, route.command) == (4, 29)
            && self
                .readiness
                .as_ref()
                .is_some_and(|r| !r.is_self_mesh(request))
        {
            return self.host_mesh(request, unix_micros);
        }
        let answer = self.dispatch(route, request)?;
        if let Some(frames) = &answer {
            let first = frames.first().ok_or(Failure::Reply)?.clone();
            self.previous = Some((request.to_vec(), first));
        }
        Ok(answer.map_or(Answer::Unsupported, Answer::Reply))
    }

    /// Leave and disconnected-mesh reports for the committed current world.
    /// `None` leaves the request to the other handlers.
    fn depart(&mut self, request: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        use nfs_services::departure::{Departure, Error, owns};
        let Some(readiness) = self.readiness.as_ref() else {
            return Ok(None);
        };
        if !owns(request) {
            return Ok(None);
        }
        let departure = match &mut self.departure {
            Some(departure) => departure,
            slot => slot
                .insert(Departure::new(readiness.binding()).map_err(|_| Failure::ProfileConfig)?),
        };
        match departure.reply(request) {
            Ok(frames) => Ok(Some(frames)),
            Err(Error::Ineligible) => Ok(None),
            Err(Error::Encode) => Err(Failure::Reply),
            Err(Error::Context) => Err(Failure::ProfileConfig),
            Err(Error::Bound | Error::Closed | Error::Write) => Ok(None),
        }
    }
    /// The current world was left by a written leave batch.
    pub fn world_left(&self) -> bool {
        self.departure.as_ref().is_some_and(|d| d.left())
    }
    fn gate(&self) -> InitialReliableSyncPrerequisite {
        match self.world_gate {
            Some(binding) => InitialReliableSyncPrerequisite::Published { binding },
            None => InitialReliableSyncPrerequisite::Pending,
        }
    }

    fn decision(&mut self, decision: Result<Decision, world_readiness::Error>) -> Answer {
        match decision {
            Ok(Decision::Prepared(batch)) => {
                let frames = batch.frames().to_vec();
                self.pending_readiness = Some(batch);
                Answer::Reply(frames)
            }
            Ok(Decision::Ack(ack)) => Answer::Reply(vec![ack]),
            Ok(Decision::PendingPrerequisite) | Err(world_readiness::Error::AdmissionBusy) => {
                Answer::AwaitWorld
            }
            Ok(Decision::PendingWrite) | Err(_) => Answer::Unsupported,
        }
    }
    fn host_mesh(&mut self, request: &[u8], unix_micros: i64) -> Result<Answer, Failure> {
        let gate = self.gate();
        let Some(readiness) = self.readiness.as_mut() else {
            return Ok(Answer::Unsupported);
        };
        let decision = readiness.request(request, gate, Some(unix_micros));
        Ok(self.decision(decision))
    }
    pub fn world_proof(&mut self, binding: world_readiness::Binding) {
        self.world_gate = Some(binding);
    }
    pub fn resume_world(&mut self, unix_micros: i64) -> Result<Answer, Failure> {
        let gate = self.gate();
        let Some(readiness) = self.readiness.as_mut() else {
            return Ok(Answer::Unsupported);
        };
        if !readiness.has_pending_request() {
            return Ok(Answer::Unsupported);
        }
        let decision = readiness.resume(gate, Some(unix_micros));
        Ok(self.decision(decision))
    }
    pub fn take_world_start(&mut self) -> Option<world_handshake::Binding> {
        self.world_start.take()
    }

    fn dispatch(
        &mut self,
        route: RequestRoute,
        request: &[u8],
    ) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        let profile = self.profile;
        let single = |reply: Option<Vec<u8>>| reply.map(|frame| vec![frame]);
        if self.needs_challenges_view(request) {
            let current = self
                .current_challenges
                .take()
                .ok_or(Failure::ProfileConfig)?;
            let catalog = self.challenges.ok_or(Failure::ProfileConfig)?;
            return match current.reply(catalog, request, self.identities.persona) {
                Ok(frame) => Ok(Some(vec![frame])),
                Err(nfs_services::challenges::Error::Ineligible) => Ok(None),
                Err(_) => Err(Failure::ProfileConfig),
            };
        }
        if self.needs_awards_view(request) {
            let current = self.current_awards.take().ok_or(Failure::ProfileConfig)?;
            return match current.reply(request, self.identities.persona) {
                Ok(frame) => Ok(Some(vec![frame])),
                Err(nfs_services::awards::Error::Ineligible) => Ok(None),
                Err(_) => Err(Failure::ProfileConfig),
            };
        }
        if self.users.is_none()
            && nfs_services::stats::owns(route.component, route.command)
            && let Some(catalog) = self.stats
        {
            let current = self.current_stats.take().ok_or(Failure::ProfileConfig)?;
            return match catalog.reply(request, self.identities.persona, &current) {
                Ok(answer) => Ok(answer),
                Err(nfs_services::stats::Error::Ineligible) => Ok(None),
                Err(_) => Err(Failure::ProfileConfig),
            };
        }
        if let Some(settings) = self.settings.as_mut()
            && user_settings::owns(route.component, route.command)
        {
            return Ok(single(
                accepted(settings.reply(request).map_err(user_settings::failure))?.flatten(),
            ));
        }
        if nfs_services::control_catalogs::owns(route.component, route.command)
            && let Some(catalog) = self.catalogs
        {
            if self.users.is_some() {
                return Ok(None);
            }
            return match catalog.reply(request, self.identities.persona) {
                Ok(reply) => Ok(Some(vec![reply])),
                Err(nfs_services::control_catalogs::Error::Ineligible) => Ok(None),
                Err(nfs_services::control_catalogs::Error::Encode) => Err(Failure::Reply),
            };
        }
        if self.owned_local_social
            && nfs_services::local_social::owns(route.component, route.command)
        {
            let Some(current) = &self.local_social else {
                return Ok(None);
            };
            return match current.reply(request) {
                Ok(reply) => Ok(single(reply)),
                Err(nfs_services::local_social::Error::IneligibleRequest) => Ok(None),
                Err(_) => Err(Failure::ProfileConfig),
            };
        }
        if nfs_services::entitlements::owns(route.component, route.command)
            && let Some((state, account)) = self.entitlements
        {
            if self.users.is_some() {
                return Ok(None);
            }
            return match state.reply(request, account, self.identities.persona) {
                Ok(reply) => Ok(Some(vec![reply])),
                Err(nfs_services::entitlements::Error::Ineligible) => Ok(None),
                Err(_) => Err(Failure::ProfileConfig),
            };
        }
        if nfs_services::kickback::owns(route.component, route.command)
            && let Some((state, account)) = self.kickback
        {
            if self.users.is_some() {
                return Ok(None);
            }
            return match state.reply(request, account, self.identities.persona) {
                Ok(reply) => Ok(Some(vec![reply])),
                Err(nfs_services::kickback::Error::Ineligible) => Ok(None),
                Err(nfs_services::kickback::Error::Identity) => Ok(None),
                Err(nfs_services::kickback::Error::Encode) => Err(Failure::Reply),
            };
        }
        if nfs_services::speedwall::owns(route.component, route.command)
            && let Some((state, account)) = self.speedwall
        {
            if self.users.is_some() {
                return Ok(None);
            }
            return match state.reply(
                request,
                account,
                self.identities.persona,
                &self.identities.name,
            ) {
                Ok(reply) => Ok(Some(vec![reply])),
                Err(nfs_services::speedwall::Error::Encode) => Err(Failure::Reply),
                Err(_) => Ok(None),
            };
        }
        if nfs_services::item_licenses::owns(route.component, route.command)
            && let Some(content) = self.item_licenses
        {
            if self.users.is_some() {
                return Ok(None);
            }
            return match content.reply(request) {
                Ok(reply) => Ok(Some(vec![reply])),
                Err(nfs_services::item_licenses::Error::Encode) => Err(Failure::Reply),
                Err(_) => Ok(None),
            };
        }
        let prefix = match (route.component, route.command) {
            (9, 28) => single(accepted(crate::client_state::reply(request))?),
            (30722, 20) | (30722, 8) if self.users.is_some() => {
                let users = self.users.as_mut().ok_or(Failure::Reply)?;
                let state = users.state();
                let answer =
                    accepted(users.response(request).map_err(crate::user_session_failure))?
                        .flatten();
                if let Some(frames) = &answer
                    && matches!(
                        state,
                        users_metrics::State::Network | users_metrics::State::Metrics
                    )
                {
                    self.latest = frames.get(1).ok_or(Failure::Reply)?.clone();
                }
                if answer.is_some() && users.complete() {
                    self.users = None;
                    self.open_services()?;
                }
                return Ok(answer);
            }
            (30722, 8) if self.users.is_none() && self.lookup.is_some() => {
                let lookup = self.lookup.as_mut().ok_or(Failure::Reply)?;
                single(accepted(
                    lookup
                        .update_hardware(request)
                        .map_err(user_lookup::failure),
                )?)
            }
            (30722, 12) if self.lookup.is_some() => {
                let lookup = self.lookup.as_ref().ok_or(Failure::Reply)?;
                let external = user_lookup::single_account_external_resolution(
                    request,
                    self.identities.account as u64,
                );
                single(
                    accepted(
                        lookup
                            .reply(request, external)
                            .map_err(user_lookup::failure),
                    )?
                    .flatten(),
                )
            }
            (25, 6) => match nfs_services::association::reply(profile.persona_id(), request) {
                Ok(reply) => single(Some(reply)),
                Err(nfs_services::association::Error::Ineligible) => None,
                Err(nfs_services::association::Error::Identity) => {
                    return Err(Failure::ProfileConfig);
                }
                Err(nfs_services::association::Error::Encode) => return Err(Failure::Reply),
            },
            (telemetry::COMPONENT, telemetry::REPORT_TELEMETRY) if self.readiness.is_some() => {
                single(accepted(telemetry::reply(&mut self.telemetry, request))?)
            }
            (recommendations::COMPONENT, recommendations::GET_IN_GAME_RECOMMENDATIONS)
                if self.branch.is_some() =>
            {
                single(accepted(recommendations::reply(
                    &mut self.recommendations,
                    profile.persona_id(),
                    request,
                ))?)
            }
            (shared_wraps::COMPONENT, shared_wraps::LIST_SHARED_WRAPS) if self.branch.is_some() => {
                single(accepted(shared_wraps::reply(
                    &mut self.shared_wraps,
                    profile.persona_id(),
                    request,
                ))?)
            }
            (nfs_protocol::autolog::COMPONENT, nfs_protocol::autolog::GET_NEWS)
                if self.branch.is_some() =>
            {
                accepted(menu_news::reply(request).map_err(menu_news::failure))?
                    .flatten()
                    .map(|frame| vec![frame])
            }
            _ => None,
        };
        if prefix.is_some() {
            return Ok(prefix);
        }
        match self.branch.as_mut() {
            Some(branch) => Ok(accepted(branch.response(request))?.flatten()),
            None => Ok(None),
        }
    }
    pub fn committed(&mut self) -> Result<(), Failure> {
        if self.pending_settings_change().is_some() {
            return Err(Failure::Reply);
        }
        if let Some(departure) = self.departure.as_mut()
            && departure.pending()
        {
            return departure.committed().map_err(|_| Failure::Reply);
        }
        let Some(branch) = self.branch.as_mut() else {
            return Ok(());
        };
        branch.commit_after_write()?;
        if let Some(batch) = self.pending_readiness.take() {
            let readiness = self.readiness.as_mut().ok_or(Failure::Reply)?;
            readiness
                .commit_written(batch)
                .map_err(|_| Failure::Reply)?;
            if let Some(permit) = readiness.take_continuation_permit() {
                branch.enable_world_continuation(permit)?;
            }
        }
        if !self.world_started
            && let Some(binding) = branch.take_world_transport_binding()?
        {
            self.world_started = true;
            self.world_start = Some(binding);
        }
        if self.readiness.is_none()
            && let Some(session) = branch.take_world_readiness_after_write()?
        {
            self.readiness = Some(session);
        }
        Ok(())
    }
    pub fn write_failed(&mut self) {
        if let Some(settings) = self.settings.as_mut() {
            settings.abort();
        }
        if let Some(readiness) = self.readiness.as_mut() {
            readiness.abort_write();
        }
        if let Some(departure) = self.departure.as_mut() {
            departure.abort_write();
        }
        self.pending_readiness = None;
        if let Some(branch) = self.branch.as_mut() {
            branch.abort_write();
        }
    }
}
