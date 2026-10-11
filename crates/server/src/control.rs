// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ordered control-session dispatch. A reply batch must be fully written before
//! `committed`; failed writes close the session. World publication additionally
//! requires the matching transport proof. Unsupported input stays explicit.
use crate::{
    Failure,
    deployment::SessionProfiles,
    identity::Identities,
    startup::{Answer, Startup},
    user_settings,
};
use crate::{world_handshake, world_readiness};

use nfs_services::heartbeat;
pub const MAX_REPLY_FRAMES: usize = 4;

#[derive(Debug, Eq, PartialEq)]
pub enum Outcome {
    Reply(Vec<Vec<u8>>),
    Unsupported,
    AwaitWorld,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseName {
    PreAuth,
    Util,
    PostAuth,
    Startup,
    Closed,
}

enum Phase<'p> {
    OwnedBootstrap(nfs_services::bootstrap::Session<'p>),
    OwnedAuth {
        exchange: nfs_services::authentication::Exchange<'p>,
        services: Startup<'p>,
        pending: bool,
    },
    Startup {
        services: Startup<'p>,
        pending: bool,
    },
    Closed,
}

pub struct ControlSession<'p> {
    profiles: &'p SessionProfiles,
    completed_auth: Option<nfs_services::authentication::Exchange<'p>>,
    phase: Phase<'p>,
    settings: Option<user_settings::Settings>,
    catalogs: Option<&'p nfs_services::control_catalogs::Catalog>,
    entitlements: Option<(
        &'p nfs_services::entitlements::State,
        nfs_storage::AccountId,
    )>,
    stats: Option<&'p nfs_services::stats::Catalog>,
    owned_local_social: bool,
    owned_menu_awards: bool,
    challenges: Option<&'p nfs_services::challenges::Catalog>,
    kickback: Option<(&'p nfs_services::kickback::State, nfs_storage::AccountId)>,
    speedwall: Option<(&'p nfs_services::speedwall::State, nfs_storage::AccountId)>,
    item_licenses: Option<&'p nfs_services::item_licenses::Content>,
}

fn outcome(answer: Answer, pending: &mut bool) -> Result<Outcome, Failure> {
    match answer {
        Answer::Reply(batch) => {
            if batch.is_empty() || batch.len() > MAX_REPLY_FRAMES {
                return Err(Failure::BodyLimit);
            }
            *pending = true;
            Ok(Outcome::Reply(batch))
        }
        Answer::AwaitWorld => Ok(Outcome::AwaitWorld),
        Answer::Unsupported => Ok(Outcome::Unsupported),
    }
}

impl<'p> ControlSession<'p> {
    pub fn new(profiles: &'p SessionProfiles) -> Self {
        Self {
            profiles,
            completed_auth: None,
            phase: Phase::OwnedBootstrap(nfs_services::bootstrap::Session::new(
                &profiles.bootstrap,
            )),
            settings: None,
            catalogs: None,
            entitlements: None,
            stats: None,
            owned_local_social: false,
            owned_menu_awards: false,
            challenges: None,
            kickback: None,
            speedwall: None,
            item_licenses: None,
        }
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
    pub fn with_user_settings(mut self, settings: user_settings::Settings) -> Self {
        self.settings = Some(settings);
        self
    }

    pub fn needs_settings_view(&self, wire: &[u8]) -> bool {
        matches!(&self.phase, Phase::Startup { services, pending: false } if services.needs_settings_view(wire))
    }
    pub fn settings_view(&mut self, current: user_settings::Settings) -> Result<(), Failure> {
        match &mut self.phase {
            Phase::Startup { services, .. } => services.settings_view(current),
            _ => Err(Failure::Reply),
        }
    }
    pub fn pending_settings_change(&self) -> Option<user_settings::Change> {
        match &self.phase {
            Phase::Startup { services, .. } => services.pending_settings_change(),
            _ => None,
        }
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

    pub fn needs_account_view(&self, wire: &[u8]) -> bool {
        matches!(&self.phase, Phase::Startup { services, pending: false } if services.needs_account_view(wire))
    }

    pub fn with_owned_menu_awards(mut self) -> Self {
        self.owned_menu_awards = true;
        self
    }
    pub fn with_challenges(mut self, catalog: &'p nfs_services::challenges::Catalog) -> Self {
        self.challenges = Some(catalog);
        self
    }
    pub fn needs_challenges_view(&self, wire: &[u8]) -> bool {
        matches!(&self.phase, Phase::Startup { services, pending: false } if services.needs_challenges_view(wire))
    }
    pub fn challenges_view(&mut self, current: nfs_services::challenges::Current) {
        if let Phase::Startup { services, .. } = &mut self.phase {
            services.challenges_view(current);
        }
    }
    pub fn needs_awards_view(&self, wire: &[u8]) -> bool {
        matches!(&self.phase, Phase::Startup { services, pending: false } if services.needs_awards_view(wire))
    }
    pub fn awards_view(&mut self, current: nfs_services::awards::Current) {
        if let Phase::Startup { services, .. } = &mut self.phase {
            services.awards_view(current);
        }
    }

    pub fn account_view(&mut self, current: nfs_services::stats::Current) {
        if let Phase::Startup { services, .. } = &mut self.phase {
            services.account_view(current);
        }
    }
    pub fn identities(&self) -> Option<&Identities> {
        match &self.phase {
            Phase::OwnedAuth { services, .. } | Phase::Startup { services, .. } => {
                Some(services.identities())
            }
            _ => None,
        }
    }

    pub fn phase(&self) -> PhaseName {
        match self.phase {
            Phase::OwnedBootstrap(ref session) => {
                if session.stage() == nfs_services::bootstrap::Stage::PreAuth {
                    PhaseName::PreAuth
                } else {
                    PhaseName::Util
                }
            }
            Phase::OwnedAuth { .. } => PhaseName::PostAuth,
            Phase::Startup { .. } => PhaseName::Startup,
            Phase::Closed => PhaseName::Closed,
        }
    }
    pub fn on_frame(
        &mut self,
        wire: &[u8],
        unix_seconds: u32,
        unix_micros: i64,
    ) -> Result<Outcome, Failure> {
        if matches!(self.phase, Phase::Closed) {
            return Ok(Outcome::Unsupported);
        }
        if matches!(self.phase, Phase::OwnedAuth { pending: true, .. }) {
            return Err(Failure::ProfileConfig);
        }
        match heartbeat::reply(wire) {
            Ok(Some(reply)) => return Ok(Outcome::Reply(vec![reply])),
            Err(heartbeat::Error::Ineligible) => {}
            Err(heartbeat::Error::Encode) => return Err(Failure::Reply),
            Ok(None) => {}
        }
        match &mut self.phase {
            Phase::Closed => Ok(Outcome::Unsupported),
            Phase::OwnedBootstrap(session) => {
                if session.identity_complete() && nfs_services::authentication::eligible_login(wire)
                {
                    self.login(wire, unix_seconds)
                } else {
                    match session.response(wire, unix_seconds) {
                        Ok(Some(reply)) => Ok(Outcome::Reply(vec![reply])),
                        Ok(None) | Err(nfs_services::bootstrap::Error::Ineligible) => {
                            Ok(Outcome::Unsupported)
                        }
                        Err(error) => Err(crate::bootstrap_failure(error)),
                    }
                }
            }
            Phase::OwnedAuth {
                exchange, pending, ..
            } => match exchange.response(wire, unix_seconds) {
                Ok(Some(batch)) => {
                    *pending = true;
                    Ok(Outcome::Reply(batch))
                }
                Ok(None) | Err(nfs_services::authentication::Error::Ineligible) => {
                    Ok(Outcome::Unsupported)
                }
                Err(error) => Err(crate::authentication_failure(error)),
            },
            Phase::Startup { services, pending } => {
                if *pending {
                    return Err(Failure::ProfileConfig);
                }
                if let Some(exchange) = &mut self.completed_auth {
                    match exchange.response(wire, unix_seconds) {
                        Ok(Some(batch)) => {
                            *pending = true;
                            return Ok(Outcome::Reply(batch));
                        }
                        Ok(None) => {}
                        Err(nfs_services::authentication::Error::Ineligible) => {
                            return Ok(Outcome::Unsupported);
                        }
                        Err(error) => return Err(crate::authentication_failure(error)),
                    }
                }
                outcome(services.handle(wire, unix_micros)?, pending)
            }
        }
    }

    fn login(&mut self, wire: &[u8], unix_seconds: u32) -> Result<Outcome, Failure> {
        let profiles = self.profiles;
        let mut exchange = nfs_services::authentication::Exchange::new(&profiles.auth);
        let batch = exchange
            .response(wire, unix_seconds)
            .map_err(crate::authentication_failure)?
            .ok_or(Failure::IneligibleRequest)?;
        if batch.len() != 4
            || batch.iter().map(Vec::len).sum::<usize>() > crate::limits::CONTROL_FRAME
        {
            return Err(Failure::BodyLimit);
        }
        let mut services = Startup::new(&batch[2], &profiles.startup)?;
        if let Some(settings) = self.settings.take() {
            services = services.with_user_settings(settings)?;
        }
        if let Some(catalog) = self.stats {
            services = services.with_stats(catalog);
        }
        if self.owned_menu_awards {
            services = services.with_owned_menu_awards();
        }
        if self.owned_local_social {
            services = services.with_owned_local_social();
        }
        if let Some((state, account)) = self.entitlements {
            services = services.with_entitlements(state, account);
        }
        if let Some((state, account)) = self.kickback {
            services = services.with_kickback(state, account);
        }
        if let Some((state, account)) = self.speedwall {
            services = services.with_speedwall(state, account);
        }
        if let Some(content) = self.item_licenses {
            services = services.with_item_licenses(content);
        }
        if let Some(catalog) = self.catalogs {
            services = services.with_control_catalogs(catalog);
        }
        if let Some(catalog) = self.challenges {
            services = services.with_challenges(catalog);
        }
        self.phase = Phase::OwnedAuth {
            exchange,
            services,
            pending: true,
        };
        Ok(Outcome::Reply(batch))
    }
    pub fn world_proof(&mut self, binding: world_readiness::Binding) {
        if let Phase::Startup { services, .. } = &mut self.phase {
            services.world_proof(binding);
        }
    }
    pub fn resume_world(&mut self, unix_micros: i64) -> Result<Outcome, Failure> {
        match &mut self.phase {
            Phase::Startup { services, pending } if !*pending => {
                outcome(services.resume_world(unix_micros)?, pending)
            }
            _ => Ok(Outcome::Unsupported),
        }
    }
    pub fn take_world_start(&mut self) -> Option<world_handshake::Binding> {
        match &mut self.phase {
            Phase::Startup { services, .. } => services.take_world_start(),
            _ => None,
        }
    }
    pub fn committed(&mut self) -> Result<(), Failure> {
        if let Some(exchange) = &mut self.completed_auth {
            exchange
                .committed()
                .map_err(crate::authentication_failure)?;
        }
        if let Phase::OwnedBootstrap(session) = &mut self.phase {
            session.committed().map_err(crate::bootstrap_failure)?;
        }
        if let Phase::OwnedAuth {
            exchange, pending, ..
        } = &mut self.phase
        {
            if *pending {
                exchange
                    .committed()
                    .map_err(crate::authentication_failure)?;
                *pending = false;
            }
            if exchange.stage() == nfs_services::authentication::Stage::Ready {
                let Phase::OwnedAuth {
                    services, exchange, ..
                } = std::mem::replace(&mut self.phase, Phase::Closed)
                else {
                    unreachable!()
                };
                self.completed_auth = Some(exchange);
                self.phase = Phase::Startup {
                    services,
                    pending: false,
                };
            }
        }
        if let Phase::Startup { services, pending } = &mut self.phase
            && *pending
        {
            *pending = false;
            services.committed()?;
        }
        Ok(())
    }
    pub fn write_failed(&mut self) {
        if let Some(exchange) = &mut self.completed_auth {
            exchange.write_failed();
        }
        if let Phase::OwnedBootstrap(session) = &mut self.phase {
            session.write_failed();
        }
        if let Phase::OwnedAuth { exchange, .. } = &mut self.phase {
            exchange.write_failed();
        }
        if let Phase::Startup { services, .. } = &mut self.phase {
            services.write_failed();
        }
        self.phase = Phase::Closed;
    }
}
