// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explicit loopback-local identity and fresh connection credentials.
//! External auth codes are shape-checked, never verified, stored or returned.
//! The edge must authorize the chosen local account before constructing a profile.
mod config;
mod identity;
use crate::bootstrap::body_limits;
pub use config::Config;
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{
    Blob,
    authentication::{LoginRequest, LoginResponse, PersonaDetails, UserLoginInfo},
    metadata::Fire2Metadata,
    users::{
        AbsentClientData, ExtendedDataMap, NetworkQosData, NotifyUserAddedInitial, ObjectId,
        ObjectIdList, UnsetNetworkAddress, UserIdentification, UserSessionExtendedDataInitial,
        UserSessionLoginInfo, UserStatus,
    },
    util::{
        GetTelemetryServerResponse, GetTickerServerResponse, PostAuthRequest, PostAuthResponse,
        UserOptions,
    },
};
use std::net::{Ipv4Addr, SocketAddr};

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
        write!(f, "local authentication {self:?}")
    }
}
impl std::error::Error for Error {}
const DMAP_KEYS: [u32; 6] = [1, 0x70001, 0x70002, 0xe0001, 0xe0002, 0x78020001];
/// 32 session-key, 4 connection, 6 telemetry-session, 44 telemetry-key, 29 ticker bytes.
pub const SEED_BYTES: usize = 115;
#[derive(Clone)]
pub struct Identity {
    storage: nfs_storage::AccountId,
    persona: i64,
    account: i64,
    name: String,
}
impl Identity {
    /// Caller owns the persistent mapping and uniqueness of the two positive wire IDs.
    pub fn new(
        storage: nfs_storage::AccountId,
        persona: i64,
        account: i64,
        name: String,
    ) -> Result<Self, Error> {
        if persona <= 0
            || account <= 0
            || persona == account
            || name.is_empty()
            || name.len() > 64
            || name.chars().any(char::is_control)
        {
            return Err(Error::Config);
        }
        Ok(Self {
            storage,
            persona,
            account,
            name,
        })
    }
    pub fn storage_account(&self) -> nfs_storage::AccountId {
        self.storage
    }
    pub fn persona_id(&self) -> i64 {
        self.persona
    }
    pub fn account_id(&self) -> i64 {
        self.account
    }
}
/// No Debug: local keys must not enter ordinary logs.
pub struct Tokens {
    key: Vec<u8>,
    connection: i64,
    telemetry_session: Vec<u8>,
    telemetry_key: Vec<u8>,
    ticker_key: Vec<u8>,
}
fn hex(seed: &[u8], length: usize) -> Result<Vec<u8>, Error> {
    let bytes: Vec<_> = seed
        .iter()
        .flat_map(|b| {
            [
                b"0123456789abcdef"[(b >> 4) as usize],
                b"0123456789abcdef"[(b & 15) as usize],
            ]
        })
        .take(length)
        .collect();
    if bytes.len() != length || bytes.iter().all(|v| *v == b'0') {
        return Err(Error::Config);
    }
    Ok(bytes)
}
impl Tokens {
    /// Fresh entropy per connection from the edge; deterministic bytes only in tests.
    pub fn from_seed(seed: &[u8]) -> Result<Self, Error> {
        if seed.len() != SEED_BYTES {
            return Err(Error::Config);
        }
        let ticker_key = hex(&seed[86..115], 58)?;
        if ticker_key[..57].iter().all(|b| *b == b'0') {
            return Err(Error::Config);
        }
        Ok(Self {
            key: hex(&seed[..32], 59)?,
            connection: i64::from(
                (u32::from_le_bytes(seed[32..36].try_into().expect("length")) & 0x0fff_ffff)
                    | 0x3000_0000,
            ),
            telemetry_session: hex(&seed[36..42], 11)?,
            telemetry_key: hex(&seed[42..86], 87)?,
            ticker_key,
        })
    }
}
pub struct Profile {
    config: Config,
    identity: Identity,
    tokens: Tokens,
    port: u16,
}
impl Profile {
    pub fn new(
        config: Config,
        identity: Identity,
        tokens: Tokens,
        address: SocketAddr,
    ) -> Result<Self, Error> {
        config.validate()?;
        if address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
            return Err(Error::Config);
        }
        Ok(Self {
            config,
            identity,
            tokens,
            port: address.port(),
        })
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    fn notification(&self, authenticated: u32) -> Result<Vec<u8>, Error> {
        let body = UserSessionLoginInfo {
            is_first_console_login: Some(self.config.first_console),
            account_locale: Some(self.config.locale),
            blaze_user_id: Some(self.identity.persona),
            connection_group_object_id: Some(ObjectId(
                self.config.object_parts[0],
                self.config.object_parts[1],
                self.tokens.connection,
            )),
            display_name: Some(self.identity.name.as_bytes()),
            is_first_login: Some(self.config.first_login),
            session_key: Some(&self.tokens.key),
            last_authenticated: Some(authenticated),
            last_login_date_time: Some(0),
            email: Some(b"offline@localhost"),
            persona_namespace: Some(&self.config.namespace),
            persona_id: Some(self.identity.persona),
            client_platform: Some(4),
            user_id: Some(self.identity.account),
            user_session_type: Some(self.config.session_type),
            external_id: Some(self.identity.account as u64),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    routing_a: 30722,
                    routing_b: 8,
                    category: 2,
                    reserved: [1, 0],
                    ..Default::default()
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
    /// Queue all four outputs without waiting for postAuth. Request AUTH bytes
    /// never enter the generated local session.
    fn login_batch(&self, login_wire: &[u8], authenticated: u32) -> Result<Vec<Vec<u8>>, Error> {
        if !eligible_login(login_wire) {
            return Err(Error::Ineligible);
        }
        let request = frame(login_wire, [1, 10], 0, false).map_err(|_| Error::Ineligible)?;
        let message =
            LoginRequest::decode(request.body, body_limits()).map_err(|_| Error::Ineligible)?;
        if message
            .encode(body_limits())
            .map_err(|_| Error::Ineligible)?
            != request.body
        {
            return Err(Error::Ineligible);
        }
        let n = &self.config;
        let s = &self.identity;
        let metadata = Fire2Metadata {
            context: Some(0),
            error_code: Some(0),
            session_key: Some(&self.tokens.key),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let body = LoginResponse {
            is_anonymous: Some(false),
            needs_legal_doc: Some(false),
            is_of_legal_contact_age: Some(false),
            is_underage: Some(false),
            user_login_info: Some(UserLoginInfo {
                is_first_console_login: Some(n.first_console),
                blaze_user_id: Some(s.persona),
                is_first_login: Some(n.first_login),
                session_key: Some(&self.tokens.key),
                last_login_date_time: Some(i64::from(authenticated)),
                email: Some(b"offline@localhost"),
                user_id: Some(s.account),
                persona_details: Some(PersonaDetails {
                    display_name: Some(self.identity.name.as_bytes()),
                    last_authenticated: Some(0),
                    persona_id: Some(s.persona),
                    client_platform: Some(4),
                    status: Some(0),
                    external_id: Some(s.account as u64),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let added = NotifyUserAddedInitial {
            extended_data: Some(UserSessionExtendedDataInitial {
                address: Some(UnsetNetworkAddress),
                best_ping_site_alias: Some(b""),
                country: Some(b""),
                client_data: Some(AbsentClientData),
                data_map: Some(ExtendedDataMap(
                    DMAP_KEYS.into_iter().map(|key| (key, 0)).collect(),
                )),
                hardware_flags: Some(0),
                isp: Some(b""),
                latency_list: None,
                qos_data: Some(NetworkQosData {
                    bandwidth_error_code: Some(0),
                    downstream_bits_per_second: Some(0),
                    nat_error_code: Some(0),
                    nat_type: Some(0),
                    upstream_bits_per_second: Some(0),
                    ..Default::default()
                }),
                time_zone: Some(b""),
                user_info_attribute: Some(0),
                blaze_object_id_list: Some(ObjectIdList(vec![ObjectId(
                    n.object_parts[0],
                    n.object_parts[1],
                    self.tokens.connection,
                )])),
                ..Default::default()
            }),
            user_info: Some(UserIdentification {
                account_id: Some(s.account),
                account_locale: Some(n.locale),
                external_blob: Some(Blob(&[])),
                external_id: Some(s.account as u64),
                blaze_id: Some(s.persona),
                name: Some(self.identity.name.as_bytes()),
                persona_namespace: Some(&n.namespace),
                origin_persona_id: Some(s.persona as u64),
                pid_id: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let status = UserStatus {
            status_flags: Some(3),
            blaze_id: Some(s.persona),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        Ok(vec![
            self.notification(authenticated)?,
            encode(reply_fields(request.fields), &metadata, &body)?,
            encode(notification_fields(2), &[], &added)?,
            encode(notification_fields(5), &[], &status)?,
        ])
    }
    /// Produces only the observed DSUI=0/UDID-empty postAuth shape. The caller
    /// owns ordering, duplicate policy and whether login has been accepted.
    fn postauth_reply(&self, wire: &[u8]) -> Result<Vec<u8>, Error> {
        let request = frame(wire, [9, 8], 0, false).map_err(|_| Error::Ineligible)?;
        let message =
            PostAuthRequest::decode(request.body, body_limits()).map_err(|_| Error::Ineligible)?;
        if message.unknown_field_count() != 0
            || message.dirty_sock_user_index != Some(0)
            || message.unique_device_id != Some(b"")
            || message
                .encode(body_limits())
                .map_err(|_| Error::Ineligible)?
                != request.body
        {
            return Err(Error::Ineligible);
        }
        let body = PostAuthResponse {
            telemetry_server: Some(GetTelemetryServerResponse {
                address: Some(b"127.0.0.1"),
                is_anonymous: Some(false),
                disable: Some(b""),
                enable_disconnect_telemetry: Some(false),
                filter: Some(&self.config.filter),
                locale: Some(self.config.locale),
                underage: Some(false),
                no_toggle_ok: Some(&self.config.no_toggle_ok),
                port: Some(u32::from(self.port)),
                send_delay: Some(15000),
                session_id: Some(&self.tokens.telemetry_session),
                key: Some(&self.tokens.telemetry_key),
                send_percentage: Some(75),
                use_server_time: Some(&self.config.use_server_time),
                telemetry_service_name: Some(&self.config.service_name),
                ..Default::default()
            }),
            ticker_server: Some(GetTickerServerResponse {
                address: Some(b"127.0.0.1"),
                port: Some(u32::from(self.port)),
                key: Some(&self.tokens.ticker_key[..self.config.ticker_key_length]),
                ..Default::default()
            }),
            user_options: Some(UserOptions {
                telemetry_opt: Some(0),
                user_id: Some(self.identity.persona),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        encode(reply_fields(request.fields), &[], &body)
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(64 * 1024, 1024, 64 * 1024 - 16).expect("constant limits")
}
fn frame(
    source: &[u8],
    route: [u16; 2],
    category: u8,
    allow_metadata: bool,
) -> Result<Frame<'_>, Error> {
    let decoded = nfs_fire2::decode(source, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let f = decoded.frame;
    if decoded.consumed != source.len()
        || f.fields.routing_a != route[0]
        || f.fields.routing_b != route[1]
        || f.fields.category != category
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || (!allow_metadata && !f.metadata.is_empty())
    {
        return Err(Error::Ineligible);
    }
    Ok(f)
}
fn encode(fields: Fields, metadata: &[u8], body: &[u8]) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields,
            metadata,
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}
fn reply_fields(mut fields: Fields) -> Fields {
    fields.category = 1;
    fields
}
fn notification_fields(command: u16) -> Fields {
    Fields {
        routing_a: 30722,
        routing_b: command,
        category: 2,
        ..Default::default()
    }
}
pub fn eligible_login(wire: &[u8]) -> bool {
    let Ok(Some(d)) = nfs_fire2::decode(wire, frame_limits()) else {
        return false;
    };
    let f = d.frame;
    if d.consumed != wire.len()
        || f.fields.routing_a != 1
        || f.fields.routing_b != 10
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
    {
        return false;
    }
    let Ok(m) = LoginRequest::decode(f.body, body_limits()) else {
        return false;
    };
    f.metadata.is_empty()
        && m.encode(body_limits())
            .is_ok_and(|encoded| encoded == f.body)
        && m.unknown_field_count() == 0
        && m.auth_code.is_some_and(|s| !s.is_empty() && s.len() <= 256)
        && m.external_blob.is_some_and(|b| b.0.is_empty())
        && m.external_id == Some(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Login,
    PostAuth,
    Ready,
    Closed,
}
/// Bounded state for one authorized local connection. A valid retry with the same
/// correlation retains the first login time; opaque auth-code contents have no
/// bearing on local account selection and are never retained. Commit means the
/// entire batch was written, not just its first notification.
pub struct Exchange<'a> {
    profile: &'a Profile,
    stage: Stage,
    pending: Option<Stage>,
    login: Option<(u32, u32)>,
    postauth: Option<u32>,
}
impl<'a> Exchange<'a> {
    pub fn new(profile: &'a Profile) -> Self {
        Self {
            profile,
            stage: Stage::Login,
            pending: None,
            login: None,
            postauth: None,
        }
    }
    pub fn stage(&self) -> Stage {
        self.stage
    }
    pub fn login_complete(&self) -> bool {
        matches!(self.stage, Stage::PostAuth | Stage::Ready) && self.pending.is_none()
    }
    pub fn response(&mut self, wire: &[u8], now: u32) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if self.stage == Stage::Closed {
            return Err(Error::Closed);
        }
        if self.pending.is_some() {
            return Err(Error::Pending);
        }
        if eligible_login(wire) && matches!(self.stage, Stage::Login | Stage::PostAuth) {
            let correlation = frame(wire, [1, 10], 0, false)?.fields.correlation;
            let authenticated = match self.login {
                Some((prior, time)) if prior == correlation => time,
                Some(_) => return Ok(None),
                None => now,
            };
            let batch = self.profile.login_batch(wire, authenticated)?;
            self.login = Some((correlation, authenticated));
            self.pending = Some(Stage::PostAuth);
            return Ok(Some(batch));
        }
        if matches!(self.stage, Stage::PostAuth | Stage::Ready) {
            let request = match frame(wire, [9, 8], 0, false) {
                Ok(v) => v,
                Err(Error::Ineligible) => return Ok(None),
                Err(e) => return Err(e),
            };
            if self
                .postauth
                .is_some_and(|v| v != request.fields.correlation)
            {
                return Ok(None);
            }
            let reply = self.profile.postauth_reply(wire)?;
            self.postauth = Some(request.fields.correlation);
            self.pending = Some(Stage::Ready);
            return Ok(Some(vec![reply]));
        }
        Ok(None)
    }
    pub fn committed(&mut self) -> Result<(), Error> {
        if self.stage == Stage::Closed {
            return Err(Error::Closed);
        }
        if let Some(next) = self.pending.take() {
            self.stage = next;
        }
        Ok(())
    }
    pub fn write_failed(&mut self) {
        self.pending = None;
        self.login = None;
        self.postauth = None;
        self.stage = Stage::Closed;
    }
}
#[cfg(test)]
mod tests;
