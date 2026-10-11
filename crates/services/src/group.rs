// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Single-member startup groups from authenticated state and explicit local policy.
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{
    gamemanager::*,
    users::{NetworkQosData, ObjectId, UserIdentification},
};
mod config;
pub use config::Config;
mod allocation;
mod setup;
pub use allocation::SEED_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Config,
    Ineligible,
    Encode,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "group {self:?}")
    }
}
impl std::error::Error for Error {}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 16 * 1024,
        max_depth: 6,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(16 * 1024 + 16, 0, 16 * 1024).expect("constant limits")
}

/// Immutable handoff minted by this group's accepted create. It is exposed only
/// after initialization has been committed by the socket edge.
#[derive(Clone)]
pub struct Context {
    group: u64,
    player_session: u64,
    persona: i64,
    user: Vec<u8>,
    network: Vec<u8>,
    connection: u64,
    setup: Vec<u8>,
}
impl Context {
    fn from_create(
        group: u64,
        player_session: u64,
        user: &[u8],
        body: &[u8],
        connection: u64,
        setup: &[u8],
    ) -> Result<Self, Error> {
        let mut user =
            UserIdentification::decode(user, body_limits()).map_err(|_| Error::Config)?;
        let persona = user.blaze_id.ok_or(Error::Config)?;
        user.origin_persona_id = Some(0);
        let request = CreateGameRequest::decode(body, body_limits()).map_err(|_| Error::Config)?;
        let network = CommonGameRequestData {
            player_network_address: request
                .common_game_data
                .ok_or(Error::Config)?
                .player_network_address,
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        Ok(Self {
            group,
            player_session,
            persona,
            user: user.encode(body_limits()).map_err(|_| Error::Encode)?,
            network,
            connection,
            setup: setup.to_vec(),
        })
    }
    pub fn group(&self) -> u64 {
        self.group
    }
    pub fn player_session(&self) -> u64 {
        self.player_session
    }
    pub fn persona(&self) -> i64 {
        self.persona
    }
    pub fn user(&self) -> &[u8] {
        &self.user
    }
    pub fn network(&self) -> &[u8] {
        &self.network
    }
    pub fn connection(&self) -> u64 {
        self.connection
    }
    pub fn setup(&self) -> &[u8] {
        &self.setup
    }
}

/// Caller owns uniqueness and time units. No captured identifier is a default.
/// A durable multiplayer allocator may supply the same explicit fields.
#[derive(Clone)]
pub struct Generated {
    pub game_id: u64,
    pub reporting_id: u64,
    pub player_session_id: u64,
    pub seed: u32,
    pub create_time: i64,
    pub join_time: i64,
    pub game_uuid: Vec<u8>,
    pub player_uuid: Vec<u8>,
    pub external_session_name: Vec<u8>,
}
impl Generated {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.game_id == 0
            || self.game_id > i64::MAX as u64
            || self.reporting_id == 0
            || self.player_session_id == 0
            || self.game_uuid.is_empty()
            || self.player_uuid.is_empty()
            || self.game_uuid == self.player_uuid
            || self.external_session_name.is_empty()
            || [
                &self.game_uuid,
                &self.player_uuid,
                &self.external_session_name,
            ]
            .iter()
            .any(|s| s.len() > 64 || !s.iter().all(u8::is_ascii_graphic))
        {
            return Err(Error::Config);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    AwaitCreate,
    Created,
    MeshRecorded,
    Finalized,
    Initialized,
    ObserveOnly,
}
/// One current authenticated user and connection. No Debug exposes profile data.
pub struct Identity {
    user: Vec<u8>,
    connection: ObjectId,
    qos: Vec<u8>,
}
impl Identity {
    pub fn new(
        user: &UserIdentification<'_>,
        connection: ObjectId,
        qos: &NetworkQosData<'_>,
    ) -> Result<Self, Error> {
        if user.unknown_field_count() != 0
            || qos.unknown_field_count() != 0
            || user.blaze_id.is_none_or(|x| x <= 0)
            || user.account_id.is_none()
            || user.account_locale.is_none()
            || user.external_id.is_none()
            || user.external_blob.is_none()
            || user.name.is_none()
            || user.persona_namespace.is_none()
            || user.origin_persona_id.is_none()
            || user.pid_id.is_none()
            || connection.0 != 30722
            || connection.1 != 2
            || connection.2 <= 0
            || qos.downstream_bits_per_second.is_none()
            || qos.upstream_bits_per_second.is_none()
            || qos.nat_type.is_none()
            || qos.bandwidth_error_code.is_none()
            || qos.nat_error_code.is_none()
        {
            return Err(Error::Config);
        }
        Ok(Self {
            user: user.encode(body_limits()).map_err(|_| Error::Config)?,
            connection,
            qos: qos.encode(body_limits()).map_err(|_| Error::Config)?,
        })
    }
}
pub const MAX_REQUESTS: usize = 16;
pub const MAX_INPUT_BYTES: usize = 32 * 1024;
pub struct Session {
    config: Config,
    identity: Identity,
    generated: Generated,
    state: State,
    prior: Option<(Vec<u8>, Vec<u8>)>,
    requests: usize,
    input_bytes: usize,
    matchmaking_context: Option<Context>,
    pending_write: bool,
}
fn frame(bytes: &[u8], category: u8) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(bytes, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    if d.consumed != bytes.len()
        || d.frame.fields.routing_a != 4
        || d.frame.fields.category != category
        || d.frame.fields.slot != 0
        || d.frame.fields.reserved != [0, 0]
        || !d.frame.metadata.is_empty()
    {
        return Err(Error::Ineligible);
    }
    Ok(d.frame)
}
fn encode(fields: Fields, body: &[u8]) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields,
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}
fn ack(f: Frame<'_>, body: &[u8]) -> Result<Vec<u8>, Error> {
    encode(
        Fields {
            category: 1,
            ..f.fields
        },
        body,
    )
}
fn notify(command: u16, body: &[u8]) -> Result<Vec<u8>, Error> {
    encode(
        Fields {
            routing_a: 4,
            routing_b: command,
            category: 2,
            ..Default::default()
        },
        body,
    )
}

impl Session {
    pub fn new(config: Config, identity: Identity, generated: Generated) -> Result<Self, Error> {
        generated.validate()?;
        config.validate()?;
        Ok(Self {
            config,
            identity,
            generated,
            state: State::AwaitCreate,
            prior: None,
            requests: 0,
            input_bytes: 0,
            matchmaking_context: None,
            pending_write: false,
        })
    }
    pub fn state(&self) -> State {
        self.state
    }
    /// Only an initialized current group can authorize a separate M1 request.
    pub fn matchmaking_context(&self) -> Option<&Context> {
        (self.state == State::Initialized && !self.pending_write)
            .then_some(self.matchmaking_context.as_ref())
            .flatten()
    }
    pub fn game_id(&self) -> Option<u64> {
        matches!(
            self.state,
            State::Created | State::MeshRecorded | State::Finalized | State::Initialized
        )
        .then_some(self.generated.game_id)
    }
    /// Retry policy: identical immediate retries get only the RPC response;
    /// notifications and membership transitions occur once. This is local policy,
    /// not a claim that captured traffic included retries.
    pub fn response(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if self.pending_write {
            return Err(Error::Ineligible);
        }
        if self.state == State::ObserveOnly {
            return Ok(None);
        }
        if self.requests == MAX_REQUESTS
            || wire.len() > MAX_INPUT_BYTES.saturating_sub(self.input_bytes)
        {
            self.stop();
            return Ok(None);
        }
        self.requests += 1;
        self.input_bytes += wire.len();
        if let Some((old, reply)) = &self.prior
            && old == wire
        {
            return Ok(Some(vec![reply.clone()]));
        }
        let result = self.next(wire);
        if let Ok(Some(output)) = &result {
            self.prior = Some((wire.to_vec(), output[0].clone()));
            self.pending_write = true;
        } else {
            self.stop();
        }
        result
    }
    fn stop(&mut self) {
        self.state = State::ObserveOnly;
        self.prior = None;
        self.matchmaking_context = None;
        self.pending_write = false;
    }
    /// Commit only after every returned frame has been written successfully.
    pub fn commit_after_write(&mut self) {
        self.pending_write = false;
    }
    /// A failed write closes this connection's group and revokes its handoff.
    pub fn abort_write(&mut self) {
        self.stop();
    }
    fn next(&mut self, wire: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Error> {
        let f = frame(wire, 0)?;
        if let Some((old, _)) = &self.prior
            && frame(old, 0)?.fields.correlation == f.fields.correlation
        {
            return Err(Error::Ineligible);
        }
        match (self.state, f.fields.routing_b) {
            (State::AwaitCreate, CREATE_GAME) => {
                let (reply, setup) = self
                    .config
                    .create(f.body, &self.identity, &self.generated)?;
                let output = vec![ack(f, &reply)?, notify(NOTIFY_GAME_SETUP, &setup)?];
                let context = Context::from_create(
                    self.generated.game_id,
                    self.generated.player_session_id,
                    &self.identity.user,
                    f.body,
                    self.identity.connection.2 as u64,
                    &output[1],
                )?;
                self.matchmaking_context = Some(context);
                self.state = State::Created;
                Ok(Some(output))
            }
            (State::Created, UPDATE_MESH_CONNECTION) => {
                let q = UpdateMeshConnectionRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                    || q.game_id != Some(self.generated.game_id)
                    || q.source_group_id != Some(self.identity.connection)
                    || q.target_group_id != Some(self.identity.connection)
                {
                    return Err(Error::Ineligible);
                }
                config::check_mesh(q)?;
                let output = vec![ack(f, &[])?];
                self.state = State::MeshRecorded;
                Ok(Some(output))
            }
            (State::MeshRecorded, FINALIZE_GAME_CREATION) => {
                let q = UpdateGameSessionRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                    || q.game_id != Some(self.generated.game_id)
                    || q.np_session_id != Some(b"")
                    || q.xnet_nonce.as_ref().is_none_or(|b| !b.0.is_empty())
                    || q.xnet_session.as_ref().is_none_or(|b| !b.0.is_empty())
                {
                    return Err(Error::Ineligible);
                }
                let output = vec![ack(f, &[])?];
                self.state = State::Finalized;
                Ok(Some(output))
            }
            (State::Finalized, ADVANCE_GAME_STATE) => {
                let q = AdvanceGameStateRequest::decode(f.body, body_limits())
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
                    || q.game_id != Some(self.generated.game_id)
                    || q.new_game_state != Some(16)
                {
                    return Err(Error::Ineligible);
                }
                let body = NotifyGameStateChange {
                    game_id: Some(self.generated.game_id),
                    new_game_state: Some(16),
                    ..Default::default()
                }
                .encode(body_limits())
                .map_err(|_| Error::Encode)?;
                let output = vec![ack(f, &[])?, notify(NOTIFY_GAME_STATE_CHANGE, &body)?];
                self.state = State::Initialized;
                Ok(Some(output))
            }
            _ => Ok(None),
        }
    }
}
