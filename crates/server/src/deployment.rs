// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Operator-owned policy and fresh per-connection profiles. This module has no
//! captured-source constructors: identities, endpoints, time and entropy are
//! explicit inputs, and missing configuration fails closed.
use crate::{
    Failure,
    seeds::{SeedSource, fresh},
    startup_branch, startup_profile, world_handshake,
};
use std::net::SocketAddr;
#[derive(Clone, Copy, Debug)]
pub struct Endpoints {
    pub auxiliary: SocketAddr,
    pub qos: SocketAddr,
    pub world: SocketAddr,
}
pub struct SessionProfiles {
    pub bootstrap: nfs_services::bootstrap::Profile,
    pub auth: nfs_services::authentication::Profile,
    pub startup: startup_profile::Profile,
    pub mac_template: Option<[u8; 64]>,
    pub persona: i64,
    pub records: crate::auxiliary::EmptyLocalRecords,
}
#[derive(Clone, Copy, Debug)]
pub struct Features {
    pub world_setup: bool,
    pub world_mac_template: bool,
}
#[derive(Default)]
pub struct Deployment {
    bootstrap: Option<nfs_services::bootstrap::Config>,
    authentication: Option<(
        nfs_services::authentication::Config,
        nfs_services::authentication::Identity,
    )>,
    group: Option<nfs_services::group::Config>,
    admission: Option<nfs_services::matchmaking::Config>,
    status: Option<nfs_services::matchmaking_status::Config>,
    world: Option<nfs_services::world_setup::Config>,
    mac_template: Option<[u8; 64]>,
}
impl Deployment {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_bootstrap(mut self, config: nfs_services::bootstrap::Config) -> Self {
        self.bootstrap = Some(config);
        self
    }
    pub fn with_authentication(
        mut self,
        config: nfs_services::authentication::Config,
        identity: nfs_services::authentication::Identity,
    ) -> Result<Self, Failure> {
        config.validate().map_err(crate::authentication_failure)?;
        self.authentication = Some((config, identity));
        Ok(self)
    }
    pub fn with_group_policy(
        mut self,
        config: nfs_services::group::Config,
    ) -> Result<Self, Failure> {
        config.validate().map_err(crate::group_failure)?;
        self.group = Some(config);
        Ok(self)
    }
    pub fn with_matchmaking_admission(mut self, config: nfs_services::matchmaking::Config) -> Self {
        self.admission = Some(config);
        self
    }
    pub fn with_matchmaking_policy(
        mut self,
        config: nfs_services::matchmaking_status::Config,
    ) -> Self {
        self.status = Some(config);
        self
    }
    pub fn with_world_policy(mut self, config: nfs_services::world_setup::Config) -> Self {
        self.world = Some(config);
        self
    }
    pub fn with_world_mac_template(mut self, bytes: &[u8]) -> Result<Self, Failure> {
        self.mac_template = Some(world_handshake::validate_template(bytes)?);
        Ok(self)
    }
    pub fn validate(self) -> Result<Self, Failure> {
        if self.bootstrap.is_none()
            || self.authentication.is_none()
            || self.group.is_none()
            || self.admission.is_none()
            || self.status.is_none()
            || self.world.is_none()
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(self)
    }
    pub fn features(&self) -> Features {
        Features {
            world_setup: self.world.is_some(),
            world_mac_template: self.mac_template.is_some(),
        }
    }
    pub fn session_profiles(
        &self,
        endpoints: Endpoints,
        seeds: &mut dyn SeedSource,
        unix_micros: i64,
    ) -> Result<SessionProfiles, Failure> {
        let config = self.bootstrap.as_ref().ok_or(Failure::ProfileConfig)?;
        let bootstrap =
            nfs_services::bootstrap::Profile::new(config, endpoints.auxiliary, endpoints.qos)
                .map_err(crate::bootstrap_failure)?;
        let (config, identity) = self.authentication.as_ref().ok_or(Failure::ProfileConfig)?;
        let entropy = fresh::<{ nfs_services::authentication::SEED_BYTES }>(seeds)?;
        let tokens = nfs_services::authentication::Tokens::from_seed(&entropy)
            .map_err(crate::authentication_failure)?;
        let auth = nfs_services::authentication::Profile::new(
            config.clone(),
            identity.clone(),
            tokens,
            endpoints.auxiliary,
        )
        .map_err(crate::authentication_failure)?;
        let persona = identity.persona_id();
        let records = crate::auxiliary::EmptyLocalRecords::new(persona, identity.account_id())
            .ok_or(Failure::ProfileConfig)?;
        let group = nfs_services::group::Generated::from_seed(
            &fresh::<{ nfs_services::group::SEED_BYTES }>(seeds)?,
            unix_micros,
        )
        .map_err(crate::group_failure)?;
        let matchmaking = nfs_services::matchmaking::Generated::from_seed(&fresh::<
            { nfs_services::matchmaking::SEED_BYTES },
        >(seeds)?)
        .map_err(|_| Failure::ProfileConfig)?;
        let world = nfs_services::world_setup::Generated::from_seed(
            &fresh::<{ nfs_services::world_setup::SEED_BYTES }>(seeds)?,
            unix_micros,
            endpoints.world,
        )
        .map_err(|_| Failure::ProfileConfig)?;
        let branch = startup_branch::SoloProfile::owned(persona)?
            .with_group(
                self.group.as_ref().ok_or(Failure::ProfileConfig)?.clone(),
                group,
            )?
            .with_owned_matchmaking(
                self.admission
                    .as_ref()
                    .ok_or(Failure::ProfileConfig)?
                    .clone(),
                self.status.as_ref().ok_or(Failure::ProfileConfig)?.clone(),
                self.world.as_ref().ok_or(Failure::ProfileConfig)?.clone(),
                matchmaking,
                world,
            )?;
        let startup = startup_profile::Profile::owned(persona)?.with_solo_branch(branch)?;
        Ok(SessionProfiles {
            bootstrap,
            auth,
            startup,
            mac_template: self.mac_template,
            persona,
            records,
        })
    }
}
