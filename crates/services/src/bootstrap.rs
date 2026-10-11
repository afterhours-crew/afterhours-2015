// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local pre-authentication configuration and ordered ping/identity exchange.
//! Deployment data is separate from endpoints, time and per-connection state.
//! These routes do not authenticate an account or validate external credentials.
use nfs_fire2::{Fields, Frame};
use nfs_protocol::util::*;
use std::net::{Ipv4Addr, SocketAddr};
mod content;
pub use content::Config;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Config,
    Ineligible,
    Pending,
    Closed,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bootstrap {self:?}")
    }
}
impl std::error::Error for Error {}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 64 * 1024 - 16,
        max_depth: 8,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(64 * 1024, 0, 64 * 1024 - 16).expect("constant limits")
}
pub struct Profile {
    preauth: Vec<u8>,
    identity: Vec<u8>,
    service_name: String,
}
impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapProfile").finish_non_exhaustive()
    }
}
impl Profile {
    /// Endpoints must be already-bound IPv4 loopback listeners. The configured
    /// bandwidth site remains unset; every configured latency site uses `qos`.
    pub fn new(config: &Config, auxiliary: SocketAddr, qos: SocketAddr) -> Result<Self, Error> {
        if [auxiliary, qos]
            .iter()
            .any(|a| a.ip() != Ipv4Addr::LOCALHOST || a.port() == 0)
        {
            return Err(Error::Config);
        }
        let base = format!("http://127.0.0.1:{}", auxiliary.port());
        let port = auxiliary.port().to_string();
        let entries = config
            .settings
            .iter()
            .map(|(key, value)| {
                let value = match key.as_str() {
                    "bytevaultHostname" => "127.0.0.1",
                    "bytevaultPort" | "telemetryPinServerPort" => &port,
                    "bytevaultSecure" => "0",
                    "xblTokenUrn" => "",
                    _ if content::URL_KEYS.contains(&key.as_str()) => &base,
                    _ => value.as_deref().expect("validated scalar"),
                };
                (key.as_bytes(), value.as_bytes())
            })
            .collect();
        let m = |name: &str| config.metadata[name].as_bytes();
        let preauth = PreAuthResponse {
            authentication_source: Some(m("authentication_source")),
            component_ids: Some(ComponentIds(config.components.clone())),
            client_id: Some(m("client_id")),
            config: Some(FetchConfigResponse {
                config: Some(ConfigEntries(entries)),
                ..Default::default()
            }),
            entitlement_source: Some(m("entitlement_source")),
            service_name: Some(m("service_name")),
            machine_id: Some(1),
            underage_supported: Some(config.underage),
            persona_namespace: Some(m("persona_namespace")),
            legal_doc_game_identifier: Some(m("legal_doc_game_identifier")),
            platform: Some(m("platform")),
            registration_source: Some(m("registration_source")),
            server_version: Some(m("server_version")),
            qos_settings: Some(QosConfigInfo {
                bandwidth_ping_site_info: Some(QosPingSiteInfo {
                    address: Some(b""),
                    port: Some(0),
                    site_name: Some(b""),
                    ..Default::default()
                }),
                num_latency_probes: Some(config.probes),
                service_id: Some(config.service_id),
                timeout: Some(config.timeout),
                ping_site_info_by_alias_map: Some(PingSites(
                    config
                        .sites
                        .iter()
                        .map(|(alias, name)| {
                            (
                                alias.as_bytes(),
                                QosPingSiteInfo {
                                    address: Some(b"127.0.0.1"),
                                    port: Some(qos.port()),
                                    site_name: Some(name.as_bytes()),
                                    ..Default::default()
                                },
                            )
                        })
                        .collect(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let redirect = format!("{base}/identity/callback");
        let identity = FetchConfigResponse {
            config: Some(ConfigEntries(vec![
                (b"client_id", m("client_id")),
                (b"display", m("identity_display")),
                (b"redirect_uri", redirect.as_bytes()),
            ])),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        Ok(Self {
            preauth,
            identity,
            service_name: config.metadata["service_name"].clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    PreAuth,
    Ping,
    Identity,
    Ready,
    Closed,
}
/// One connection. State transitions commit only after the entire reply writes.
/// Immediate exact retries retain their original clock value; different requests
/// are validated against the current stage. Failed writes permanently close it.
pub struct Session<'a> {
    profile: &'a Profile,
    stage: Stage,
    pending: Option<(Vec<u8>, Vec<u8>, Stage)>,
    previous: Option<(Vec<u8>, Vec<u8>)>,
}
impl<'a> Session<'a> {
    pub fn new(profile: &'a Profile) -> Self {
        Self {
            profile,
            stage: Stage::PreAuth,
            pending: None,
            previous: None,
        }
    }
    pub fn stage(&self) -> Stage {
        self.stage
    }
    pub fn identity_complete(&self) -> bool {
        self.stage == Stage::Ready && self.pending.is_none()
    }
    pub fn response(&mut self, wire: &[u8], server_time: u32) -> Result<Option<Vec<u8>>, Error> {
        if self.stage == Stage::Closed {
            return Err(Error::Closed);
        }
        if self.pending.is_some() {
            return Err(Error::Pending);
        }
        if let Some((previous, reply)) = &self.previous
            && previous == wire
        {
            let reply = reply.clone();
            self.pending = Some((wire.to_vec(), reply.clone(), self.stage));
            return Ok(Some(reply));
        }
        let decoded = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = decoded.frame;
        if decoded.consumed != wire.len()
            || f.fields.routing_a != 9
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Error::Ineligible);
        }
        let (body, next) = match (self.stage, f.fields.routing_b) {
            (Stage::PreAuth, 7) => {
                let q =
                    PreAuthRequest::decode(f.body, body_limits()).map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.client_data.as_ref().is_none_or(|v| {
                        v.client_type != Some(0)
                            || v.service_name != Some(self.profile.service_name.as_bytes())
                    })
                    || q.client_info.as_ref().is_none_or(|v| v.platform != Some(4))
                    || q.fetch_client_config
                        .as_ref()
                        .is_none_or(|v| v.config_section.is_none())
                    || q.local_address.is_none()
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                (self.profile.preauth.clone(), Stage::Ping)
            }
            (Stage::Ping, 2) if f.body.is_empty() => (
                PingResponse {
                    server_time: Some(server_time),
                    ..Default::default()
                }
                .encode(body_limits())
                .map_err(|_| Error::Encode)?,
                Stage::Identity,
            ),
            (Stage::Identity, 1) => {
                let q = FetchClientConfigRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.config_section != Some(b"IdentityParams")
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                (self.profile.identity.clone(), Stage::Ready)
            }
            _ => return Ok(None),
        };
        let reply = nfs_fire2::encode(
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
        .map_err(|_| Error::Encode)?;
        self.pending = Some((wire.to_vec(), reply.clone(), next));
        Ok(Some(reply))
    }
    pub fn committed(&mut self) -> Result<(), Error> {
        if self.stage == Stage::Closed {
            return Err(Error::Closed);
        }
        if let Some((request, reply, next)) = self.pending.take() {
            self.previous = Some((request, reply));
            self.stage = next;
        }
        Ok(())
    }
    pub fn write_failed(&mut self) {
        self.pending = None;
        self.previous = None;
        self.stage = Stage::Closed;
    }
}

#[cfg(test)]
mod tests;
